//! File logger behind the `log` crate facade (port of `TADNLog.cs`): `active.log` in the data
//! folder, rolled to `old.log` once it reaches 10 MB or is 7 days old. Every crate logs through
//! `log::{error, warn, info, debug}!`.
//!
//! Lines are `[yyyy-MM-dd HH:mm:ss.fff pid=<pid> tid=<thread>] <message>\n` (local time, UTF-8
//! without BOM). Warnings and errors carry a `WARN: `/`ERROR: ` prefix, debug/trace lines (debug
//! builds only) `DEBUG: `/`TRACE: `. Lines are buffered and appended every 2 s by a background
//! thread, on [`flush`], and before a panic is reported. Writes go through an append-only handle
//! shared for read/write/delete, retried 3 times 20 ms apart; a failed batch stays queued (at most
//! 10 000 lines, the oldest dropped and counted).

use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::os::windows::fs::{FileTimesExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

const ACTIVE_NAME: &str = "active.log";
const OLD_NAME: &str = "old.log";
/// `TADNLogFile.MaxBytes`.
const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// `TimeConstants.LogMaxAgeMs`.
const LOG_MAX_AGE_MS: u64 = 604_800_000;
/// `TimeConstants.LogFlushIntervalMs`.
const LOG_FLUSH_INTERVAL_MS: u64 = 2_000;
/// `TimeConstants.LogWriteRetryDelayMs`.
const LOG_WRITE_RETRY_DELAY_MS: u64 = 20;
/// `TADNLogFile.WriteAttempts`.
const WRITE_ATTEMPTS: u32 = 3;
/// `TADNLogFile.MaxPendingLines`.
const MAX_PENDING_LINES: usize = 10_000;

struct Buffer {
    lines: Vec<String>,
    dropped: usize,
}

struct LogFile {
    active: PathBuf,
    old: PathBuf,
    buffer: Mutex<Buffer>,
    /// Serializes flushes so batches never interleave.
    flush_gate: Mutex<()>,
}

struct Logger {
    file: LogFile,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Messages recorded before [`init`] (data-folder setup runs before logging starts); written
/// first once the logger exists.
static EARLY: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn lock<V>(mutex: &Mutex<V>) -> MutexGuard<'_, V> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Queues a message for the log; before [`init`] it is kept and written once logging starts.
pub(crate) fn early(message: String) {
    match LOGGER.get() {
        Some(logger) => logger.file.enqueue(format_line(&message)),
        None => lock(&EARLY).push(format_line(&message)),
    }
}

/// Installs the global logger writing `<dir>\active.log`. Idempotent.
pub fn init(dir: &Path) -> io::Result<()> {
    if LOGGER.get().is_some() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let logger = Logger {
        file: LogFile {
            active: dir.join(ACTIVE_NAME),
            old: dir.join(OLD_NAME),
            buffer: Mutex::new(Buffer { lines: Vec::with_capacity(64), dropped: 0 }),
            flush_gate: Mutex::new(()),
        },
    };
    if LOGGER.set(logger).is_err() {
        return Ok(());
    }
    let Some(logger) = LOGGER.get() else { return Ok(()) };
    for line in std::mem::take(&mut *lock(&EARLY)) {
        logger.file.enqueue(line);
    }
    log::set_logger(logger).map_err(|error| io::Error::other(error.to_string()))?;
    log::set_max_level(level_from_environment().unwrap_or(if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    }));
    std::thread::Builder::new().name("log-flush".to_owned()).spawn(|| {
        loop {
            std::thread::sleep(Duration::from_millis(LOG_FLUSH_INTERVAL_MS));
            flush();
        }
    })?;
    Ok(())
}

/// Environment variable that overrides the level of this app's own log lines (`error`, `warn`,
/// `info`, `debug`, `trace`), e.g. `trace` to log every tray callback while diagnosing input.
pub const LOG_LEVEL_ENVIRONMENT_VARIABLE: &str = "TADN_LOG_LEVEL";

fn level_from_environment() -> Option<log::LevelFilter> {
    let value = std::env::var(LOG_LEVEL_ENVIRONMENT_VARIABLE).ok()?;
    value.trim().parse().ok()
}

/// Installs a panic hook that logs the panic message, location and backtrace, then flushes.
/// The previously installed hook runs afterwards.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_owned());
        let location =
            info.location().map_or_else(String::new, |l| format!(" at {}:{}:{}", l.file(), l.line(), l.column()));
        let thread = std::thread::current();
        let backtrace = std::backtrace::Backtrace::force_capture();
        early(format!(
            "FATAL panic on thread '{}': {message}{location}\n{backtrace}",
            thread.name().unwrap_or("<unnamed>")
        ));
        flush();
        previous(info);
    }));
}

/// Flushes buffered log lines to disk (call at shutdown).
pub fn flush() {
    if let Some(logger) = LOGGER.get() {
        logger.file.flush();
    }
}

/// `[yyyy-MM-dd HH:mm:ss.fff pid=<pid> tid=<thread>] <message>\n` in local time.
fn format_line(message: &str) -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    // SAFETY: GetLocalTime and GetCurrentThreadId have no preconditions.
    let (time, tid) = unsafe { (GetLocalTime(), GetCurrentThreadId()) };
    format!(
        "[{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} pid={} tid={tid}] {message}\n",
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond,
        time.wMilliseconds,
        std::process::id(),
    )
}

