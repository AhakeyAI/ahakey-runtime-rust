//! Build-default or explicitly enabled diagnostics. A bounded queue keeps disk/terminal I/O off device workers.
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::Path,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

static TRACE: OnceLock<Trace> = OnceLock::new();
struct Trace {
    sender: mpsc::SyncSender<Option<Value>>,
    started: Instant,
    dropped: AtomicU64,
}

pub struct Guard(Option<thread::JoinHandle<()>>);
impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(trace) = TRACE.get() {
            let _ = trace.sender.send(None);
        }
        if let Some(writer) = self.0.take() {
            let _ = writer.join();
        }
    }
}

/// A log file must be new, preventing accidental truncation of configuration files.
/// With no path, JSON lines go to stderr alongside normal Runtime messages.
pub fn init(path: Option<&Path>) -> io::Result<Guard> {
    let mut sink: Box<dyn Write + Send> = if let Some(path) = path {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        Box::new(options.open(path)?)
    } else {
        Box::new(io::stderr())
    };
    let (sender, receiver) = mpsc::sync_channel::<Option<Value>>(256);
    TRACE
        .set(Trace {
            sender,
            started: Instant::now(),
            dropped: AtomicU64::new(0),
        })
        .map_err(|_| io::Error::other("debug logging already initialized"))?;
    let writer = thread::spawn(move || {
        while let Ok(Some(record)) = receiver.recv() {
            let line = record.to_string();
            if writeln!(sink, "{line}").is_err() {
                eprintln!("Debug log write failed; diagnostics disabled for this run");
                return;
            }
        }
        let dropped = TRACE.get().unwrap().dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            let _ = writeln!(
                sink,
                "{}",
                json!({"event":"debug.dropped", "count":dropped})
            );
        }
        let _ = sink.flush();
    });
    Ok(Guard(Some(writer)))
}

/// Callers provide only diagnostic fields, never IPC tokens or complete requests.
pub fn event(name: &'static str, fields: impl FnOnce() -> Value) {
    let Some(trace) = TRACE.get() else {
        return;
    };
    let record = json!({
        "event": name,
        "unixMs": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().to_string(),
        "elapsedMs": trace.started.elapsed().as_secs_f64() * 1000.0,
        "data": fields(),
    });
    if trace.sender.try_send(Some(record)).is_err() {
        trace.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn bytes(data: &[u8]) -> Value {
    use std::fmt::Write;
    let mut hex = String::new();
    for (index, byte) in data.iter().take(1024).enumerate() {
        if index > 0 {
            hex.push(' ');
        }
        let _ = write!(hex, "{byte:02X}");
    }
    json!({"length":data.len(), "hex":hex, "truncated":data.len() > 1024})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_unknown_and_malformed_bytes_with_bounded_output() {
        assert_eq!(bytes(&[0xaa, 0xbb, 0xff])["hex"], "AA BB FF");
        assert_eq!(bytes(&[])["hex"], "");
        let large = bytes(&[0x42; 2048]);
        assert_eq!(large["length"], 2048);
        assert_eq!(large["truncated"], true);
        assert_eq!(large["hex"].as_str().unwrap().len(), 3071);
    }
}
