use ahakey_runtime_rust::{
    ahakey::{device::AhaKey, hardware::Hardware, state::HardwareState},
    configuration::Configuration,
    error::{Error, Result},
    hooks::{HookEvent, Preferences},
    input::{InputDevice, InputMessage, InputState, KeyEvent},
    runtime::Runtime,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

struct Lever {
    automatic: Arc<AtomicBool>,
    fail: Arc<AtomicBool>,
    reads: Arc<AtomicUsize>,
}
#[async_trait]
impl Hardware for Lever {
    fn configuration_commands(&self, _: &Configuration) -> Result<Vec<Vec<u8>>> {
        Err(Error("UNSUPPORTED_CAPABILITY"))
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        self.refresh().await
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(Error("DEVICE_TIMEOUT"));
        }
        Ok(HardwareState {
            work_mode: 0,
            light_mode: 0,
            battery: Some(80),
            brightness: Some(50),
            lever: Some(if self.automatic.load(Ordering::SeqCst) {
                "automatic"
            } else {
                "manual"
            }),
        })
    }
    async fn write_confirmed(&mut self, _: &[u8]) -> Result<()> {
        panic!("Hook must never write firmware configuration")
    }
}
fn event(tool: &str, name: &str) -> HookEvent {
    HookEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        tool: tool.into(),
        session_id: "test-session".into(),
        event: name.into(),
        cwd: "/project".into(),
        tool_name: "Bash".into(),
        summary: "echo harmless".into(),
    }
}
async fn setup() -> (
    tempfile::TempDir,
    Arc<Runtime>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    Arc<AtomicUsize>,
) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(dir.path().join("state.json")).await.unwrap();
    let automatic = Arc::new(AtomicBool::new(false));
    let fail = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    runtime
        .register(
            AhaKey {
                device_id: "x1".into(),
                name: "Test X1".into(),
                hardware: Box::new(Lever {
                    automatic: automatic.clone(),
                    fail: fail.clone(),
                    reads: reads.clone(),
                }),
            },
            None,
            false,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if runtime.subscribe().await.0["devices"][0]["connectionState"] == "ready" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    runtime
        .configure_hooks(Preferences {
            enabled: true,
            device_id: Some("x1".into()),
            allow_automatic: true,
            ..Default::default()
        })
        .await
        .unwrap();
    (dir, runtime, automatic, fail, reads)
}
#[tokio::test]
async fn automatic_approval_requires_a_new_successful_physical_read() {
    let (_dir, runtime, automatic, fail, reads) = setup().await;
    let pending = runtime
        .hook_event(event("claude", "PermissionRequest"))
        .await
        .unwrap();
    let id = pending["requestId"].as_str().unwrap();
    let before = reads.load(Ordering::SeqCst);
    assert_eq!(runtime.hook_poll(id).await.unwrap()["decision"], "pending");
    assert!(reads.load(Ordering::SeqCst) > before);
    automatic.store(true, Ordering::SeqCst);
    assert_eq!(runtime.hook_poll(id).await.unwrap()["decision"], "allow");
    let second = runtime
        .hook_event(event("codex", "PermissionRequest"))
        .await
        .unwrap();
    fail.store(true, Ordering::SeqCst);
    assert_eq!(
        runtime
            .hook_poll(second["requestId"].as_str().unwrap())
            .await
            .unwrap()["decision"],
        "defer"
    );
    assert_eq!(runtime.hooks().await["preferences"]["enabled"], false);
    runtime.shutdown().await;
}
#[tokio::test]
async fn hardware_confirmation_needs_an_unambiguous_pending_request() {
    let (_dir, runtime, _, _, _) = setup().await;
    runtime
        .input_message(InputMessage::State(InputState {
            status: "listening".into(),
            permission: "granted".into(),
            devices: vec![InputDevice {
                input_device_id: "hid-1".into(),
                name: "AhaKey".into(),
                pressed_usages: vec![],
            }],
        }))
        .await;
    runtime
        .configure_hooks(Preferences {
            enabled: true,
            device_id: Some("x1".into()),
            input_device_id: Some("hid-1".into()),
            approve_usage: Some(28),
            reject_usage: Some(17),
            ..Default::default()
        })
        .await
        .unwrap();
    let a = runtime
        .hook_event(event("claude", "PermissionRequest"))
        .await
        .unwrap();
    let b = runtime
        .hook_event(event("cursor", "beforeShellExecution"))
        .await
        .unwrap();
    let key = |pressed| {
        InputMessage::Key(KeyEvent {
            input_device_id: "hid-1".into(),
            name: "AhaKey".into(),
            usage: 28,
            pressed,
            timestamp_ms: "1".into(),
            held_ms: None,
        })
    };
    runtime.input_message(key(true)).await;
    assert_eq!(
        runtime
            .hook_poll(a["requestId"].as_str().unwrap())
            .await
            .unwrap()["decision"],
        "pending"
    );
    runtime.input_message(key(false)).await;
    runtime
        .hook_select(Some(b["requestId"].as_str().unwrap().into()), None)
        .await
        .unwrap();
    runtime.input_message(key(true)).await;
    assert_eq!(
        runtime
            .hook_poll(b["requestId"].as_str().unwrap())
            .await
            .unwrap()["decision"],
        "allow"
    );
    assert_eq!(
        runtime
            .hook_poll(a["requestId"].as_str().unwrap())
            .await
            .unwrap()["decision"],
        "pending"
    );
    runtime.shutdown().await;
}
#[tokio::test]
async fn real_hook_binary_returns_the_decision_from_authenticated_runtime() {
    use tokio::io::AsyncWriteExt;
    let (dir, runtime, _, _, _) = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ahakey_runtime_rust::ipc::serve(
        listener,
        runtime.clone(),
        "test-token".into(),
    ));
    let discovery = dir.path().join("discovery.json");
    ahakey_runtime_rust::storage::atomic_write(
        &discovery,
        json!({"endpoint":endpoint,"token":"test-token"})
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    for tool in ["claude", "codex", "cursor"] {
        let event_name = if tool == "cursor" {
            "beforeShellExecution"
        } else {
            "PermissionRequest"
        };
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ahakey-hook"))
            .args(["--tool", tool, "--event", event_name, "--discovery"])
            .arg(&discovery)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(json!({"session_id":"stdio-test","cwd":"/project","tool_name":"Bash","tool_input":{"command":"echo test"}}).to_string().as_bytes()).await.unwrap();
        drop(stdin);
        let id = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let view = runtime.hooks().await;
                if let Some(request) = view["approvals"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["decision"] == "pending" && r["context"]["tool"] == tool)
                {
                    break request["requestId"].as_str().unwrap().to_owned();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        runtime.hook_decide(&id, "deny").await.unwrap();
        let output = tokio::time::timeout(Duration::from_secs(4), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(output.status.success());
        let actual: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            actual,
            ahakey_runtime_rust::hooks::hook_output(tool, event_name, "deny")
        );
    }
    server.abort();
    runtime.shutdown().await;
}
