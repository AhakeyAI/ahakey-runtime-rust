use super::{
    hardware::Hardware,
    state::{HardwareState, X1State},
};
use crate::{
    error::{Error, Result},
    protocol::x1::{frame, valid_frame},
};
use async_trait::async_trait;
#[cfg(target_os = "macos")]
use btleplug::api::{Central, RetrievePeripheralsOptions};
use btleplug::{
    api::{Characteristic, Peripheral as _, ValueNotification, WriteType, bleuuid::uuid_from_u16},
    platform::{Adapter, Peripheral},
};
use futures_util::{Stream, StreamExt};
use std::{pin::Pin, time::Duration};

type Notifications = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

/// X1 alone owns its GATT session and the notification stream used to confirm commands.
pub struct ModelX1 {
    // Peripheral uses a weak adapter reference on macOS. Keep its owner alive.
    _adapter: Adapter,
    peripheral: Peripheral,
    command_char: Option<Characteristic>,
    notifications: Option<Notifications>,
}
impl ModelX1 {
    pub fn new(peripheral: Peripheral, adapter: Adapter) -> Self {
        Self {
            _adapter: adapter,
            peripheral,
            command_char: None,
            notifications: None,
        }
    }
    async fn exchange(&mut self, command: &[u8]) -> Result<Vec<u8>> {
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
            while let Some(notification) = notifications.next().await {
                let data = notification.value;
                if notification.uuid != uuid_from_u16(0x7344)
                    || !valid_frame(&data)
                    || data[2] != command[2]
                {
                    continue;
                }
                if command[2] == 0 {
                    if X1State::from_bytes(&data).is_some() {
                        return Ok(data);
                    }
                } else if data.len() >= 6 {
                    return if data[3] == 0 {
                        Ok(data)
                    } else {
                        Err(Error("DEVICE_REJECTED"))
                    };
                }
            }
            Err(Error("DEVICE_DISCONNECTED"))
        })
        .await
        .unwrap_or(Err(Error("DEVICE_TIMEOUT")));
        // An ACK has no request ID. Never reuse a session after an uncertain exchange.
        if outcome.is_err() {
            let _ = self.disconnect().await;
        }
        outcome
    }
    async fn open(&mut self) -> Result<HardwareState> {
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
        self.peripheral.connect().await.map_err(|error| {
            eprintln!("X1 BLE connect: {error}");
            Error("DEVICE_NOT_READY")
        })?;
        self.peripheral.discover_services().await.map_err(|error| {
            eprintln!("X1 BLE discover_services: {error}");
            Error("DEVICE_NOT_READY")
        })?;
        let characteristics = self.peripheral.characteristics();
        let find = |uuid| {
            characteristics
                .iter()
                .find(|c| c.service_uuid == uuid_from_u16(0x7340) && c.uuid == uuid_from_u16(uuid))
                .cloned()
                .ok_or(Error("UNSUPPORTED_CAPABILITY"))
        };
        self.command_char = Some(find(0x7343)?);
        let notify = find(0x7344)?;
        self.notifications = Some(self.peripheral.notifications().await.map_err(|error| {
            eprintln!("X1 BLE notifications: {error}");
            Error("DEVICE_NOT_READY")
        })?);
        self.peripheral.subscribe(&notify).await.map_err(|error| {
            eprintln!("X1 BLE subscribe: {error}");
            Error("DEVICE_NOT_READY")
        })?;
        self.refresh().await
    }
}
#[async_trait]
impl Hardware for ModelX1 {
    fn configuration_commands(
        &self,
        configuration: &crate::configuration::Configuration,
    ) -> Result<Vec<Vec<u8>>> {
        crate::protocol::x1::configuration_commands(configuration)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        let result = tokio::time::timeout(Duration::from_secs(15), self.open())
            .await
            .unwrap_or(Err(Error("DEVICE_TIMEOUT")));
        if result.is_err() {
            let _ = self.disconnect().await;
        }
        result
    }
    async fn disconnect(&mut self) -> Result<()> {
        self.command_char = None;
        self.notifications = None;
        tokio::time::timeout(Duration::from_secs(3), self.peripheral.disconnect())
            .await
            .map_err(|_| Error("DEVICE_TIMEOUT"))?
            .map_err(|_| Error("DEVICE_DISCONNECTED"))
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        let bytes = self.exchange(&frame(0, &[])).await?;
        X1State::from_bytes(&bytes)
            .map(|s| s.0)
            .ok_or(Error("DEVICE_INVALID_RESPONSE"))
    }
    async fn write_confirmed(&mut self, command: &[u8]) -> Result<()> {
        self.exchange(command).await?;
        if command[2] == 0x85 && self.refresh().await?.brightness != Some(command[3]) {
            return Err(Error("UNSUPPORTED_CAPABILITY"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(())
    }
}
