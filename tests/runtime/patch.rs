use super::*;
use ahakey_runtime_rust::configuration::patch::ConfigurationPatch;

fn patch(token: &str, id: &str) -> ConfigurationPatch {
    serde_json::from_value(
        json!({"operationId":id,"deviceId":"test-x1","baseToken":token,
        "brightnessPercent":57,"descriptions":[{"mode":2,"index":1,"description":"Approve"}]}),
    )
    .unwrap()
}
#[tokio::test]
async fn patch_ipc_preserves_unrelated_fields_and_survives_restart() {
    let (directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let before = runtime.configuration("test-x1").await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ipc::serve(listener, runtime.clone(), "test-token".into()));
    let mut client = Client::open(&endpoint).await;
    let hello = client.hello().await;
    assert!(
        hello["result"]["methods"]
            .as_array()
            .unwrap()
            .contains(&json!("configuration.patch"))
    );
    let request = patch(&before.base_token, "fields-only");
    let mut expected = before.configuration.unwrap();
    expected.device.brightness_percent = 57;
    expected
        .modes
        .iter_mut()
        .find(|m| m.mode == 2)
        .unwrap()
        .keys
        .iter_mut()
        .find(|k| k.role == "approve")
        .unwrap()
        .description = "Approve".into();
    assert_eq!(
        client.call("configuration.patch", json!(request)).await["result"]["status"],
        "accepted"
    );
    assert_eq!(terminal(&runtime, "fields-only").await.status, "completed");
    assert_eq!(
        runtime
            .configuration("test-x1")
            .await
            .unwrap()
            .configuration,
        Some(expected)
    );
    assert_eq!(
        client.call("configuration.patch", json!(request)).await["result"]["status"],
        "completed"
    );
    let mut changed = request.clone();
    changed.brightness_percent = Some(58);
    assert_eq!(
        client.call("configuration.patch", json!(changed)).await["error"]["data"]["code"],
        "OPERATION_ID_CONFLICT"
    );
    changed.operation_id = "stale-field".into();
    assert_eq!(
        client.call("configuration.patch", json!(changed)).await["error"]["data"]["code"],
        "BASE_CONFLICT"
    );
    client.socket.close(None).await.unwrap();
    server.abort();
    runtime.shutdown().await;
    let recovered = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    assert_eq!(
        recovered.patch_configuration(request).await.unwrap().status,
        "completed"
    );
    recovered.shutdown().await;
}
#[tokio::test]
async fn patch_without_baseline_sends_only_selected_fields_and_save() {
    let writes = Arc::new(std::sync::Mutex::new(vec![]));
    let (_directory, runtime) = setup(
        Box::new(KeyOnlyHardware {
            writes: writes.clone(),
            fail_at: None,
        }),
        false,
        None,
    )
    .await;
    let token = runtime.configuration("test-x1").await.unwrap().base_token;
    let request = patch(&token, "partial-fields");
    runtime.patch_configuration(request.clone()).await.unwrap();
    assert_eq!(
        runtime
            .patch_configuration(patch(&token, "concurrent"))
            .await
            .unwrap_err()
            .0,
        "BUSY"
    );
    assert_eq!(
        terminal(&runtime, "partial-fields").await.status,
        "completed"
    );
    assert_eq!(
        *writes.lock().unwrap(),
        vec![
            vec![0xaa, 0xbb, 0x85, 57, 0xcc, 0xdd],
            vec![
                0xaa, 0xbb, 0x73, 0x75, 2, 1, b'A', b'p', b'p', b'r', b'o', b'v', b'e', 0xcc, 0xdd
            ],
            vec![0xaa, 0xbb, 4, 0xcc, 0xdd],
        ]
    );
    assert!(
        runtime
            .configuration("test-x1")
            .await
            .unwrap()
            .configuration
            .is_none()
    );
    runtime.shutdown().await;
}
#[tokio::test]
async fn field_failure_remains_unknown_and_does_not_replay_after_restart() {
    let writes = Arc::new(std::sync::Mutex::new(vec![]));
    let (directory, runtime) = setup(
        Box::new(KeyOnlyHardware {
            writes: writes.clone(),
            fail_at: Some(2),
        }),
        false,
        None,
    )
    .await;
    let request = patch(
        &runtime.configuration("test-x1").await.unwrap().base_token,
        "lost-field-ack",
    );
    runtime.patch_configuration(request.clone()).await.unwrap();
    let failed = terminal(&runtime, "lost-field-ack").await;
    assert_eq!(failed.effect, "unknown");
    assert_eq!(failed.status, "failed");
    runtime.shutdown().await;
    let recovered = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    assert_eq!(
        recovered.patch_configuration(request).await.unwrap().status,
        "failed"
    );
    assert_eq!(writes.lock().unwrap().len(), 2);
    recovered.shutdown().await;
}
#[test]
fn patch_validation_rejects_empty_duplicate_out_of_range_and_unrelated_changes() {
    let request = patch("base", "id");
    for value in [
        json!({}),
        json!({"brightnessPercent":0}),
        json!({"brightnessPercent":101}),
        json!({"descriptions":[{"mode":4,"index":0,"description":"x"}]}),
        json!({"descriptions":[{"mode":0,"index":4,"description":"x"}]}),
        json!({"descriptions":[{"mode":0,"index":0,"description":"中文"}]}),
        json!({"descriptions":[{"mode":0,"index":0,"description":"123456789012345678901"}]}),
        json!({"descriptions":[{"mode":0,"index":0,"description":"a"},{"mode":0,"index":0,"description":"b"}]}),
    ] {
        let mut data = json!({"operationId":"id","deviceId":"test-x1","baseToken":"base"});
        for (key, value) in value.as_object().unwrap() {
            data[key] = value.clone();
        }
        let parsed: ConfigurationPatch = serde_json::from_value(data).unwrap();
        assert!(parsed.validate().is_err());
    }
    for field in ["lights", "keys", "changes"] {
        let mut value = json!(request);
        value[field] = json!([]);
        assert!(serde_json::from_value::<ConfigurationPatch>(value).is_err());
    }
    let mut clear = request;
    clear.brightness_percent = None;
    clear.descriptions[0].description.clear();
    assert!(clear.validate().is_ok());
    let commands = ahakey_runtime_rust::protocol::x1::patch_commands(&clear).unwrap();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0], vec![0xaa, 0xbb, 0x73, 0x75, 2, 1, 0xcc, 0xdd]);
}
