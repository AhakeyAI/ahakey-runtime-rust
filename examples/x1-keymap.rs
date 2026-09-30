//! Narrow master-firmware key binding utility. Quit Studio/Runtime before using it.
use ahakey_runtime_rust::{
    ahakey::{hardware::Hardware, model_x1::ModelX1},
    discovery::{self, Discovery},
    protocol::x1::frame,
};
use btleplug::{
    api::{
        Central, Manager as _, Peripheral as _, RetrievePeripheralsOptions, ScanFilter,
        bleuuid::uuid_from_u16,
    },
    platform::Manager,
};
use std::{error::Error, time::Duration};

fn usages(keys: &str) -> Result<[u8; 4], &'static str> {
    if keys.len() != 4 || !keys.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err("--keys requires exactly four lowercase ASCII letters, e.g. dfjk");
    }
    Ok(std::array::from_fn(|i| keys.as_bytes()[i] - b'a' + 4))
}

fn commands(mode: u8, keys: [u8; 4]) -> Vec<Vec<u8>> {
    let mut result: Vec<_> = keys
        .into_iter()
        .enumerate()
        .map(|(index, usage)| frame(0x73, &[0x73, mode, index as u8, usage]))
        .collect();
    result.push(frame(0x04, &[]));
    result
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let mut selected = None;
    let mut keys = "dfjk".to_string();
    let mut mode = None;
    let mut apply = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--device-id" => selected = Some(args.next().ok_or("missing device ID")?),
            "--keys" => keys = args.next().ok_or("missing four letters")?,
            "--mode" => {
                let value: u8 = args.next().ok_or("missing mode 0..3")?.parse()?;
                if value > 3 {
                    return Err("mode must be 0..3".into());
                }
                mode = Some(value);
            }
            "--apply" => apply = true,
            "--help" => {
                println!(
                    "x1-keymap [--device-id x1-UUID] [--keys dfjk] [--mode 0..3] [--apply]\nDefault: inspect current mode and print planned frames.\nWriting requires explicit --device-id and --mode. Quit Studio/Runtime first."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    let keys_hid = usages(&keys)?;
    if apply && (selected.is_none() || mode.is_none()) {
        return Err("--apply requires --device-id and --mode from inspection".into());
    }
    // Use the Runtime's normal ownership lock, including a custom discovery path.
    let _ownership = Discovery::acquire(discovery::default_path()?)?;
    let manager = Manager::new().await?;
    let mut candidates = Vec::new();
    for adapter in manager.adapters().await? {
        let mut devices = adapter
            .retrieve_peripherals(RetrievePeripheralsOptions {
                identifiers: None,
                services: Some(vec![uuid_from_u16(0x7340)]),
            })
            .await?;
        adapter
            .start_scan(ScanFilter {
                services: vec![uuid_from_u16(0x7340)],
            })
            .await?;
        tokio::time::sleep(Duration::from_secs(5)).await;
        devices.extend(adapter.peripherals().await?);
        adapter.stop_scan().await?;
        devices.sort_by_key(|p| p.id().to_string());
        devices.dedup_by_key(|p| p.id());
        for peripheral in devices {
            let id = format!("x1-{}", peripheral.id());
            let name = peripheral
                .properties()
                .await?
                .and_then(|p| p.local_name)
                .unwrap_or_default();
            let lower = name.to_lowercase();
            if (!lower.starts_with("ahakey") && !lower.starts_with("vibe code"))
                || selected.as_ref().is_some_and(|wanted| wanted != &id)
            {
                continue;
            }
            println!("Found {id} ({name})");
            candidates.push((id, ModelX1::new(peripheral, adapter.clone())));
        }
    }
    if candidates.len() != 1 {
        return Err(format!(
            "expected one matching X1, found {}; select --device-id or connect the device",
            candidates.len()
        )
        .into());
    }
    let (id, mut hardware) = candidates.pop().unwrap();
    let state = hardware.connect().await?;
    let target = mode.unwrap_or(state.work_mode);
    println!(
        "Device {id}; current mode={} (UI mode {}); brightness={:?}; target mode={target}; keys={keys}",
        state.work_mode,
        state.work_mode + 1,
        state.brightness
    );
    let result: Result<(), Box<dyn Error>> = async {
        if apply && target != state.work_mode {
            return Err("device mode changed since inspection; inspect again before writing".into());
        }
        for (index, command) in commands(target, keys_hid).iter().enumerate() {
            println!("{} {}: {command:02x?}", if apply { "WRITE" } else { "PLAN" }, index + 1);
            if apply {
                if let Err(error) = hardware.write_confirmed(command).await {
                    return Err(format!("command {} failed: {error}; earlier writes may have taken effect; no automatic retry or rollback", index + 1).into());
                }
                println!("ACK {}", index + 1);
            }
        }
        if apply {
            println!("Four key writes and save acknowledged. Firmware has no binding readback; verify physical key output and persistence after power cycling.");
        }
        Ok(())
    }.await;
    let disconnected = hardware.disconnect().await;
    result?;
    disconnected?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dfjk_writes_only_four_bindings_in_selected_mode_then_saves() {
        let frames = commands(2, usages("dfjk").unwrap());
        assert_eq!(
            frames,
            vec![
                vec![0xaa, 0xbb, 0x73, 0x73, 2, 0, 0x07, 0xcc, 0xdd],
                vec![0xaa, 0xbb, 0x73, 0x73, 2, 1, 0x09, 0xcc, 0xdd],
                vec![0xaa, 0xbb, 0x73, 0x73, 2, 2, 0x0d, 0xcc, 0xdd],
                vec![0xaa, 0xbb, 0x73, 0x73, 2, 3, 0x0e, 0xcc, 0xdd],
                vec![0xaa, 0xbb, 0x04, 0xcc, 0xdd],
            ]
        );
        for invalid in ["", "dfj", "dfjkk", "DFJK", "éjk", "1234"] {
            assert!(usages(invalid).is_err());
        }
    }
}
