use ahakey_runtime_rust::{
    ahakey::{device::AhaKey, hardware::Hardware},
    configuration::{Apply, Configuration},
    runtime::Runtime,
    wire::Operation,
};
use std::{sync::Arc, time::Duration};
use tempfile::TempDir;
pub(crate) async fn ready(runtime: &Runtime) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if runtime.subscribe().await.0["devices"][0]["connectionState"] == "ready" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
pub(crate) async fn terminal(runtime: &Runtime, id: &str) -> Operation {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(operation) = runtime.operation(id).await
                && operation.terminal()
            {
                break operation;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
pub(crate) async fn setup(
    hardware: Box<dyn Hardware>,
    simulated: bool,
    initial: Option<Configuration>,
) -> (TempDir, Arc<Runtime>) {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(directory.path().join("state.json"))
        .await
        .unwrap();
    runtime
        .register(
            AhaKey {
                device_id: "test-x1".into(),
                name: "Test X1".into(),
                hardware,
            },
            initial,
            simulated,
        )
        .await
        .unwrap();
    ready(&runtime).await;
    (directory, runtime)
}
pub(crate) fn apply(token: &str, id: &str) -> Apply {
    let mut request: Apply = serde_json::from_str(include_str!(
        "../../../contracts/runtime-v1/fixtures/apply.json"
    ))
    .unwrap();
    request.device_id = "test-x1".into();
    request.operation_id = id.into();
    request.base_token = token.into();
    request
}
