use super::{InputMessage, InputState};
use crate::runtime::Runtime;
use std::{path::PathBuf, sync::Arc};
use tokio::{io::AsyncBufReadExt, process::Command};

/// The native adapter is bundled beside the Runtime, inherits its app identity,
/// and exits on stdin EOF. Dropping this task also kills the child.
pub async fn monitor(runtime: Arc<Runtime>, executable: PathBuf) {
    runtime
        .input_message(InputMessage::State(InputState {
            status: "starting".into(),
            permission: "unknown".into(),
            devices: vec![],
        }))
        .await;
    let result = async {
        let mut child = Command::new(executable)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        while let Some(line) = lines.next_line().await? {
            if line.len() > 16_384 {
                return Err(std::io::Error::other("Oversized input event"));
            }
            let message: InputMessage =
                serde_json::from_str(&line).map_err(std::io::Error::other)?;
            runtime.input_message(message).await;
        }
        child.wait().await?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    if let Err(error) = result {
        eprintln!("Input monitor: {error}");
    }
    runtime
        .input_message(InputMessage::State(InputState {
            status: "error".into(),
            permission: "unknown".into(),
            devices: vec![],
        }))
        .await;
}
