use ahakey_runtime_rust::{
    input::{InputDevice, InputMessage, InputState, KeyEvent},
    runtime::Runtime,
};

#[tokio::test]
async fn input_snapshot_recovers_held_keys_and_permission_loss_clears_them() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(directory.path().join("journal.json"))
        .await
        .unwrap();
    let (_, mut events) = runtime.subscribe().await;
    runtime
        .input_message(InputMessage::State(InputState {
            status: "listening".into(),
            permission: "granted".into(),
            devices: vec![InputDevice {
                input_device_id: "hid-123".into(),
                name: "AhaKey Test".into(),
                pressed_usages: vec![],
            }],
        }))
        .await;
    assert_eq!(events.recv().await.unwrap().kind, "input.changed");
    runtime
        .input_message(InputMessage::Key(KeyEvent {
            input_device_id: "hid-123".into(),
            name: "AhaKey Test".into(),
            usage: 109,
            pressed: true,
            timestamp_ms: "9007199254740993".into(),
            held_ms: None,
        }))
        .await;
    let key = events.recv().await.unwrap();
    assert_eq!(key.kind, "input.key");
    assert_eq!(key.data["timestampMs"], "9007199254740993");
    let (snapshot, _) = runtime.subscribe().await;
    assert_eq!(
        snapshot["input"]["devices"][0]["pressedUsages"],
        serde_json::json!([109])
    );
    assert_eq!(snapshot["sequence"], key.sequence.to_string());

    runtime
        .input_message(InputMessage::State(InputState {
            status: "permissionDenied".into(),
            permission: "denied".into(),
            devices: vec![],
        }))
        .await;
    let (snapshot, _) = runtime.subscribe().await;
    assert!(snapshot["input"]["devices"].as_array().unwrap().is_empty());
    assert_eq!(snapshot["input"]["permission"], "denied");
    // Input events and pressed states are session-only, never stored in the journal.
    let journal = std::fs::read_to_string(directory.path().join("journal.json")).unwrap();
    assert!(!journal.contains("hid-123"));
    assert!(!journal.contains("timestampMs"));
    runtime.shutdown().await;
}

#[tokio::test]
async fn missing_native_helper_reports_error_without_affecting_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(directory.path().join("journal.json"))
        .await
        .unwrap();
    ahakey_runtime_rust::input::monitor(runtime.clone(), directory.path().join("missing-helper"))
        .await;
    let (snapshot, _) = runtime.subscribe().await;
    assert_eq!(snapshot["input"]["status"], "error");
    assert!(snapshot["input"]["devices"].as_array().unwrap().is_empty());
    runtime.shutdown().await;
}
