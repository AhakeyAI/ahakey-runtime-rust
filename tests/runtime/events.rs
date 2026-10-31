use super::*;

struct NotifyingHardware {
    events: tokio::sync::mpsc::Receiver<Result<HardwareState>>,
    refreshes: Arc<AtomicUsize>,
    state: HardwareState,
}
#[async_trait]
impl Hardware for NotifyingHardware {
    fn configuration_commands(&self, configuration: &Configuration) -> Result<Vec<Vec<u8>>> {
        ahakey_runtime_rust::protocol::x1::configuration_commands(configuration)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        Ok(self.state.clone())
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        self.connect().await
    }
    async fn next_event(&mut self) -> Result<HardwareState> {
        let state = self
            .events
            .recv()
            .await
            .unwrap_or(Err(Error("DEVICE_DISCONNECTED")))?;
        self.state = state.clone();
        Ok(state)
    }
    async fn write_confirmed(&mut self, _: &[u8]) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn hardware_notifications_reach_websocket_without_waiting_for_status_poll() {
    let (sender, events) = tokio::sync::mpsc::channel(8);
    let refreshes = Arc::new(AtomicUsize::new(0));
    let (_directory, runtime) = setup(
        Box::new(NotifyingHardware {
            events,
            refreshes: refreshes.clone(),
            state: HardwareState {
                work_mode: 0,
                light_mode: 0,
                battery: Some(82),
                lever: Some("manual"),
                brightness: Some(35),
            },
        }),
        false,
        None,
    )
    .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(ipc::serve(listener, runtime.clone(), "test-token".into()));
    let mut client = Client::open(&endpoint).await;
    client.hello().await;
    client.subscribe().await;
    sender
        .send(Ok(HardwareState {
            work_mode: 3,
            light_mode: 0,
            battery: Some(82),
            lever: Some("manual"),
            brightness: Some(35),
        }))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_millis(500), client.receive())
        .await
        .expect("mode notification must reach Studio without waiting for a poll");
    assert_eq!(event["params"]["type"], "device.changed");
    assert_eq!(event["params"]["data"]["workMode"], 3);
    tokio::time::sleep(Duration::from_millis(320)).await;
    assert!(
        (2..=5).contains(&refreshes.load(Ordering::SeqCst)),
        "idle status queries should run about ten times per second without bursts"
    );
    sender
        .send(Err(Error("DEVICE_DISCONNECTED")))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let event = client.receive().await;
            if event["params"]["type"] == "device.changed" {
                assert_eq!(event["params"]["data"]["connectionState"], "disconnected");
                break;
            }
        }
    })
    .await
    .expect("link loss must be reported without polling");
    client.socket.close(None).await.unwrap();
    server.abort();
    runtime.shutdown().await;
}
