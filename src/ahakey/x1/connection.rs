use super::{ModelX1, notifications::Notifications};
use crate::{
    ahakey::{hardware::Hardware, state::HardwareState},
    error::{Error, Result},
};
use async_trait::async_trait;
#[cfg(target_os = "macos")]
use btleplug::api::RetrievePeripheralsOptions;
use btleplug::api::{Central, CentralEvent, Peripheral as _, WriteType, bleuuid::uuid_from_u16};
use futures_util::StreamExt;
use std::time::Duration;
impl ModelX1 {
    pub(super) async fn exchange(&mut self, command: &[u8]) -> Result<Vec<u8>> {
        let started = std::time::Instant::now();
        crate::debug::event("ble.tx", || {
            serde_json::json!({
                "deviceId":format!("x1-{}", self.peripheral.id()),
                "frame":crate::debug::bytes(command),
            })
        });
        let mut phase = "GATT write";
        let outcome = tokio::time::timeout(Duration::from_secs(4), async {
            let characteristic = self
                .command_char
                .as_ref()
                .ok_or(Error("DEVICE_NOT_READY"))?;
            let notifications = self
                .notifications
                .as_mut()
                .ok_or(Error("DEVICE_NOT_READY"))?;
            self.peripheral
                .write(characteristic, command, WriteType::WithResponse)
                .await
                .map_err(|_| Error("DEVICE_DISCONNECTED"))?;
            phase = "protocol response";
            notifications.response(command[2]).await
        })
        .await
        .unwrap_or(Err(Error("DEVICE_TIMEOUT")));
        crate::debug::event("ble.exchange", || {
            serde_json::json!({
                "deviceId":format!("x1-{}", self.peripheral.id()), "opcode":command.get(2),
                "phase":phase, "durationMs":started.elapsed().as_secs_f64() * 1000.0,
                "error":outcome.as_ref().err().map(|error| error.0),
            })
        });
        // An ACK has no request ID. Never reuse a session after an uncertain exchange.
        if let Err(error) = &outcome {
            eprintln!(
                "X1 command 0x{:02x} failed during {phase}: {error}",
                command[2]
            );
            let _ = self.disconnect().await;
        }
        outcome
    }
    pub(super) async fn open(&mut self) -> Result<HardwareState> {
        #[cfg(target_os = "macos")]
        {
            // CoreBluetooth removes the internal peripheral on disconnect. Keeping
            // a Rust Peripheral clone alive does not keep that registration alive.
            // Reacquire by stable ID before connecting, including after timeouts.
            let id = self.peripheral.id();
            let peripherals = self
                ._adapter
                .retrieve_peripherals(RetrievePeripheralsOptions {
                    identifiers: Some(vec![id.clone()]),
                    services: None,
                })
                .await
                .map_err(|error| {
                    eprintln!("X1 BLE retrieve_peripherals: {error}");
                    Error("DEVICE_NOT_READY")
                })?;
            self.peripheral = peripherals
                .into_iter()
                .find(|p| p.id() == id)
                .ok_or(Error("DEVICE_NOT_READY"))?;
        }
        let id = self.peripheral.id();
        let disconnects = self
            ._adapter
            .events()
            .await
            .map_err(|_| Error("DEVICE_NOT_READY"))?
            .filter_map(move |event| {
                let disconnected = matches!(event, CentralEvent::DeviceDisconnected(ref device)
                    | CentralEvent::DeviceServicesModified(ref device) if *device == id);
                std::future::ready(disconnected.then_some(()))
            });
        self.peripheral.connect().await.map_err(|error| {
            eprintln!("X1 BLE connect: {error}");
            Error("DEVICE_NOT_READY")
        })?;
        self.peripheral.discover_services().await.map_err(|error| {
            eprintln!("X1 BLE discover_services: {error}");
            Error("DEVICE_NOT_READY")
        })?;
        let characteristics = self.peripheral.characteristics();
        crate::debug::event("ble.services", || {
            serde_json::json!({
                "deviceId":format!("x1-{}", self.peripheral.id()),
                "characteristics":characteristics.iter().map(|c| serde_json::json!({
                    "service":c.service_uuid.to_string(), "uuid":c.uuid.to_string(),
                    "properties":format!("{:?}", c.properties),
                })).collect::<Vec<_>>(),
            })
        });
        let find = |uuid| {
            characteristics
                .iter()
                .find(|c| c.service_uuid == uuid_from_u16(0x7340) && c.uuid == uuid_from_u16(uuid))
                .cloned()
                .ok_or(Error("UNSUPPORTED_CAPABILITY"))
        };
        self.command_char = Some(find(0x7343)?);
        let notify = find(0x7344)?;
        self.notifications = Some(
            Notifications::new(
                self.peripheral.notifications().await.map_err(|error| {
                    eprintln!("X1 BLE notifications: {error}");
                    Error("DEVICE_NOT_READY")
                })?,
                Box::pin(disconnects),
            )
            .with_device(format!("x1-{}", self.peripheral.id())),
        );
        self.peripheral.subscribe(&notify).await.map_err(|error| {
            eprintln!("X1 BLE subscribe: {error}");
            Error("DEVICE_NOT_READY")
        })?;
        // The worker performs optional configuration readback after this opens.
        self.refresh().await
    }
}
#[async_trait]
impl super::transport::CommandTransport for ModelX1 {
    async fn query(&mut self, command: &[u8]) -> Result<Vec<u8>> {
        let result = self.exchange(command).await;
        if command[2] == 0x9f
            && result
                .as_ref()
                .err()
                .is_some_and(|error| matches!(error.0, "DEVICE_TIMEOUT" | "DEVICE_REJECTED"))
        {
            // Nonresponsive legacy capability queries need one fresh session
            // without probing. A lost link during configuration readback still reads on reconnect.
            self.skip_readback_once = true;
        }
        result
    }
}
