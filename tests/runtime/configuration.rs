use super::*;

struct FailingHardware {
    writes: Arc<AtomicUsize>,
}
#[async_trait]
impl Hardware for FailingHardware {
    fn configuration_commands(&self, configuration: &Configuration) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::configuration_commands(configuration)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        self.refresh().await
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        Ok(HardwareState {
            work_mode: 0,
            light_mode: 0,
            battery: Some(50),
            lever: Some("manual"),
            brightness: Some(35),
        })
    }
    async fn write_confirmed(&mut self, _: &[u8]) -> Result<()> {
        if self.writes.fetch_add(1, Ordering::SeqCst) == 1 {
            Err(Error("DEVICE_TIMEOUT"))
        } else {
            Ok(())
        }
    }
}

struct DelayedNotification {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    writes: Arc<AtomicUsize>,
    fail: bool,
}
#[async_trait]
impl Hardware for DelayedNotification {
    fn configuration_commands(&self, configuration: &Configuration) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::configuration_commands(configuration)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        Ok(HardwareState {
            work_mode: 0,
            light_mode: 0,
            battery: Some(82),
            lever: Some("manual"),
            brightness: Some(35),
        })
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        self.connect().await
    }
    async fn next_event(&mut self) -> Result<HardwareState> {
        self.started.notify_one();
        self.release.notified().await;
        if self.fail {
            Err(Error("DEVICE_TIMEOUT"))
        } else {
            let mut state = self.connect().await?;
            state.brightness = Some(99);
            Ok(state)
        }
    }
    async fn write_confirmed(&mut self, _: &[u8]) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn concurrent_admission_is_atomic_and_canonical_retries_are_idempotent() {
    let (_directory, runtime) = setup(Box::<MockX1>::default(), true, None).await;
    let token = runtime.configuration("test-x1").await.unwrap().base_token;
    let one = apply(&token, "one");
    let two = apply(&token, "two");
    let (a, b) = tokio::join!(runtime.apply(one.clone()), runtime.apply(two));
    assert!(a.is_ok());
    assert_eq!(b.unwrap_err().0, "BUSY");
    let mut reordered = one.clone();
    reordered.changes.modes[0].keys.reverse();
    reordered.changes.modes[0].lights.reverse();
    assert!(runtime.apply(reordered).await.is_ok());
    assert_eq!(
        runtime.connect("test-x1", false).await.unwrap_err().0,
        "BUSY"
    );
    assert_eq!(terminal(&runtime, "one").await.status, "completed");
    runtime.connect("test-x1", false).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.subscribe().await.0["devices"][0]["connectionState"] != "disconnected" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let disconnected = runtime.configuration("test-x1").await.unwrap();
    assert!(disconnected.configuration.is_none());
    assert_ne!(disconnected.base_token, token);
    runtime.connect("test-x1", true).await.unwrap();
    ready(&runtime).await;
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
async fn partial_write_failure_invalidates_baseline_and_does_not_retry() {
    let writes = Arc::new(AtomicUsize::new(0));
    let (_directory, runtime) = setup(
        Box::new(FailingHardware {
            writes: writes.clone(),
        }),
        true,
        None,
    )
    .await;
    let token = runtime.configuration("test-x1").await.unwrap().base_token;
    let request = apply(&token, "fail");
    runtime.apply(request.clone()).await.unwrap();
    let result = terminal(&runtime, "fail").await;
    assert_eq!(result.status, "failed");
    assert_eq!(result.error_code.as_deref(), Some("DEVICE_TIMEOUT"));
    assert_eq!(result.effect, "unknown");
    assert!(result.progress > 0.0);
    assert!(
        runtime
            .configuration("test-x1")
            .await
            .unwrap()
            .configuration
            .is_none()
    );
    assert_eq!(runtime.apply(request).await.unwrap().status, "failed");
    assert_eq!(writes.load(Ordering::SeqCst), 2);
    runtime.shutdown().await;
}

#[tokio::test]
async fn real_device_baseline_requires_explicit_complete_initialization() {
    let (_directory, runtime) = setup(Box::<MockX1>::default(), false, None).await;
    let document = runtime.configuration("test-x1").await.unwrap();
    assert!(document.configuration.is_none());
    assert_eq!(
        runtime
            .apply(apply(&document.base_token, "unknown"))
            .await
            .unwrap_err()
            .0,
        "INCOMPLETE_WRITE_GROUP"
    );
    runtime.shutdown().await;
    let (_directory, runtime) = setup(
        Box::<MockX1>::default(),
        false,
        Some(Configuration::mock_defaults()),
    )
    .await;
    let operation = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let snapshot = runtime.subscribe().await.0;
            if let Some(id) = snapshot["operations"][0]["operationId"].as_str() {
                break id.to_owned();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal(&runtime, &operation).await.status, "completed");
    assert_eq!(
        runtime
            .configuration("test-x1")
            .await
            .unwrap()
            .configuration
            .unwrap()
            .modes
            .len(),
        4
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn ready_notification_rechecks_baseline_before_queued_apply_can_write() {
    for fail in [true, false] {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let writes = Arc::new(AtomicUsize::new(0));
        let (_directory, runtime) = setup(
            Box::new(DelayedNotification {
                started: started.clone(),
                release: release.clone(),
                writes: writes.clone(),
                fail,
            }),
            true,
            None,
        )
        .await;
        let document = runtime.configuration("test-x1").await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        // Make the notification ready before admission yields to the worker.
        // It must be projected before executing the newly queued write.
        release.notify_one();
        runtime
            .apply(apply(&document.base_token, "racing-notification"))
            .await
            .unwrap();
        let operation = terminal(&runtime, "racing-notification").await;
        assert_eq!(operation.status, "failed");
        assert_eq!(operation.effect, "none");
        assert_eq!(
            operation.error_code.as_deref(),
            Some(if fail {
                "DEVICE_NOT_READY"
            } else {
                "BASE_CONFLICT"
            })
        );
        assert_eq!(writes.load(Ordering::SeqCst), 0);
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
}
