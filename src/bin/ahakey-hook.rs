//! Stdio adapter for Claude Code, Cursor, Codex and Kimi Code. Never executes tool input.
use ahakey_runtime_rust::hooks::{HookEvent, hook_output};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mut tool = String::new();
    let mut event = String::new();
    let mut path = None;
    let mut configuration = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tool" => tool = args.next().unwrap_or_default(),
            "--event" => event = args.next().unwrap_or_default(),
            "--discovery" => path = args.next().map(PathBuf::from),
            "--config" => configuration = true,
            _ => {
                eprintln!(
                    "Usage: ahakey-hook --tool claude|cursor|codex|kimi [--event EVENT] [--discovery PATH] [--config]"
                );
                std::process::exit(1);
            }
        }
    }
    if !["claude", "cursor", "codex", "kimi"].contains(&tool.as_str()) {
        eprintln!("Unknown tool");
        std::process::exit(1);
    }
    let path = path
        .or_else(|| std::env::var_os("AHAKEY_RUNTIME_DISCOVERY").map(PathBuf::from))
        .or_else(|| ahakey_runtime_rust::discovery::default_path().ok());
    if configuration {
        match config(&tool, path.as_deref()) {
            Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    let mut bytes = Vec::new();
    if std::io::stdin()
        .take(65_537)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > 65_536
    {
        emit(&tool, &event, "defer");
        return;
    }
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if event.is_empty() {
        event = value["hook_event_name"].as_str().unwrap_or("").into();
    }
    let session = value["session_id"]
        .as_str()
        .or_else(|| value["conversation_id"].as_str())
        .unwrap_or("");
    let summary = if let Some(command) = value["command"].as_str() {
        command.to_owned()
    } else if !value["tool_input"].is_null() {
        value["tool_input"].to_string()
    } else {
        String::new()
    };
    let context = HookEvent {
        event_id: Uuid::new_v4().to_string(),
        tool: tool.clone(),
        session_id: session.into(),
        event: event.clone(),
        cwd: value["cwd"]
            .as_str()
            .or_else(|| value["workspace_roots"][0].as_str())
            .unwrap_or("")
            .into(),
        tool_name: value["tool_name"]
            .as_str()
            .unwrap_or(if tool == "cursor" { "Shell" } else { "" })
            .into(),
        summary,
    };
    let outcome = tokio::time::timeout(Duration::from_secs(65), send(path, context)).await;
    let decision = match outcome {
        Ok(Ok(decision)) => decision,
        _ => "defer".into(),
    };
    emit(&tool, &event, &decision);
}

fn emit(tool: &str, event: &str, decision: &str) {
    // Kimi's permission hooks are observation-only; stdout can become prompt context.
    if tool != "kimi" {
        println!("{}", hook_output(tool, event, decision));
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
async fn rpc(
    socket: &mut Socket,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let id = id.to_string();
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await?;
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await?
            .ok_or("Disconnected")??;
        match message {
            Message::Text(text) => {
                let reply: Value = serde_json::from_str(&text)?;
                if reply["id"] == id {
                    if reply.get("error").is_some() {
                        return Err("Runtime rejected request".into());
                    }
                    return Ok(reply["result"].clone());
                }
            }
            Message::Ping(_) => socket.flush().await?,
            Message::Close(_) => return Err("Disconnected".into()),
            _ => {}
        }
    }
}
async fn send(
    path: Option<PathBuf>,
    context: HookEvent,
) -> Result<String, Box<dyn std::error::Error>> {
    context.validate()?;
    let path = path.ok_or("Missing discovery")?;
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.len() > 16_384 {
        return Err("Invalid discovery".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Discovery must be private".into());
        }
    }
    let discovery: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let endpoint = discovery["endpoint"].as_str().ok_or("Missing endpoint")?;
    // Runtime publishes exactly one private IPv4 loopback endpoint.
    let port = endpoint
        .strip_prefix("ws://127.0.0.1:")
        .map(|s| s.strip_suffix('/').unwrap_or(s))
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| *p > 0)
        .ok_or("Invalid endpoint")?;
    let token = discovery["token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Missing token")?;
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(3),
        connect_async(format!("ws://127.0.0.1:{port}/")),
    )
    .await??;
    rpc(&mut socket,1,"runtime.hello",json!({"protocol":{"major":1,"minor":0},"token":token,"client":{"name":"AhaKey Hook","version":"0.1.0","kind":"hook"}})).await?;
    let mut result = rpc(
        &mut socket,
        2,
        "hooks.event",
        serde_json::to_value(context)?,
    )
    .await?;
    let request_id = result["requestId"].as_str().map(str::to_owned);
    let mut serial = 3;
    while result["decision"] == "pending" {
        let request_id = request_id.as_deref().ok_or("Missing request ID")?;
        result = rpc(
            &mut socket,
            serial,
            "hooks.poll",
            json!({"requestId":request_id}),
        )
        .await?;
        serial += 1;
        if result["decision"] == "pending" {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    Ok(result["decision"].as_str().unwrap_or("defer").into())
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn config(tool: &str, path: Option<&std::path::Path>) -> Result<Value, Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let mut command = format!("{} --tool {}", quote(&executable.to_string_lossy()), tool);
    if let Some(path) = path {
        command.push_str(&format!(" --discovery {}", quote(&path.to_string_lossy())));
    }
    let events: &[&str] = if tool == "cursor" {
        &[
            "sessionStart",
            "sessionEnd",
            "beforeSubmitPrompt",
            "postToolUse",
            "postToolUseFailure",
            "beforeShellExecution",
            "beforeMCPExecution",
            "stop",
        ]
    } else if tool == "kimi" {
        &[
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionRequest",
            "PermissionResult",
            "Stop",
            "Interrupt",
        ]
    } else {
        &[
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "Stop",
        ]
    };
    let mut hooks = serde_json::Map::new();
    if tool == "kimi" {
        // JSON representation of Kimi's [[hooks]] TOML array, consumed by tooling.
        return Ok(
            json!({"hooks": events.iter().map(|event| json!({"event":event,"command":format!("{command} --event {event}"),"timeout":3})).collect::<Vec<_>>()}),
        );
    }
    for event in events {
        let cmd = format!("{command} --event {event}");
        let handlers = if tool == "cursor" {
            json!([{"command":cmd,"timeout":70}])
        } else {
            json!([{"hooks":[{"type":"command","command":cmd,"timeout":if *event=="SessionEnd" {3} else {70}}]}])
        };
        hooks.insert((*event).into(), handlers);
    }
    Ok(if tool == "cursor" {
        json!({"version":1,"hooks":hooks})
    } else {
        json!({"hooks":hooks})
    })
}
