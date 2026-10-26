use crate::runtime::Runtime;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
};

/// The pipe is owned by Runtime, so EOF ends recording if Runtime crashes.
pub async fn run(runtime: Arc<Runtime>, executable: PathBuf) {
    let result = async {
        let mut child = Command::new(executable)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
        let (sender, mut commands) = mpsc::channel::<Value>(32);
        runtime.attach_voice(sender).await;
        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { break; };
                    input.write_all(format!("{command}\n").as_bytes()).await?;
                    input.flush().await?;
                }
                line = output.next_line() => {
                    let Some(line) = line? else { break; };
                    if line.len() > 131_072 {
                        return Err(std::io::Error::other("Oversized voice event"));
                    }
                    let event = serde_json::from_str(&line).map_err(std::io::Error::other)?;
                    runtime.voice_event(event).await;
                }
            }
        }
        Ok::<(), std::io::Error>(())
    }
    .await;
    if let Err(error) = result {
        eprintln!("Voice helper: {error}");
    }
    runtime.detach_voice().await;
}
