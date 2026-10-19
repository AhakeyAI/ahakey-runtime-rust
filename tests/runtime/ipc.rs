use super::*;

#[tokio::test]
async fn websocket_auth_save_disconnect_recovery_and_event_order() {
    let (directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ipc::serve(listener, runtime.clone(), "test-token".into()));
    let mut writer = Client::open(&endpoint).await;
    assert_eq!(
        writer.subscribe().await["error"]["data"]["code"],
        "PERMISSION_DENIED"
    );
    let hello = writer.hello().await;
    assert_eq!(hello["result"]["protocol"]["major"], 1);
    let mut observer = Client::open(&endpoint).await;
    observer.hello().await;
    let subscription = observer.subscribe().await;
    let snapshot_sequence = subscription["result"]["snapshot"]["sequence"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let document = writer
        .call("configuration.get", json!({"deviceId":"test-x1"}))
        .await;
    let original = document["result"]["configuration"].clone();
    let request = apply(
        document["result"]["baseToken"].as_str().unwrap(),
        "lost-acceptance",
    );
    writer.send("configuration.apply", json!(request)).await;
    // Close before reading the acceptance response. The operation must still finish.
    writer.socket.close(None).await.unwrap();
    drop(writer);
    let completed = terminal(&runtime, "lost-acceptance").await;
    assert_eq!(completed.status, "completed");
    let mut config_index = None;
    let mut completed_index = None;
    let mut last = snapshot_sequence;
    for index in 0..100 {
        let event = observer.receive().await;
        assert_eq!(
            event["params"]["subscriptionId"],
            subscription["result"]["subscriptionId"]
        );
        let sequence = event["params"]["sequence"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(sequence > last);
        last = sequence;
        if event["params"]["type"] == "configuration.changed" {
            config_index = Some(index);
        }
        if event["params"]["type"] == "operation.changed"
            && event["params"]["data"]["status"] == "completed"
        {
            completed_index = Some(index);
            break;
        }
    }
    assert!(config_index.unwrap() < completed_index.unwrap());
    let after = observer
        .call("configuration.get", json!({"deviceId":"test-x1"}))
        .await;
    assert_eq!(
        after["result"]["configuration"]["device"]["brightnessPercent"],
        60
    );
    assert_ne!(
        after["result"]["baseToken"],
        document["result"]["baseToken"]
    );
    let mut expected: Configuration = serde_json::from_value(original).unwrap();
    expected.merge(&request.changes);
    assert_eq!(after["result"]["configuration"], json!(expected));
    let recovered = observer
        .call("operation.get", json!({"operationId":"lost-acceptance"}))
        .await;
    assert_eq!(recovered["result"]["status"], "completed");
    assert_eq!(
        observer.call("configuration.apply", json!(request)).await["result"]["status"],
        "completed"
    );
    let mut conflict = request.clone();
    conflict.changes.device.brightness_percent = 61;
    assert_eq!(
        observer.call("configuration.apply", json!(conflict)).await["error"]["data"]["code"],
        "OPERATION_ID_CONFLICT"
    );
    let mut stale = request.clone();
    stale.operation_id = "stale".into();
    assert_eq!(
        observer.call("configuration.apply", json!(stale)).await["error"]["data"]["code"],
        "BASE_CONFLICT"
    );
    observer.socket.close(None).await.unwrap();
    server.abort();
    let _ = server.await;
    runtime.shutdown().await;
    drop(runtime);
    let restarted = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    assert_eq!(
        restarted.operation("lost-acceptance").await.unwrap().status,
        "completed"
    );
    assert_eq!(restarted.apply(request).await.unwrap().status, "completed");
    restarted.shutdown().await;
}

#[tokio::test]
async fn rejects_browser_origin_wrong_token_and_unknown_mutation_fields() {
    let (_directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ipc::serve(listener, runtime.clone(), "test-token".into()));
    let mut request = endpoint.clone().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", "http://localhost".parse().unwrap());
    assert!(connect_async(request).await.is_err());
    let mut client = Client::open(&endpoint).await;
    let bad=client.call("runtime.hello",json!({"protocol":{"major":1,"minor":0},"client":{"name":"test","version":"1","kind":"studio"},"token":"bad"})).await;
    assert_eq!(bad["error"]["data"]["code"], "PERMISSION_DENIED");
    assert!(client.subscribe().await.get("result").is_none());
    client.hello().await;
    let response = client
        .call("device.connect", json!({"deviceId":"test-x1","extra":true}))
        .await;
    assert_eq!(response["error"]["code"], -32602);
    let response = client.call("made.up", json!({})).await;
    assert_eq!(response["error"]["code"], -32601);
    client
        .socket
        .send(Message::Text("[]".into()))
        .await
        .unwrap();
    assert_eq!(client.receive().await["error"]["code"], -32600);
    client.socket.close(None).await.unwrap();
    server.abort();
    let _ = server.await;
    runtime.shutdown().await;
}
