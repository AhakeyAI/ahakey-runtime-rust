use super::*;

#[tokio::test]
async fn keymap_ipc_works_without_full_configuration_and_survives_restart() {
    let (directory, runtime) = setup(Box::<MockX1>::default(), false, None).await;
    let document = runtime.configuration("test-x1").await.unwrap();
    assert!(document.configuration.is_none());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ipc::serve(listener, runtime.clone(), "test-token".into()));
    let mut client = Client::open(&endpoint).await;
    client.hello().await;
    let request = json!({"operationId":"keys-only", "deviceId":"test-x1",
        "baseToken":document.base_token, "mode":2,
        "keys":[{"index":0,"action":{"type":"shortcut","modifiers":[],"keyCode":7}}]});
    let accepted = client.call("keymap.apply", request.clone()).await;
    assert_eq!(accepted["result"]["status"], "accepted", "{accepted}");
    assert_eq!(terminal(&runtime, "keys-only").await.status, "completed");
    let after = runtime.configuration("test-x1").await.unwrap();
    assert!(
        after.configuration.is_none(),
        "a key write must not invent lights/brightness"
    );
    assert_ne!(after.base_token, document.base_token);
    assert_eq!(
        client.call("keymap.apply", request.clone()).await["result"]["status"],
        "completed"
    );
    let mut conflicting = request.clone();
    conflicting["keys"][0]["action"]["keyCode"] = json!(9);
    assert_eq!(
        client.call("keymap.apply", conflicting).await["error"]["data"]["code"],
        "OPERATION_ID_CONFLICT"
    );
    let mut stale = request.clone();
    stale["operationId"] = json!("stale-keys");
    assert_eq!(
        client.call("keymap.apply", stale).await["error"]["data"]["code"],
        "BASE_CONFLICT"
    );
    client.socket.close(None).await.unwrap();
    server.abort();
    runtime.shutdown().await;
    let restarted = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    assert_eq!(
        restarted.operation("keys-only").await.unwrap().status,
        "completed"
    );
    restarted.shutdown().await;
}

#[tokio::test]
async fn keymap_without_lighting_writes_only_requested_keys_then_save_and_reserves_worker() {
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
    let request = keys_request(&token, "dfjk");
    runtime.apply_keymap(request.clone()).await.unwrap();
    assert_eq!(
        runtime
            .apply_keymap(keys_request(&token, "concurrent"))
            .await
            .unwrap_err()
            .0,
        "BUSY"
    );
    assert_eq!(
        runtime
            .apply(apply(&token, "config-concurrent"))
            .await
            .unwrap_err()
            .0,
        "BUSY"
    );
    assert_eq!(
        runtime.connect("test-x1", false).await.unwrap_err().0,
        "BUSY"
    );
    assert_eq!(terminal(&runtime, "dfjk").await.status, "completed");
    let mut expected = vec![];
    for (index, code) in [7, 9, 13, 14].into_iter().enumerate() {
        expected.push(vec![0xaa, 0xbb, 0x73, 0x74, 2, index as u8, 0xcc, 0xdd]);
        expected.push(vec![
            0xaa,
            0xbb,
            0x73,
            0x73,
            2,
            index as u8,
            code,
            0xcc,
            0xdd,
        ]);
    }
    expected.push(vec![0xaa, 0xbb, 4, 0xcc, 0xdd]);
    assert_eq!(*writes.lock().unwrap(), expected);
    let mut reordered = request;
    reordered.keys.reverse();
    assert_eq!(
        runtime.apply_keymap(reordered).await.unwrap().status,
        "completed"
    );
    assert_eq!(writes.lock().unwrap().len(), 9);
    runtime.shutdown().await;
}

#[tokio::test]
async fn keymap_failure_is_durable_unknown_and_never_replayed() {
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
    let token = runtime.configuration("test-x1").await.unwrap().base_token;
    let request = keys_request(&token, "key-failure");
    runtime.apply_keymap(request.clone()).await.unwrap();
    let result = terminal(&runtime, "key-failure").await;
    assert_eq!(result.status, "failed");
    assert_eq!(result.effect, "unknown");
    assert_eq!(result.error_code.as_deref(), Some("DEVICE_TIMEOUT"));
    assert!(result.progress > 0.0);
    runtime.shutdown().await;
    let recovered = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    assert_eq!(
        recovered.apply_keymap(request).await.unwrap().status,
        "failed"
    );
    assert_eq!(writes.lock().unwrap().len(), 2);
    recovered.shutdown().await;
}

#[tokio::test]
async fn keymap_preserves_known_unrelated_configuration() {
    let (_directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let before = runtime.configuration("test-x1").await.unwrap();
    let mut request = keys_request(&before.base_token, "one-key");
    request.keys.truncate(1);
    let mut expected = before.configuration.unwrap();
    let mode = expected.modes.iter_mut().find(|m| m.mode == 2).unwrap();
    mode.keys
        .iter_mut()
        .find(|k| k.role == "voice")
        .unwrap()
        .action = request.keys[0].action.clone();
    runtime.apply_keymap(request).await.unwrap();
    assert_eq!(terminal(&runtime, "one-key").await.status, "completed");
    assert_eq!(
        runtime
            .configuration("test-x1")
            .await
            .unwrap()
            .configuration,
        Some(expected)
    );
    runtime.shutdown().await;
}

#[test]
fn keymap_rejects_invalid_scope_actions_and_extra_fields() {
    let request = keys_request("base", "id");
    for bad in [
        json!({"mode":4}),
        json!({"keys":[]}),
        json!({"operationId":""}),
        json!({"baseToken":""}),
        json!({"keys":[{"index":4,"action":{"type":"disabled"}}]}),
        json!({"keys":[{"index":0,"action":{"type":"shortcut","modifiers":["bad"],"keyCode":7}}]}),
        json!({"keys":[{"index":0,"action":{"type":"macro","steps":[{"type":"delay","delayMs":1}]}}]}),
    ] {
        let mut value = json!(request);
        for (k, v) in bad.as_object().unwrap() {
            value[k] = v.clone();
        }
        let parsed: ahakey_runtime_rust::keymap::KeymapApply =
            serde_json::from_value(value).unwrap();
        assert_eq!(parsed.validate().unwrap_err().0, "INVALID_PARAMS");
    }
    let mut duplicate = request.clone();
    duplicate.keys[1].index = 0;
    assert!(duplicate.validate().is_err());
    let mut value = json!(request);
    value["brightnessPercent"] = json!(50);
    assert!(serde_json::from_value::<ahakey_runtime_rust::keymap::KeymapApply>(value).is_err());
}
