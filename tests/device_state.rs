use ahakey_runtime_rust::{
    ahakey::{device::AhaKey, hardware::Hardware, state::HardwareState},
    configuration::{Configuration, ConfigurationReadback},
    error::Result,
    runtime::Runtime,
};
use async_trait::async_trait;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

struct SilentLever {
    automatic: Arc<AtomicBool>,
    configuration_reads: Arc<AtomicUsize>,
}
#[async_trait]
impl Hardware for SilentLever {
    fn configuration_commands(&self, _: &Configuration) -> Result<Vec<Vec<u8>>> {
        panic!("Observing a lever must not write configuration")
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
            battery: Some(80),
            brightness: Some(50),
            lever: Some(if self.automatic.load(Ordering::SeqCst) {
                "automatic"
            } else {
                "manual"
            }),
        })
    }
    async fn read_configuration(&mut self) -> Result<Option<ConfigurationReadback>> {
        self.configuration_reads.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    async fn write_confirmed(&mut self, _: &[u8]) -> Result<()> {
        panic!("Observing a lever must not write configuration")
    }
    // No unsolicited notifications: this reproduces the connected X1 firmware.
}

#[tokio::test]
async fn silent_lever_changes_reach_subscribers_without_configuration_reads_or_writes() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(dir.path().join("state.json")).await.unwrap();
    let automatic = Arc::new(AtomicBool::new(false));
    let configuration_reads = Arc::new(AtomicUsize::new(0));
    let (_, mut events) = runtime.subscribe().await;
    runtime
        .register(
            AhaKey {
                device_id: "silent-x1".into(),
                name: "Silent X1".into(),
                hardware: Box::new(SilentLever {
                    automatic: automatic.clone(),
                    configuration_reads: configuration_reads.clone(),
                }),
            },
            None,
            false,
        )
        .await
        .unwrap();
    for (position, expected) in [(false, "manual"), (true, "automatic"), (false, "manual")] {
        automatic.store(position, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_millis(350), async {
            loop {
                let event = events.recv().await.unwrap();
                if event.kind == "device.changed" && event.data["lever"] == expected {
                    break;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("No device.changed event for {expected} lever"));
        assert_eq!(runtime.subscribe().await.0["devices"][0]["lever"], expected);
    }
    assert_eq!(configuration_reads.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.hooks().await["preferences"]["enabled"], false);
    assert_eq!(
        runtime.hooks().await["preferences"]["allowAutomatic"],
        false
    );
    runtime.shutdown().await;
}
