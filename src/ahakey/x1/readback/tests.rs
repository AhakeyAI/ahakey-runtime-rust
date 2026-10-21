use super::*;
use crate::{
    ahakey::{
        device::AhaKey,
        hardware::Hardware,
        state::{HardwareState, X1State},
    },
    runtime::Runtime,
};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../../tests/fixtures/x1-readback.json")).unwrap()
}
fn case(name: &str) -> Vec<u8> {
    unhex(fixture()["cases"][name]["response"].as_str().unwrap())
}
struct Cancelled(Arc<AtomicBool>);
impl Drop for Cancelled {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct Device {
    responses: Arc<Mutex<BTreeMap<Vec<u8>, Vec<u8>>>>,
    queries: usize,
    round: usize,
    versions: Vec<Vec<u8>>,
    stall: bool,
    cancelled: Arc<AtomicBool>,
    writes: Arc<AtomicUsize>,
    connects: Arc<AtomicUsize>,
}
impl Device {
    fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(
                fixture()["responses"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(q, r)| (unhex(q), unhex(r.as_str().unwrap())))
                    .collect(),
            )),
            queries: 0,
            round: 0,
            versions: vec![],
            stall: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            writes: Arc::new(AtomicUsize::new(0)),
            connects: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn response(&mut self, request: Vec<u8>, response: Vec<u8>) {
        self.responses.lock().unwrap().insert(request, response);
    }
    fn into_device(self) -> AhaKey {
        AhaKey {
            device_id: "readable-x1".into(),
            name: "X1".into(),
            hardware: Box::new(self),
        }
    }
}
#[async_trait]
impl CommandTransport for Device {
    async fn query(&mut self, command: &[u8]) -> Result<Vec<u8>> {
        self.queries += 1;
        assert!(
            matches!(command[2], 0 | 0x9f | 0x87 | 0x88),
            "readback must not write or use 0x9D"
        );
        if command == frame(0x88, &[0]) {
            self.round += 1;
            if std::mem::take(&mut self.stall) {
                let _cancel = Cancelled(self.cancelled.clone());
                std::future::pending::<()>().await;
            }
        }
        if command == frame(0x87, &[0, 0]) && !self.versions.is_empty() {
            return Ok(self.versions[(self.round - 1).min(self.versions.len() - 1)].clone());
        }
        Ok(self
            .responses
            .lock()
            .unwrap()
            .get(command)
            .expect("query must exist in production C capture")
            .clone())
    }
}
#[async_trait]
impl Hardware for Device {
    fn configuration_commands(
        &self,
        config: &crate::configuration::Configuration,
    ) -> Result<Vec<Vec<u8>>> {
        super::super::commands::configuration_commands(config)
    }
    async fn connect(&mut self) -> Result<HardwareState> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        self.refresh().await
    }
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&mut self) -> Result<HardwareState> {
        Ok(X1State::from_bytes(&self.query(&frame(0, &[])).await?)
            .unwrap()
            .0)
    }
    async fn read_configuration(&mut self) -> Result<Option<ConfigurationReadback>> {
        let budget = if self.stall {
            Duration::from_millis(20)
        } else {
            Duration::from_secs(5)
        };
        read_bounded(self, budget).await
    }
    async fn write_confirmed(&mut self, _: &[u8]) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn production_c_capture_restores_descriptions_and_a_complete_configuration() {
    let mut device = Device::new();
    let observed = read(&mut device).await.unwrap().unwrap();
    assert_eq!(device.queries, 107);
    assert_eq!(observed.device.brightness_percent, 35);
    assert!(observed.device.task_light_mode.is_none());
    for mode in &observed.modes {
        for (index, key) in mode.keys.iter().enumerate() {
            assert_eq!(
                key.action,
                Some(Action::Shortcut {
                    modifiers: vec![],
                    key_code: 7 + mode.mode * 4 + index as u8
                })
            );
            assert_eq!(key.description, Some(format!("M{}K{}", mode.mode, index)));
            assert_eq!(key.raw_binding.len(), 100);
        }
        for (index, light) in mode.lights.iter().enumerate() {
            assert_eq!(light.effect, EFFECTS[(mode.mode as usize + index) % 17]);
        }
    }
    assert!(observed.complete_configuration().is_some());
    for name in [
        "empty",
        "macro",
        "chord",
        "nonAscii",
        "maximum",
        "tooLarge",
        "unsupportedType",
    ] {
        let decoded = key(&case(name), 0, 0).unwrap();
        match name {
            "empty" => {
                assert_eq!(decoded.action, Some(Action::Disabled {}));
                assert_eq!(decoded.description.as_deref(), Some(""));
            }
            "macro" => assert!(matches!(decoded.action, Some(Action::Macro { .. }))),
            "nonAscii" => {
                assert!(decoded.description.is_none());
                assert_eq!(decoded.raw_description, Some(vec![255]));
            }
            "tooLarge" | "unsupportedType" => {
                assert!(decoded.action.is_none());
                assert!(decoded.raw_binding.is_empty());
                assert_eq!(
                    decoded.read_status,
                    Some(if name == "tooLarge" { 3 } else { 2 })
                );
            }
            _ => {
                assert!(decoded.action.is_none());
                assert_eq!(decoded.raw_binding.len(), 100);
            }
        }
    }
}

