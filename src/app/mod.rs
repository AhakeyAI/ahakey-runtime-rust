//! Process bootstrap; device and request behavior lives in the library modules.
mod lifecycle;
mod options;
use ahakey_runtime_rust::{
    ahakey::{device::AhaKey, mock::MockX1},
    discovery::Discovery,
    ipc,
    protocol::ble,
    runtime::Runtime,
};
use lifecycle::{parent_closed, shutdown_signal};
use options::Options;
use std::time::Duration;
use tokio::net::TcpListener;
use uuid::Uuid;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let Some(Options {
        discovery_path,
        mock,
        selected,
        initialize_path,
        input_monitor,
        parent_stdin,
        debug,
        debug_log,
    }) = options::parse()?
    else {
        return Ok(());
    };
    let _debug_guard = if debug || debug_log.is_some() {
        Some(ahakey_runtime_rust::debug::init(debug_log.as_deref())?)
    } else {
        None
    };
    ahakey_runtime_rust::debug::event("runtime.start", || {
        serde_json::json!({
            "version":env!("CARGO_PKG_VERSION"), "mock":mock,
            "selectedDevice":selected, "inputMonitor":input_monitor,
        })
    });
    let initialize = options::load_configuration(initialize_path, &selected)?;
    let mut discovery = Discovery::acquire(discovery_path)?;
    let runtime = Runtime::open(discovery.path().with_extension(if mock {
        "mock-state.json"
    } else {
        "state.json"
    }))
    .await?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let endpoint = format!("ws://{}/", listener.local_addr()?);
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    discovery.publish(&endpoint, &token)?;
    eprintln!(
        "Runtime listening at {endpoint}\nDiscovery: {}",
        discovery.path().display()
    );
    let discovery_task = if mock {
        runtime
            .register(
                AhaKey {
                    device_id: "mock-x1".into(),
                    name: "AhaKey X1 (simulated)".into(),
                    hardware: Box::<MockX1>::default(),
                },
                None,
                true,
            )
            .await?;
        None
    } else {
        Some(tokio::spawn(ble::discover(
            runtime.clone(),
            selected,
            initialize,
        )))
    };
    let input_task = if input_monitor && !mock {
        let helper = std::env::current_exe()?.with_file_name("ahakey-input-monitor");
        Some(tokio::spawn(ahakey_runtime_rust::input::monitor(
            runtime.clone(),
            helper,
        )))
    } else {
        None
    };
    let mut serving = tokio::spawn(ipc::serve(listener, runtime.clone(), token));
    tokio::select! {
        result = &mut serving => result??,
        result = shutdown_signal() => result?,
        _ = parent_closed(), if parent_stdin => {},
    }
    serving.abort();
    let _ = serving.await;
    discovery.unpublish();
    if let Some(task) = input_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(task) = discovery_task {
        task.abort();
        let _ = task.await;
    }
    eprintln!("Runtime shutting down; allowing accepted writes up to 30 seconds to finish");
    if tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
        .await
        .is_err()
    {
        eprintln!(
            "Shutdown deadline reached; unfinished operations will recover as unknown on restart"
        );
    }
    drop(discovery);
    ahakey_runtime_rust::debug::event("runtime.stop", || serde_json::json!({}));
    Ok(())
}
