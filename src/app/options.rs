//! Command-line options and explicit initial-configuration validation.
use ahakey_runtime_rust::{configuration::Configuration, discovery};
use std::path::PathBuf;
pub(super) struct Options {
    pub(super) discovery_path: PathBuf,
    pub(super) mock: bool,
    pub(super) selected: Option<String>,
    pub(super) initialize_path: Option<PathBuf>,
    pub(super) input_monitor: bool,
    pub(super) parent_stdin: bool,
    pub(super) debug: bool,
    pub(super) debug_log: Option<PathBuf>,
}
pub(super) fn parse() -> Result<Option<Options>, Box<dyn std::error::Error>> {
    let mut discovery_path = discovery::default_path()?;
    let mut mock = false;
    let mut selected = None;
    let mut initialize_path = None;
    let mut input_monitor = false;
    let mut parent_stdin = false;
    let mut debug = match std::env::var("AHAKEY_DEBUG").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => cfg!(debug_assertions),
    };
    let mut debug_log = std::env::var_os("AHAKEY_DEBUG_LOG").map(PathBuf::from);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mock" => mock = true,
            "--input-monitor" => input_monitor = true,
            "--parent-stdin" => parent_stdin = true,
            "--debug" => debug = true,
            "--debug-log" => {
                debug_log = Some(PathBuf::from(
                    args.next().ok_or("--debug-log needs a new file path")?,
                ));
            }
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
                    "  --debug                    Trace BLE frames, state and command timing to stderr\n  --debug-log NEW_FILE       Write JSONL diagnostics to a new file (implies --debug)\n  --input-monitor           Start the bundled HID observation helper\n  --parent-stdin            Shut down when the parent closes stdin\nDefaults: diagnostics on in Debug builds, off in Release builds\nEnvironment: AHAKEY_DEBUG=0|1, AHAKEY_DEBUG_LOG=NEW_FILE"
                );
                return Ok(None);
            }
            _ => return Err(format!("Unknown option: {arg}").into()),
        }
    }
    if mock && (initialize_path.is_some() || selected.is_some()) {
        return Err("--mock cannot be combined with --initialize or --device-id".into());
    }
    Ok(Some(Options {
        discovery_path,
        mock,
        selected,
        initialize_path,
        input_monitor,
        parent_stdin,
        debug,
        debug_log,
    }))
}
pub(super) fn load_configuration(
    initialize_path: Option<PathBuf>,
    selected: &Option<String>,
) -> Result<Option<Configuration>, Box<dyn std::error::Error>> {
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
    Ok(initialize)
}
