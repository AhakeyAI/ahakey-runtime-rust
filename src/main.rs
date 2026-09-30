use ahakey_runtime_rust::{
    ahakey::{device::AhaKey, mock::MockX1},
    configuration::Configuration,
    discovery::{self, Discovery},
    ipc,
    protocol::ble,
    runtime::Runtime,
};
use std::{path::PathBuf, time::Duration};
use tokio::net::TcpListener;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut discovery_path = discovery::default_path()?;
    let mut mock = false;
    let mut selected = None;
    let mut initialize_path = None;
    let mut input_monitor = false;
    let mut parent_stdin = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mock" => mock = true,
            "--input-monitor" => input_monitor = true,
            "--parent-stdin" => parent_stdin = true,
            "--discovery" => {
                discovery_path = PathBuf::from(
                    args.next()
                        .ok_or("--discovery needs an absolute file path")?,
                )
            }
            "--device-id" => selected = Some(args.next().ok_or("--device-id needs an ID")?),
            "--initialize" => {
                initialize_path = Some(PathBuf::from(
                    args.next()
                        .ok_or("--initialize needs a Configuration JSON file")?,
                ))
            }
            "--help" | "-h" => {
                println!(
                    "AhaKey Runtime\n  --mock                     Explicit simulated X1 (no Bluetooth)\n  --discovery ABSOLUTE_PATH  Private discovery file; default matches Studio\n  --device-id ID             Select one discovered X1\n  --initialize FILE          Explicitly overwrite all four modes and brightness on selected X1; requires --device-id\n"
                );
                println!(
                    "  --input-monitor           Start the bundled HID observation helper\n  --parent-stdin            Shut down when the parent closes stdin"
                );
                return Ok(());
            }
            _ => return Err(format!("Unknown option: {arg}").into()),
        }
    }
    if mock && (initialize_path.is_some() || selected.is_some()) {
        return Err("--mock cannot be combined with --initialize or --device-id".into());
    }
    if initialize_path.is_some() && selected.is_none() {
        return Err(
            "--initialize requires --device-id to identify the device being overwritten".into(),
        );
    }
    let initialize = initialize_path
        .map(
            |path| -> Result<Configuration, Box<dyn std::error::Error>> {
                let mut config: Configuration = serde_json::from_slice(&std::fs::read(path)?)?;
                config.validate()?;
                config.canonicalize();
                if config.modes.iter().map(|m| m.mode).collect::<Vec<_>>() != [0, 1, 2, 3] {
                    return Err("Initialization requires all four modes".into());
                }
                Ok(config)
            },
        )
        .transpose()?;
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
    Ok(())
}
async fn parent_closed() {
    // A dedicated thread avoids keeping Tokio's blocking pool alive on SIGTERM
    // while the parent still owns the pipe's write end.
    let (closed, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut input = std::io::stdin().lock();
        let mut buffer = [0; 32];
        while let Ok(count) = input.read(&mut buffer) {
            if count == 0 {
                break;
            }
        }
        let _ = closed.send(());
    });
    let _ = receiver.await;
}
async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result, _ = terminate.recv() => Ok(()) }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}
