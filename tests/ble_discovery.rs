//! Real CoreBluetooth regression: no connection or writes, even with a paired X1.
#![cfg(target_os = "macos")]

use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncBufReadExt, process::Command};

fn thread_count(pid: u32) -> usize {
    let output = std::process::Command::new("/bin/ps")
        .args(["-M", "-p", &pid.to_string()])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .count()
        .saturating_sub(1)
}

#[tokio::test]
#[ignore = "requires macOS Bluetooth permission; scans for about 55 seconds"]
async fn repeated_discovery_keeps_native_thread_count_bounded() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ahakey-runtime-rust"))
        .env_remove("AHAKEY_DEBUG_LOG")
        .args([
            "--debug",
            "--parent-stdin",
            "--device-id",
            "regression-no-device",
        ])
        .arg("--discovery")
        .arg(directory.path().join("discovery.json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let counts = tokio::time::timeout(Duration::from_secs(80), async {
        let mut counts = Vec::new();
        while counts.len() < 5 {
            let line = lines.next_line().await.unwrap().expect("Runtime exited");
            if line.contains("\"event\":\"ble.notFound\"") {
                counts.push(thread_count(pid));
            }
        }
        counts
    })
    .await
    .expect("discovery must continue completing scans");
    drop(child.stdin.take());
    assert!(
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    // Allow one lazily created system thread, but not one leaked thread per scan.
    assert!(
        counts[4] <= counts[0] + 1,
        "native threads grew with repeated scans: {counts:?}"
    );
    eprintln!("native threads across five scans: {counts:?}");
}
