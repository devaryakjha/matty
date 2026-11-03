//! Logging: `tracing` events written to a daily log file in the OS log
//! directory, seven days kept, with secrets and message content redacted.
//!
//! Redaction runs on every formatted line before it reaches the file, so it
//! covers events from any crate. It removes access tokens wherever they appear
//! (`Authorization` and `Token` headers, `token=` query parameters) and the
//! values of fields that hold user content, such as a post's text or a file's
//! name. Code that logs user content must put it in one of those fields, never
//! in the message itself. `MATTY_LOG_UNREDACTED=1` turns redaction off, in
//! debug builds only.

use std::borrow::Cow;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{Registry, fmt};

use crate::settings::LogLevel;

/// Log files kept, one per day, including today's.
pub const KEPT_FILES: usize = 7;
/// Log files are named `matty.<yyyy-mm-dd>.log`.
const FILE_PREFIX: &str = "matty";
const FILE_SUFFIX: &str = "log";

/// What replaces a redacted value.
pub const REDACTED: &str = "[redacted]";

/// Fields whose values are user content or secrets. A field whose name ends
/// with one of these (`post_message`, `upload_file_name`) is redacted too.
const SENSITIVE_FIELDS: &[&str] = &[
    "token",
    "password",
    "authorization",
    "message",
    "msg",
    "text",
    "file_name",
    "filename",
];

/// `name=value` pairs as the fmt formatter writes them: a quoted Debug string
/// or a bare value. `message` here is a field the caller named, never the
/// event's own message, which the formatter writes without a key.
static FIELD: LazyLock<Regex> = LazyLock::new(|| {
    let names = SENSITIVE_FIELDS.join("|");
    Regex::new(&format!(
        r#"(?i)\b(\w*(?:{names}))=("(?:[^"\\]|\\.)*"|[^\s,}}]+)"#
    ))
    .expect("valid field pattern")
});

/// Tokens outside a named field: an `Authorization: Bearer …` or `Token: …`
/// header, or a `token=` / `access_token=` query parameter in a URL.
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(\b(?:authorization|token)\s*[:=]\s*"?(?:bearer\s+)?|\bbearer\s+|[?&](?:access_)?token=)[A-Za-z0-9._~+/=-]+"#,
    )
    .expect("valid token pattern")
});

/// Removes tokens and user content from one formatted log line.
pub fn redact(line: &str) -> Cow<'_, str> {
    let line = TOKEN.replace_all(line, |caps: &regex::Captures<'_>| {
        format!("{}{REDACTED}", &caps[1])
    });
    match FIELD.replace_all(&line, |caps: &regex::Captures<'_>| {
        format!("{}={REDACTED}", &caps[1])
    }) {
        Cow::Borrowed(_) => line,
        Cow::Owned(owned) => Cow::Owned(owned),
    }
}

/// Whether redaction is on: always in release builds; in debug builds unless
/// `MATTY_LOG_UNREDACTED=1`.
fn redaction_enabled() -> bool {
    !(cfg!(debug_assertions) && std::env::var_os("MATTY_LOG_UNREDACTED").is_some_and(|v| v == "1"))
}

/// Wraps a [`MakeWriter`] so every line it receives is redacted first.
#[derive(Clone, Debug)]
pub struct Redacting<M> {
    inner: M,
    enabled: bool,
}

impl<M> Redacting<M> {
    /// Redacts lines on their way to `inner` when `enabled`.
    pub fn new(inner: M, enabled: bool) -> Self {
        Self { inner, enabled }
    }
}

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for Redacting<M> {
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter {
            inner: self.inner.make_writer(),
            enabled: self.enabled,
            buffer: Vec::new(),
        }
    }
}

/// Collects one event's output and writes it, redacted, when dropped. The fmt
/// layer makes one writer per event, so a whole line is redacted at once.
pub struct RedactingWriter<W: Write> {
    inner: W,
    enabled: bool,
    buffer: Vec<u8>,
}

impl<W: Write> RedactingWriter<W> {
    fn flush_line(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let buffer = std::mem::take(&mut self.buffer);
        if !self.enabled {
            return self.inner.write_all(&buffer);
        }
        let line = String::from_utf8_lossy(&buffer);
        self.inner.write_all(redact(&line).as_bytes())
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_line()?;
        self.inner.flush()
    }
}

impl<W: Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        // A failed log write has nowhere to be reported.
        let _ = self.flush_line();
    }
}

/// The OS's log directory for Matty: `~/Library/Logs/Matty` on macOS, the
/// local app data folder on Windows, and the XDG state folder on Linux.
pub fn default_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        dirs::home_dir().map(|home| home.join("Library").join("Logs").join("Matty"))
    } else if cfg!(target_os = "windows") {
        dirs::data_local_dir().map(|dir| dir.join("Matty").join("logs"))
    } else {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .map(|dir| dir.join("matty").join("logs"))
    }
}

/// A daily-rotating appender in `dir` that keeps [`KEPT_FILES`] files. Older
/// files are removed when it opens and at each rotation.
pub fn appender(dir: &Path) -> io::Result<RollingFileAppender> {
    std::fs::create_dir_all(dir)?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(FILE_PREFIX)
        .filename_suffix(FILE_SUFFIX)
        .max_log_files(KEPT_FILES)
        .build(dir)
        .map_err(io::Error::other)
}

fn level_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Trace => LevelFilter::TRACE,
    }
}

/// Changes the level of the installed logger.
#[derive(Clone)]
pub struct LevelHandle(reload::Handle<LevelFilter, Registry>);

