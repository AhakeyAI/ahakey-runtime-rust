use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::process::Command;
use tokio_tungstenite::{connect_async, tungstenite::Message};

async fn run(debug: bool, file: bool, env: Option<&str>) -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let discovery = dir.path().join("discovery.json");
    let log = dir.path().join("debug.jsonl");
    let mut command = Command::new(env!("CARGO_BIN_EXE_ahakey-runtime-rust"));
    command
        .env_remove("AHAKEY_DEBUG")
        .env_remove("AHAKEY_DEBUG_LOG")
        .args(["--mock", "--parent-stdin", "--discovery"])
        .arg(&discovery)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .kill_on_drop(true);
    if let Some(value) = env {
        command.env("AHAKEY_DEBUG", value);
    }
    if debug {
        command.arg("--debug");
    }
    if file {
        command.arg("--debug-log").arg(&log);
    }
    let mut child = command.spawn().unwrap();
    let info: Value = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(bytes) = tokio::fs::read(&discovery).await {
                break serde_json::from_slice(&bytes).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (mut socket, _) = connect_async(info["endpoint"].as_str().unwrap())
        .await
        .unwrap();
    socket
        .send(Message::Text(
            json!({
                "jsonrpc":"2.0", "id":"secret-request-id", "method":"runtime.hello",
                "params":{"protocol":{"major":1,"minor":0}, "token":info["token"],
                    "client":{"name":"secret-client-name", "version":"1", "kind":"test"}}
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(response.to_text().unwrap().contains("runtimeVersion"));
    drop(socket);
    drop(child.stdin.take());
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    let trace = if file {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::read_to_string(log).unwrap()
    } else {
        stderr.clone()
    };
    assert!(!trace.contains(info["token"].as_str().unwrap()));
    assert!(!trace.contains("secret-request-id"));
    assert!(!trace.contains("secret-client-name"));
    (trace, stderr)
}

#[tokio::test]
async fn diagnostics_follow_build_default_and_do_not_log_ipc_secrets() {
    let (default, _) = run(false, false, None).await;
    assert_eq!(default.contains("\"event\""), cfg!(debug_assertions));
    let (quiet, _) = run(false, false, Some("0")).await;
    assert!(!quiet.contains("\"event\""));
    for (flag, file, env) in [
        (true, false, Some("0")),
        (false, true, Some("0")),
        (false, false, Some("1")),
    ] {
        let (trace, stderr) = run(flag, file, env).await;
        let records: Vec<Value> = trace
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        for expected in ["runtime.start", "ipc.request", "runtime.stop"] {
            assert!(
                records.iter().any(|r| r["event"] == expected),
                "missing {expected}: {trace}"
            );
        }
        if file {
            assert_eq!(records.len(), trace.lines().count());
            assert!(!stderr.contains("\"event\""));
        }
    }
}

#[tokio::test]
async fn refuses_to_overwrite_an_existing_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing.json");
    std::fs::write(&path, "preserve me").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ahakey-runtime-rust"))
        .args(["--mock", "--debug-log"])
        .arg(&path)
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "preserve me");
}
