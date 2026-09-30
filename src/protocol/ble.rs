use crate::{
    ahakey::{device::AhaKey, model_x1::ModelX1},
    configuration::Configuration,
    error::{Error, Result},
    runtime::Runtime,
};
use btleplug::{
    api::{
        Central, Manager as _, Peripheral as _, RetrievePeripheralsOptions, ScanFilter,
        bleuuid::uuid_from_u16,
    },
    platform::Manager,
};
use std::{sync::Arc, time::Duration};

/// The first frontend handles one device. Selection stays fixed for the process lifetime.
pub async fn discover(
    runtime: Arc<Runtime>,
    selected: Option<String>,
    initialize: Option<Configuration>,
) {
    loop {
        crate::debug::event(
            "ble.discovery",
            || serde_json::json!({"selectedDevice":selected}),
        );
        match find_x1(selected.as_deref()).await {
            Ok(Some(device)) => {
                if let Err(error) = runtime.register(device, initialize, false).await {
                    eprintln!("Device registration: {error}");
                }
                return;
            }
            Ok(None) => crate::debug::event("ble.notFound", || serde_json::json!({})),
            Err(error) => eprintln!("Bluetooth discovery: {error}"),
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
async fn find_x1(selected: Option<&str>) -> Result<Option<AhaKey>> {
    let manager = Manager::new()
        .await
        .map_err(|_| Error("BLUETOOTH_UNAVAILABLE"))?;
    let adapters = manager
        .adapters()
        .await
        .map_err(|_| Error("BLUETOOTH_UNAVAILABLE"))?;
    for adapter in adapters {
        let mut devices = adapter
            .retrieve_peripherals(RetrievePeripheralsOptions {
                identifiers: None,
                services: Some(vec![uuid_from_u16(0x7340)]),
            })
            .await
            .unwrap_or_default();
        // Also discover advertising peripherals; retrieve alone misses these on macOS.
        if adapter
            .start_scan(ScanFilter {
                services: vec![uuid_from_u16(0x7340)],
            })
            .await
            .is_ok()
        {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let found = adapter.peripherals().await;
            let _ = adapter.stop_scan().await;
            devices.extend(found.map_err(|_| Error("BLUETOOTH_UNAVAILABLE"))?);
        }
        devices.sort_by_key(|p| p.id().to_string());
        for peripheral in devices {
            let id = format!("x1-{}", peripheral.id());
            if selected.is_some_and(|wanted| wanted != id) {
                continue;
            }
            let properties = peripheral
                .properties()
                .await
                .map_err(|_| Error("BLUETOOTH_UNAVAILABLE"))?;
            let Some(properties) = properties else {
                continue;
            };
            let name = properties.local_name.unwrap_or_default();
            let lower = name.to_lowercase();
            // This service/name profile comes from the existing X1 owner. New models need a new matcher.
            if !lower.starts_with("ahakey") && !lower.starts_with("vibe code") {
                continue;
            }
            eprintln!("Found X1: {id} ({name})");
            return Ok(Some(AhaKey {
                device_id: id,
                name,
                hardware: Box::new(ModelX1::new(peripheral, adapter.clone())),
            }));
        }
    }
    Ok(None)
}