impl LevelHandle {
    /// Applies `level` to every event from now on.
    pub fn set(&self, level: LogLevel) {
        // Fails only if the subscriber is gone, when there is nothing to change.
        let _ = self.0.reload(level_filter(level));
    }
}

/// Installs the global logger writing to `dir` at `level`. Returns `None`, and
/// logs to stderr instead, when the directory cannot be used; returns `None`
/// without changing anything when a logger is already installed.
pub fn init(dir: Option<&Path>, level: LogLevel) -> Option<LevelHandle> {
    let (filter, handle) = reload::Layer::new(level_filter(level));
    let enabled = redaction_enabled();
    let file = dir.map(|dir| appender(dir).map_err(|error| (dir, error)));
    let (file, failure) = match file {
        Some(Ok(file)) => (Some(file), None),
        Some(Err((dir, error))) => (None, Some(format!("{}: {error}", dir.display()))),
        None => (None, Some("no log directory on this system".to_owned())),
    };
    let to_file = file.map(|file| {
        fmt::layer()
            .with_ansi(false)
            .with_writer(Redacting::new(file, enabled))
    });
    let to_stderr = failure.is_some().then(|| {
        fmt::layer()
            .with_ansi(false)
            .with_writer(Redacting::new(io::stderr, enabled))
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(to_file)
        .with(to_stderr)
        .try_init()
        .ok()?;
    if let Some(failure) = failure {
        tracing::warn!("logging to stderr; could not open the log directory ({failure})");
    }
    if !enabled {
        tracing::warn!("log redaction is off (MATTY_LOG_UNREDACTED=1)");
    }
    Some(LevelHandle(handle))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A writer target the tests can read back.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Self;
        fn make_writer(&'a self) -> Self {
            self.clone()
        }
    }

    fn capture(enabled: bool, log: impl FnOnce()) -> String {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(Redacting::new(captured.clone(), enabled)),
        );
        tracing::subscriber::with_default(subscriber, log);
        captured.text()
    }

    #[test]
    fn redacts_token_message_and_file_name_in_a_logged_line() {
        let text = capture(true, || {
            tracing::info!(
                authorization = "Bearer s3cr3t-t0ken",
                message_text = "lunch at noon?",
                file_name = "salaries 2026.xlsx",
                channel_id = "abc123",
                "sending post"
            );
        });
        assert_eq!(text.lines().count(), 1, "{text}");
        for secret in ["s3cr3t-t0ken", "lunch at noon", "salaries"] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert!(text.contains("authorization=[redacted]"), "{text}");
        assert!(text.contains("message_text=[redacted]"), "{text}");
        assert!(text.contains("file_name=[redacted]"), "{text}");
        assert!(text.contains("channel_id=\"abc123\""), "{text}");
        assert!(text.contains("sending post"), "{text}");
    }

    #[test]
    fn redacts_tokens_inside_free_text() {
        let text = capture(true, || {
            tracing::warn!(
                "GET wss://chat.example.com/api/v4/websocket?connection_id=c1&token=abcDEF123 \
                 with header Authorization: Bearer xyz.789 and Token: plain456"
            );
        });
        for secret in ["abcDEF123", "xyz.789", "plain456"] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert!(text.contains("connection_id=c1"), "{text}");
        assert!(text.contains("&token=[redacted]"), "{text}");
    }

    #[test]
    fn keeps_ordinary_fields_and_the_event_message() {
        let line = "2026-11-03T10:00:00Z  INFO matty: connected server=\"https://x\" attempt=3";
        assert_eq!(redact(line), line);
    }

    #[test]
    fn handles_escaped_quotes_in_redacted_values() {
        let line = r#"INFO post msg="she said \"hi\" twice" id=1"#;
        assert_eq!(redact(line), "INFO post msg=[redacted] id=1");
    }

    #[test]
    fn writes_unredacted_when_disabled() {
        let text = capture(false, || tracing::info!(msg = "visible", "debugging"));
        assert!(text.contains("msg=\"visible\""), "{text}");
    }

    #[test]
    fn rotation_keeps_seven_files() {
        let dir = tempfile::tempdir().unwrap();
        // Ten old daily files, created oldest first.
        let old: Vec<String> = (1..=10)
            .map(|day| format!("{FILE_PREFIX}.2020-01-{day:02}.{FILE_SUFFIX}"))
            .collect();
        for name in &old {
            std::fs::write(dir.path().join(name), "old\n").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        // Files that are not logs stay.
        std::fs::write(dir.path().join("notes.txt"), "keep").unwrap();

        let mut appender = appender(dir.path()).unwrap();
        appender.write_all(b"today\n").unwrap();
        appender.flush().unwrap();

        let mut logs: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with(FILE_PREFIX))
            .collect();
        logs.sort();
        assert_eq!(logs.len(), KEPT_FILES, "{logs:?}");
        // The four oldest went; the six newest and today's remain.
        assert!(logs[..6].iter().eq(old[4..].iter()), "{logs:?}");
        assert!(
            !logs[6].starts_with(&format!("{FILE_PREFIX}.2020")),
            "{logs:?}"
        );
        assert!(dir.path().join("notes.txt").exists());
        let today = std::fs::read_to_string(dir.path().join(&logs[6])).unwrap();
        assert_eq!(today, "today\n");
    }

    #[test]
    fn default_dir_is_named_for_matty() {
        if let Some(dir) = default_dir() {
            let text = dir.to_string_lossy().to_lowercase();
            assert!(text.contains("matty"), "{text}");
        }
    }
}