#[tokio::test]
async fn unsupported_identities_and_generic_acks_do_not_enable_readback() {
    for identity in [
        frame(0x9f, &[0]),
        frame(0x9f, b"\0unknown"),
        unhex("aabb9f000302010401ff0700000801ccdd"),
    ] {
        let mut device = Device::new();
        device.response(frame(0x9f, &[]), identity);
        assert!(read(&mut device).await.unwrap().is_none());
        assert_eq!(device.queries, 1);
    }
    for command in [frame(0x88, &[0]), frame(0x87, &[0, 0])] {
        let mut device = Device::new();
        device.response(command.clone(), frame(command[2], &[0]));
        assert_eq!(
            read(&mut device).await.unwrap_err().0,
            "UNSUPPORTED_CONFIGURATION"
        );
    }
}

#[tokio::test]
async fn raw_observations_and_error_status_must_stabilize_before_publication() {
    for (a, mut b, offset) in [
        (case("chord"), case("chord"), 9),
        (case("nonAscii"), case("nonAscii"), 10),
        (case("tooLarge"), case("unsupportedType"), usize::MAX),
    ] {
        if offset != usize::MAX {
            b[offset] ^= 1;
        }
        let mut device = Device::new();
        device.versions = vec![a.clone(), b.clone(), b.clone()];
        let result = read(&mut device).await.unwrap().unwrap();
        assert_eq!(device.round, 3);
        assert!(result.complete_configuration().is_none());
        let mut unstable = Device::new();
        unstable.versions = vec![a.clone(), b, a];
        assert_eq!(
            read(&mut unstable).await.unwrap_err().0,
            "CONFIGURATION_CHANGED"
        );
    }
}

#[tokio::test]
async fn malformed_addresses_lengths_selectors_and_ranges_are_rejected() {
    let base = unhex(fixture()["responses"]["aabb870000ccdd"].as_str().unwrap());
    for field in [4, 5, 6, 7, 9] {
        let mut invalid_key = base.clone();
        invalid_key[field] ^= 0x80;
        assert_eq!(
            key(&invalid_key, 0, 0).unwrap_err().0,
            "DEVICE_INVALID_RESPONSE"
        );
    }
    for command in [frame(0x88, &[0]), frame(0x88, &[1, 1, 0])] {
        let response = Device::new().responses.lock().unwrap()[&command].clone();
        let changes = if command.len() == 6 {
            vec![(4, 1), (5, 101)]
        } else {
            vec![(4, 0), (5, 0), (6, 1), (7, 17)]
        };
        let mut invalid_responses = vec![
            response[..response.len() - 1].to_vec(),
            [response.clone(), vec![0]].concat(),
        ];
        for (field, value) in changes {
            let mut malformed = response.clone();
            malformed[field] = value;
            invalid_responses.push(malformed);
        }
        for malformed in invalid_responses {
            let mut device = Device::new();
            device.response(command.clone(), malformed);
            assert_eq!(
                read(&mut device).await.unwrap_err().0,
                "DEVICE_INVALID_RESPONSE"
            );
        }
    }
    let mut zero = Device::new();
    zero.response(frame(0x88, &[0]), frame(0x88, &[0, 0, 0]));
    let result = read(&mut zero).await.unwrap().unwrap();
    assert_eq!(result.device.brightness_percent, 0);
    assert!(result.complete_configuration().is_none());
}

