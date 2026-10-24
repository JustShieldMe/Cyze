//! In-app log capture.
//!
//! The app emits diagnostics via `tracing`, but nothing was subscribing to them,
//! so they went nowhere. This installs a global subscriber that formats every
//! event to **stdout** (for terminal runs) and into an in-memory **ring buffer**
//! that the UI can read back — so a user can copy the logs (e.g. sync timing)
//! straight from the app without a terminal.
//!
//! The buffer is bounded ([`MAX_LINES`]) and lives only for the process: it is
//! not persisted to disk (logs can contain addresses/amounts, and a wallet
//! should not silently write those to a log file). Restarting the app clears it.
//!
//! Every line passes through [`redact_secrets`] before it reaches the buffer or
//! stdout, so a group id or viewing key logged by mistake never ends up in a
//! copied bug report or the system journal.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use frost_app_core::redact::redact_secrets;
use tracing_subscriber::fmt::writer::MakeWriter;

/// Maximum number of log lines retained in memory. Older lines are dropped as
/// new ones arrive, so a long-running session can't grow memory without bound.
const MAX_LINES: usize = 3000;

/// A bounded, shareable ring buffer of formatted log lines.
#[derive(Clone)]
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<String>>>,
    /// Also print each (redacted) event to stdout, for terminal runs.
    echo_stdout: bool,
}

impl LogBuffer {
    fn new() -> Self {
        Self {
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LINES))),
            echo_stdout: false,
        }
    }

    /// A copy of the currently buffered lines, oldest first.
    pub fn snapshot(&self) -> Vec<String> {
        self.lines
            .lock()
            .map(|l| l.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Drop all buffered lines.
    pub fn clear(&self) {
        if let Ok(mut l) = self.lines.lock() {
            l.clear();
        }
    }

    fn push_line(&self, line: String) {
        if let Ok(mut l) = self.lines.lock() {
            while l.len() >= MAX_LINES {
                l.pop_front();
            }
            l.push_back(line);
        }
    }
}

/// The process-wide log buffer, shared by the subscriber (writer) and the
/// `get_logs` command (reader).
static LOG_BUFFER: OnceLock<LogBuffer> = OnceLock::new();

/// Access the global log buffer, creating it on first use.
pub fn global() -> &'static LogBuffer {
    LOG_BUFFER.get_or_init(|| LogBuffer {
        echo_stdout: true,
        ..LogBuffer::new()
    })
}

/// A short-lived writer for one `tracing` event. `fmt` formats the whole event
/// (a single line ending in `\n`) into this, then drops it; on drop we split the
/// accumulated bytes into lines and append them to the buffer.
pub struct LineWriter {
    buf: LogBuffer,
    pending: Vec<u8>,
}

impl io::Write for LineWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let raw = String::from_utf8_lossy(&self.pending);
        let text = redact_secrets(&raw);
        if self.buf.echo_stdout {
            let _ = io::Write::write_all(&mut io::stdout().lock(), text.as_bytes());
        }
        for line in text.split('\n') {
            let line = line.trim_end_matches('\r');
            if !line.is_empty() {
                self.buf.push_line(line.to_string());
            }
        }
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LineWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LineWriter {
            buf: self.clone(),
            pending: Vec::new(),
        }
    }
}

/// Install the global `tracing` subscriber. Idempotent: a second call (e.g. in
/// tests) is a no-op rather than a panic. Formats to stdout and the in-app ring
/// buffer, honouring `RUST_LOG` when set; otherwise our crates log at `debug`
/// (so sync timing is captured) and dependencies stay at `info`.
pub fn init_logging() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,frost_app=debug,frost_app_core=debug"));

    // The global buffer echoes each redacted event to stdout itself, so stdout
    // never sees an unredacted line.
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(false)
        .with_env_filter(filter)
        .with_writer(global().clone())
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn ring_buffer_caps_and_orders() {
        let buf = LogBuffer::new();
        for i in 0..(MAX_LINES + 50) {
            buf.push_line(format!("line {i}"));
        }
        let snap = buf.snapshot();
        assert_eq!(snap.len(), MAX_LINES, "buffer is capped");
        assert_eq!(snap.first().unwrap(), "line 50", "oldest dropped first");
        assert_eq!(snap.last().unwrap(), &format!("line {}", MAX_LINES + 49));
        buf.clear();
        assert!(buf.snapshot().is_empty());
    }

    #[test]
    fn line_writer_splits_events_into_lines() {
        let buf = LogBuffer::new();
        {
            let mut w = buf.make_writer();
            w.write_all(b"first line\n").unwrap();
        } // dropped here -> flushed
        {
            let mut w = buf.make_writer();
            w.write_all(b"second\nthird\n").unwrap();
        }
        assert_eq!(buf.snapshot(), vec!["first line", "second", "third"]);
    }

    #[test]
    fn line_writer_redacts_secrets() {
        let buf = LogBuffer::new();
        let ak = "d5a1c1b0e2f3a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3";
        {
            let mut w = buf.make_writer();
            writeln!(w, "wallet setup: group={ak}").unwrap();
        }
        assert_eq!(buf.snapshot(), vec!["wallet setup: group=[redacted]"]);
    }
}