impl log::Log for Logger {
    /// Info and up from the TADN crates and the app binary (crate `VolumeTrayAppDotNET`, library
    /// `volume_tray_app_dotnet`); dependencies (winit, Slint, Skia) only from Warn up.
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        let target = metadata.target();
        let ours = target.starts_with("tadn_")
            || target.starts_with("vtadn_")
            || target.starts_with("volume_tray_app_dotnet")
            || target.starts_with("VolumeTrayAppDotNET");
        metadata.level() <= if ours { log::max_level() } else { log::LevelFilter::Warn }
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = match record.level() {
            log::Level::Error => format!("ERROR: {}", record.args()),
            log::Level::Warn => format!("WARN: {}", record.args()),
            log::Level::Info => record.args().to_string(),
            log::Level::Debug => format!("DEBUG: {}", record.args()),
            log::Level::Trace => format!("TRACE: {}", record.args()),
        };
        self.file.enqueue(format_line(&message));
    }

    fn flush(&self) {
        self.file.flush();
    }
}

impl LogFile {
    fn enqueue(&self, line: String) {
        let mut buffer = lock(&self.buffer);
        buffer.lines.push(line);
        trim_backlog(&mut buffer);
    }

    /// Writes every pending line; on failure the batch stays pending for the next flush.
    fn flush(&self) {
        let _turn = lock(&self.flush_gate);
        let (mut batch, dropped) = {
            let mut buffer = lock(&self.buffer);
            if buffer.lines.is_empty() {
                return;
            }
            let dropped = std::mem::take(&mut buffer.dropped);
            (std::mem::replace(&mut buffer.lines, Vec::with_capacity(64)), dropped)
        };
        if dropped > 0 {
            batch
                .insert(0, format_line(&format!("TADNLog: {dropped} lines dropped while the log file was unwritable")));
        }
        if self.try_write(&batch) {
            return;
        }
        let mut buffer = lock(&self.buffer);
        // Older lines go back in front of anything logged during the failed write.
        batch.append(&mut buffer.lines);
        buffer.lines = batch;
        trim_backlog(&mut buffer);
    }

    fn try_write(&self, batch: &[String]) -> bool {
        let bytes: Vec<u8> = batch.concat().into_bytes();
        for attempt in 1..=WRITE_ATTEMPTS {
            self.ensure_rotated();
            if self.append_all(&bytes).is_ok() {
                return true;
            }
            if attempt < WRITE_ATTEMPTS {
                std::thread::sleep(Duration::from_millis(LOG_WRITE_RETRY_DELAY_MS));
            }
        }
        false
    }

    /// Append-only handle (`FILE_APPEND_DATA`), shared read/write/delete, `OPEN_ALWAYS`.
    fn append_all(&self, bytes: &[u8]) -> io::Result<()> {
        use windows::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE};
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
            .open(&self.active)?;
        file.write_all(bytes)
    }

    /// Rolls `active.log` to `old.log` when it is at least 10 MB or 7 days old, then recreates
    /// `active.log` with its creation time pinned to now.
    fn ensure_rotated(&self) {
        let Ok(metadata) = std::fs::metadata(&self.active) else { return };
        let size_over = metadata.len() >= MAX_BYTES;
        let age_over = metadata
            .created()
            .ok()
            .and_then(|created| SystemTime::now().duration_since(created).ok())
            .is_some_and(|age| age.as_millis() >= u128::from(LOG_MAX_AGE_MS));
        if !size_over && !age_over {
            return;
        }
        if self.old.exists() {
            let _ = std::fs::remove_file(&self.old);
        }
        if std::fs::rename(&self.active, &self.old).is_err() {
            return;
        }
        if let Ok(file) = File::create(&self.active) {
            let _ = file.set_times(std::fs::FileTimes::new().set_created(SystemTime::now()));
        }
    }
}

fn trim_backlog(buffer: &mut Buffer) {
    let excess = buffer.lines.len().saturating_sub(MAX_PENDING_LINES);
    if excess > 0 {
        buffer.lines.drain(..excess);
        buffer.dropped += excess;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_format_matches_tadnlog() {
        let line = format_line("hello");
        assert!(line.starts_with('['));
        assert!(line.ends_with("] hello\n"));
        let header = &line[1..line.find(']').unwrap_or(0)];
        // yyyy-MM-dd HH:mm:ss.fff
        assert_eq!(header.as_bytes()[4], b'-');
        assert_eq!(header.as_bytes()[10], b' ');
        assert_eq!(header.as_bytes()[19], b'.');
        assert!(header[24..].starts_with(&format!("pid={} tid=", std::process::id())));
    }

    #[test]
    fn backlog_keeps_newest_lines_and_counts_dropped() {
        let mut buffer = Buffer { lines: (0..MAX_PENDING_LINES + 5).map(|i| i.to_string()).collect(), dropped: 0 };
        trim_backlog(&mut buffer);
        assert_eq!(buffer.lines.len(), MAX_PENDING_LINES);
        assert_eq!(buffer.dropped, 5);
        assert_eq!(buffer.lines[0], "5");
    }

    #[test]
    fn rotation_moves_large_active_log_to_old() {
        let dir = std::env::temp_dir().join(format!("tadn-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let file = LogFile {
            active: dir.join(ACTIVE_NAME),
            old: dir.join(OLD_NAME),
            buffer: Mutex::new(Buffer { lines: Vec::new(), dropped: 0 }),
            flush_gate: Mutex::new(()),
        };
        std::fs::write(&file.active, vec![b'x'; MAX_BYTES as usize]).unwrap_or_default();
        file.enqueue("next\n".to_owned());
        file.flush();
        assert_eq!(std::fs::metadata(&file.old).map(|m| m.len()).unwrap_or(0), MAX_BYTES);
        assert_eq!(std::fs::read_to_string(&file.active).unwrap_or_default(), "next\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
