//! Logging: human-readable lines to stderr and to a daily-rotated file in the
//! app data folder. Every line written to disk passes through credential
//! redaction. Logs record paths, sizes and hashes — never file contents.

use std::io::{self, Write};
use std::path::Path;

use devbridge_core::redact::redact;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

pub struct Redacting<W>(W);

impl<W: Write> Write for Redacting<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        self.0.write_all(redact(&s).as_bytes())?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[derive(Clone)]
struct RedactingMake<M>(M);

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for RedactingMake<M> {
    type Writer = Redacting<M::Writer>;
    fn make_writer(&'a self) -> Self::Writer {
        Redacting(self.0.make_writer())
    }
}

/// Returns a guard that must live as long as the app (flushes on drop).
pub fn init(logs_dir: &Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = || EnvFilter::try_from_env("DEVBRIDGE_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let file =
        std::fs::create_dir_all(logs_dir).ok().map(|_| tracing_appender::rolling::daily(logs_dir, "devbridge.log"));
    let (file_writer, guard) = match file {
        Some(f) => {
            let (w, g) = tracing_appender::non_blocking(f);
            (Some(w), Some(g))
        }
        None => (None, None),
    };
    let stderr = tracing_subscriber::fmt::layer().with_target(false).with_writer(io::stderr).with_filter(filter());
    let registry = tracing_subscriber::registry().with(stderr);
    match file_writer {
        Some(w) => {
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_writer(RedactingMake(w))
                .with_filter(filter());
            let _ = registry.with(layer).try_init();
        }
        None => {
            let _ = registry.try_init();
        }
    }
    guard
}

const CRASH_LOG: &str = "crash.log";

/// Release builds abort on panic, so lines still queued for the non-blocking
/// file writer are lost. Panics are appended to `crash.log` synchronously.
pub fn install_panic_hook(logs_dir: &Path) {
    let path = logs_dir.join(CRASH_LOG);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut at = String::new();
        let _ = SystemTime.format_time(&mut Writer::new(&mut at));
        let thread = std::thread::current();
        let line = format!("{at} thread '{}' {info}\n", thread.name().unwrap_or("<unnamed>"));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(redact(&line).as_bytes());
        }
        default(info);
    }));
}

/// Last `n` lines of today's (or the newest) log file.
pub fn tail(logs_dir: &Path, n: usize) -> Vec<String> {
    let mut files: Vec<_> = std::fs::read_dir(logs_dir)
        .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("devbridge.log")).collect())
        .unwrap_or_default();
    files.sort_by_key(|e| e.file_name());
    let Some(last) = files.last() else { return vec![] };
    last_lines(&last.path(), n)
}

/// Last `n` lines of the crash log.
pub fn crash_tail(logs_dir: &Path, n: usize) -> Vec<String> {
    last_lines(&logs_dir.join(CRASH_LOG), n)
}

fn last_lines(path: &Path, n: usize) -> Vec<String> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<String> = content.lines().map(redact).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}