#[tokio::test]
async fn deadline_cancels_the_pending_read_and_allows_a_fresh_attempt() {
    let mut device = Device::new();
    device.stall = true;
    assert_eq!(
        read_bounded(&mut device, Duration::from_millis(5))
            .await
            .unwrap_err()
            .0,
        "DEVICE_TIMEOUT"
    );
    assert!(device.cancelled.load(Ordering::SeqCst));
    assert!(
        read(&mut device)
            .await
            .unwrap()
            .unwrap()
            .complete_configuration()
            .is_some()
    );
}

async fn ready(runtime: &Runtime, wanted: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if runtime.subscribe().await.0["devices"][0]["connectionState"] == wanted {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn worker_and_authenticated_websocket_read_real_protocol_and_reconnect_without_writes() {
    for scenario in ["complete", "tooLarge", "ack", "timeout"] {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Runtime::open(directory.path().join("state.json"))
            .await
            .unwrap();
        let mut device = Device::new();
        if scenario == "tooLarge" {
            device.response(frame(0x87, &[0, 0]), case("tooLarge"));
        }
        if scenario == "ack" {
            device.response(frame(0x88, &[0]), frame(0x88, &[0]));
        }
        device.stall = scenario == "timeout";
        let responses = device.responses.clone();
        let connects = device.connects.clone();
        let writes = device.writes.clone();
        runtime
            .register(device.into_device(), None, false)
            .await
            .unwrap();
        ready(&runtime, "ready").await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(crate::ipc::serve(
            listener,
            runtime.clone(),
            "test-token".into(),
        ));
        let (mut socket, _) = connect_async(endpoint).await.unwrap();
        for (method, params) in [
            (
                "runtime.hello",
                json!({"protocol":{"major":1,"minor":0},"client":{"name":"readback-test","version":"1","kind":"cli"},"token":"test-token"}),
            ),
            ("configuration.get", json!({"deviceId":"readable-x1"})),
        ] {
            socket
                .send(Message::Text(
                    json!({"jsonrpc":"2.0","id":method,"method":method,"params":params})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            let Message::Text(text) = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
            else {
                panic!("expected response");
            };
            let reply: Value = serde_json::from_str(&text).unwrap();
            assert!(reply.get("error").is_none(), "{reply}");
            if method == "configuration.get" {
                assert_eq!(
                    reply["result"]["configuration"].is_null(),
                    matches!(scenario, "tooLarge" | "ack")
                );
                if scenario == "tooLarge" {
                    assert_eq!(
                        reply["result"]["readback"]["modes"][0]["keys"][0]["readStatus"],
                        3
                    );
                }
            }
        }
        assert_eq!(
            connects.load(Ordering::SeqCst),
            if scenario == "timeout" { 2 } else { 1 }
        );
        if scenario == "complete" {
            let initial = runtime.configuration("readable-x1").await.unwrap();
            runtime.connect("readable-x1", false).await.unwrap();
            ready(&runtime, "disconnected").await;
            responses
                .lock()
                .unwrap()
                .get_mut(&frame(0x87, &[0, 0]))
                .unwrap()[11] = b'9';
            runtime.connect("readable-x1", true).await.unwrap();
            ready(&runtime, "ready").await;
            let fresh = runtime.configuration("readable-x1").await.unwrap();
            assert_ne!(initial.base_token, fresh.base_token);
            assert_eq!(
                fresh.configuration.as_ref().unwrap().modes[0].keys[0].description,
                "M9K0"
            );
        }
        socket.close(None).await.unwrap();
        server.abort();
        let _ = server.await;
        let previous = runtime
            .configuration("readable-x1")
            .await
            .unwrap()
            .base_token;
        runtime.shutdown().await;
        drop(runtime);
        if scenario == "complete" {
            let restarted = Runtime::open(directory.path().join("state.json"))
                .await
                .unwrap();
            let mut hardware = Device::new();
            hardware.responses = responses;
            hardware.writes = writes.clone();
            restarted
                .register(hardware.into_device(), None, false)
                .await
                .unwrap();
            ready(&restarted, "ready").await;
            let document = restarted.configuration("readable-x1").await.unwrap();
            assert_ne!(document.base_token, previous);
            assert_eq!(
                document.configuration.unwrap().modes[0].keys[0].description,
                "M9K0"
            );
            restarted.shutdown().await;
        }
        assert_eq!(writes.load(Ordering::SeqCst), 0);
    }
}
