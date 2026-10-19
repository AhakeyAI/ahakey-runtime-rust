use super::*;

struct OledFault {
    inner: MockX1,
    commit_before_timeout: bool,
    failures: usize,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Hardware for OledFault {
    fn configuration_commands(&self, configuration: &Configuration) -> Result<Vec<Vec<u8>>> {
        self.inner.configuration_commands(configuration)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        self.inner.connect().await
    }
    async fn disconnect(&mut self) -> Result<()> {
        self.inner.disconnect().await
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        self.inner.refresh().await
    }
    async fn write_confirmed(&mut self, command: &[u8]) -> Result<()> {
        self.inner.write_confirmed(command).await
    }
    async fn oled_state(&mut self) -> Result<ahakey_runtime_rust::ahakey::oled::State> {
        self.inner.oled_state().await
    }
    async fn upload_oled(
        &mut self,
        target: &ahakey_runtime_rust::oled::Target,
        asset: &ahakey_runtime_rust::oled::Prepared,
        pause: tokio::sync::watch::Receiver<bool>,
        progress: tokio::sync::mpsc::Sender<f64>,
    ) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let fail = self.failures > 0;
        self.failures = self.failures.saturating_sub(1);
        if self.commit_before_timeout || !fail {
            self.inner
                .upload_oled(target, asset, pause, progress)
                .await?;
        }
        if fail {
            Err(Error("DEVICE_TIMEOUT"))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn websocket_binary_resource_to_oled_and_idempotent_operation() {
    let (_directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ipc::serve(listener, runtime.clone(), "test-token".into()));
    let mut client = Client::open(&endpoint).await;
    client.hello().await;
    client.subscribe().await;
    let source = oled_gif();
    let begin = client
        .call(
            "resource.begin",
            json!({"byteLength":source.len(),"sha256":ahakey_runtime_rust::oled::sha256(&source)}),
        )
        .await;
    let upload_id = begin["result"]["uploadId"].as_str().unwrap();
    let id = uuid::Uuid::parse_str(upload_id).unwrap();
    // A rejected offset does not append bytes; the correct chunk can follow.
    let mut binary = id.as_bytes().to_vec();
    binary.extend_from_slice(&1u32.to_le_bytes());
    binary.extend_from_slice(&source[..ahakey_runtime_rust::oled::MAX_CHUNK_BYTES]);
    client
        .socket
        .send(Message::Binary(binary.clone().into()))
        .await
        .unwrap();
    loop {
        let event = client.receive().await;
        if event["method"] == "resource.error" {
            assert_eq!(event["params"]["code"], "INVALID_RESOURCE_CHUNK");
            break;
        }
    }
    let chunk_size = ahakey_runtime_rust::oled::MAX_CHUNK_BYTES;
    for (index, chunk) in source.chunks(chunk_size).enumerate() {
        let mut binary = id.as_bytes().to_vec();
        binary.extend_from_slice(&((index * chunk_size) as u32).to_le_bytes());
        binary.extend_from_slice(chunk);
        client
            .socket
            .send(Message::Binary(binary.into()))
            .await
            .unwrap();
        loop {
            let event = client.receive().await;
            if event["method"] == "resource.progress" {
                assert_eq!(
                    event["params"]["receivedBytes"],
                    index * chunk_size + chunk.len()
                );
                break;
            }
        }
    }
    let finished = client.call("resource.finish", json!({"uploadId":id})).await;
    assert_eq!(finished["result"]["frameCount"], 6);
    assert_eq!(finished["result"]["byteLength"], 6 * 28_672);
    let baseline = runtime.configuration("test-x1").await.unwrap();
    let request = json!({"deviceId":"test-x1","operationId":"oled-first","baseToken":baseline.base_token,
        "resourceId":finished["result"]["resourceId"],"target":{"kind":"modeDefault","mode":2}});
    let accepted = client.call("oled.upload", request.clone()).await;
    assert_eq!(accepted["result"]["operationId"], "oled-first");
    assert_eq!(terminal(&runtime, "oled-first").await.status, "completed");
    let device = client.call("oled.get", json!({"deviceId":"test-x1"})).await;
    assert_eq!(device["result"]["profile"], "x1-cp2-resource-v1");
    assert_eq!(device["result"]["entries"][8]["valid"], true);
    assert_eq!(
        device["result"]["entries"][8]["crc32"],
        finished["result"]["crc32"]
    );
    assert!(!device["result"]["entries"][0]["valid"].as_bool().unwrap());
    assert_eq!(
        runtime
            .configuration("test-x1")
            .await
            .unwrap()
            .configuration,
        baseline.configuration
    );
    assert_ne!(
        runtime.configuration("test-x1").await.unwrap().base_token,
        baseline.base_token
    );
    assert_eq!(
        client.call("oled.upload", request).await["result"]["status"],
        "completed"
    );
    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn oled_pause_keeps_resource_across_restart_and_resumes_explicitly() {
    let (directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let source = oled_png();
    let resource = runtime
        .resources
        .finish(source.clone(), ahakey_runtime_rust::oled::sha256(&source))
        .await
        .unwrap();
    runtime
        .upload_oled(ahakey_runtime_rust::oled::Upload {
            operation_id: "paused-oled".into(),
            device_id: "test-x1".into(),
            base_token: runtime.configuration("test-x1").await.unwrap().base_token,
            resource_id: resource.resource_id,
            target: ahakey_runtime_rust::oled::Target::ModeDefault { mode: 0 },
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime.operation("paused-oled").await.unwrap().progress == 0.0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    runtime.pause_operation("paused-oled").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if runtime.operation("paused-oled").await.unwrap().status == "paused" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    runtime.shutdown().await;
    drop(runtime);
    let runtime = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    assert_eq!(
        runtime.operation("paused-oled").await.unwrap().status,
        "paused"
    );
    runtime
        .register(
            AhaKey {
                device_id: "test-x1".into(),
                name: "Test X1".into(),
                hardware: Box::<MockX1>::default(),
            },
            None,
            true,
        )
        .await
        .unwrap();
    ready(&runtime).await;
    assert!(!runtime.oled_state("test-x1").await.unwrap().entries[0].valid);
    runtime.resume_operation("paused-oled").await.unwrap();
    assert_eq!(terminal(&runtime, "paused-oled").await.status, "completed");
    assert!(runtime.oled_state("test-x1").await.unwrap().entries[0].valid);
    runtime.shutdown().await;
}

#[tokio::test]
async fn oled_lost_commit_ack_is_read_back_and_retries_stop_at_three() {
    for committed in [true, false] {
        let calls = Arc::new(AtomicUsize::new(0));
        let (_directory, runtime) = setup(
            Box::new(OledFault {
                inner: MockX1::default(),
                commit_before_timeout: committed,
                failures: if committed { 1 } else { 3 },
                calls: calls.clone(),
            }),
            true,
            None,
        )
        .await;
        let source = oled_png();
        let resource = runtime
            .resources
            .finish(source.clone(), ahakey_runtime_rust::oled::sha256(&source))
            .await
            .unwrap();
        runtime
            .upload_oled(ahakey_runtime_rust::oled::Upload {
                operation_id: "uncertain-oled".into(),
                device_id: "test-x1".into(),
                base_token: runtime.configuration("test-x1").await.unwrap().base_token,
                resource_id: resource.resource_id,
                target: ahakey_runtime_rust::oled::Target::ModeDefault { mode: 0 },
            })
            .await
            .unwrap();
        let operation = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let operation = runtime.operation("uncertain-oled").await.unwrap();
                if operation.terminal() || operation.status == "paused" {
                    break operation;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        if committed {
            assert_eq!(operation.status, "completed");
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(operation.status, "paused");
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            ready(&runtime).await;
            runtime.resume_operation("uncertain-oled").await.unwrap();
            assert_eq!(
                terminal(&runtime, "uncertain-oled").await.status,
                "completed"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 4);
        }
        runtime.shutdown().await;
    }
}

#[tokio::test]
async fn unsupported_oled_does_not_write_or_block_legacy_keymap() {
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
    let source = oled_png();
    let asset = runtime
        .resources
        .finish(source.clone(), ahakey_runtime_rust::oled::sha256(&source))
        .await
        .unwrap();
    runtime
        .upload_oled(ahakey_runtime_rust::oled::Upload {
            operation_id: "old-oled".into(),
            device_id: "test-x1".into(),
            base_token: runtime.configuration("test-x1").await.unwrap().base_token,
            resource_id: asset.resource_id,
            target: ahakey_runtime_rust::oled::Target::ModeDefault { mode: 0 },
        })
        .await
        .unwrap();
    let result = terminal(&runtime, "old-oled").await;
    assert_eq!(result.status, "failed");
    assert_eq!(result.effect, "none");
    assert_eq!(result.error_code.as_deref(), Some("UNSUPPORTED_CAPABILITY"));
    assert!(writes.lock().unwrap().is_empty());
    ready(&runtime).await;
    let token = runtime.configuration("test-x1").await.unwrap().base_token;
    runtime
        .apply_keymap(keys_request(&token, "old-keys"))
        .await
        .unwrap();
    assert_eq!(terminal(&runtime, "old-keys").await.status, "completed");
    runtime.shutdown().await;
}
