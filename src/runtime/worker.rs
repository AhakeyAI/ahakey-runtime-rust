use super::{Runtime, state::Job};
use crate::{
    ahakey::hardware::Hardware,
    configuration::{Apply, Configuration, Scope},
    error::Error,
    operation::Mutation,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use uuid::Uuid;

impl Runtime {
    pub(super) async fn worker(
        self: Arc<Self>,
        id: String,
        mut hardware: Box<dyn Hardware>,
        mut receiver: mpsc::Receiver<Job>,
        mut initialize: Option<Configuration>,
        simulated: bool,
    ) {
        // Retry connection failures only. Connected devices wait on notifications.
        let mut retry = tokio::time::interval(Duration::from_secs(3));
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut connected = false;
        let mut seed_mock = simulated;
        let mut recover_oled = self
            .state
            .lock()
            .await
            .journal
            .operations
            .values()
            .any(|record| record.operation.device_id == id && record.needs_oled_recovery());
        loop {
            let job = tokio::select! {
                biased;
                event = hardware.next_event(), if connected => {
                    match event {
                        Ok(status) => self.publish_hardware(&id, &status).await,
                        Err(error) => {
                            eprintln!("Device {id}: {error}");
                            let _ = hardware.disconnect().await;
                            connected = false;
                            self.disconnected(&id).await;
                        }
                    }
                    continue;
                },
                job = receiver.recv() => match job { Some(job) => Some(job), None => break },
                _ = retry.tick(), if !connected => None,
            };
            match job {
                Some(Job::Shutdown) => break,
                Some(Job::Apply(request)) => {
                    self.execute_apply(hardware.as_mut(), &request).await;
                    if matches!(request, Mutation::Oled(_)) {
                        recover_oled = self.state.lock().await.journal.operations
                            [request.operation_id()]
                        .needs_oled_recovery();
                    }
                    connected =
                        self.state.lock().await.devices[&id].device.connection_state == "ready";
                    continue;
                }
                Some(Job::ResumeOled(request)) => {
                    self.execute_oled(hardware.as_mut(), &request, true).await;
                    recover_oled = self.state.lock().await.journal.operations
                        [&request.operation_id]
                        .needs_oled_recovery();
                    connected =
                        self.state.lock().await.devices[&id].device.connection_state == "ready";
                    continue;
                }
                Some(Job::ReadOled(reply)) => {
                    let result = hardware.oled_state().await;
                    if result
                        .as_ref()
                        .err()
                        .is_some_and(|error| error.0 != "UNSUPPORTED_CAPABILITY")
                    {
                        let _ = hardware.disconnect().await;
                        connected = false;
                        self.disconnected(&id).await;
                    }
                    self.state.lock().await.devices.get_mut(&id).unwrap().busy = false;
                    let _ = reply.send(result);
                    continue;
                }
                Some(Job::Disconnect) => {
                    let _ = hardware.disconnect().await;
                    connected = false;
                    self.disconnected(&id).await;
                }
                Some(Job::Connect) | None => {
                    let wanted = self.state.lock().await.devices[&id].desired_connected;
                    if !wanted || connected {
                        if job.is_some() {
                            self.state.lock().await.devices.get_mut(&id).unwrap().busy = false;
                        }
                        continue;
                    }
                    self.set_connecting(&id).await;
                    let result = async {
                        let mut status = hardware.connect().await?;
                        if recover_oled {
                            let screen = hardware.oled_state().await?;
                            if screen
                                .entries
                                .iter()
                                .any(|entry| entry.write_state == "writing")
                            {
                                return Err(Error("RESOURCE_RECOVERY_PENDING"));
                            }
                        }
                        let configuration = match hardware.read_configuration().await {
                            Ok(configuration) => configuration,
                            Err(error)
                                if matches!(
                                    error.0,
                                    "UNSUPPORTED_CONFIGURATION" | "CONFIGURATION_CHANGED"
                                ) =>
                            {
                                eprintln!("Device {id} configuration readback: {error}");
                                None
                            }
                            Err(error) => return Err(error),
                        };
                        if let Some(configuration) = &configuration {
                            // Consume the latest status after the chunk sequence,
                            // including notifications observed during readback.
                            status = hardware.refresh().await?;
                            if status.brightness
                                != (1..=100)
                                    .contains(&configuration.device.brightness_percent)
                                    .then_some(configuration.device.brightness_percent)
                            {
                                return Err(Error("CONFIGURATION_CHANGED"));
                            }
                        }
                        Ok((status, configuration))
                    }
                    .await;
                    match result {
                        Ok((status, configuration)) => {
                            connected = true;
                            recover_oled = false;
                            if let Some(configuration) = configuration {
                                let mut state = self.state.lock().await;
                                let entry = state.devices.get_mut(&id).unwrap();
                                entry.document.configuration =
                                    configuration.complete_configuration();
                                entry.document.readback = Some(configuration);
                                entry.document.base_token = Uuid::new_v4().to_string();
                                state.emit("configuration.changed", json!({"deviceId": id}));
                            }
                            self.publish_hardware(&id, &status).await;
                            if seed_mock {
                                let mut state = self.state.lock().await;
                                state.devices.get_mut(&id).unwrap().document.configuration =
                                    Some(Configuration::mock_defaults());
                                state.emit("configuration.changed", json!({"deviceId": id}));
                                seed_mock = false;
                            }
                        }
                        Err(error) => {
                            eprintln!("Device {id}: {error}");
                            let _ = hardware.disconnect().await;
                            connected = false;
                            self.disconnected(&id).await;
                        }
                    }
                }
            }
            // Clear only a reservation for this control job, never one admitted while connecting.
            if job.is_some() {
                self.state.lock().await.devices.get_mut(&id).unwrap().busy = false;
            }
            if connected && let Some(changes) = initialize.take() {
                let base_token = self.configuration(&id).await.unwrap().base_token;
                let request = Apply {
                    operation_id: Uuid::new_v4().to_string(),
                    device_id: id.clone(),
                    base_token,
                    scope: Scope {
                        device_fields: vec!["brightnessPercent".into()],
                        modes: vec![0, 1, 2, 3],
                    },
                    changes,
                };
                match self.accept(request.into(), true).await {
                    Ok(operation) => {
                        eprintln!("Initialization accepted: {}", operation.operation_id)
                    }
                    Err(error) => eprintln!("Initialization failed: {error}"),
                }
            }
        }
        let _ = hardware.disconnect().await;
        self.disconnected(&id).await;
    }
}
