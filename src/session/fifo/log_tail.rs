//! `LogTail`: polling tail of the Hatari log file, split out of `fifo.rs`
//! (§3 file-size cleanup) — no behavior change from the pre-split module.

use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::error::{Error, Result};

/// Poll cadence for `LogTail::read_until` — the only concession to polling
/// in this module; bounded by the caller's deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Tails the Hatari log file by polling (no inotify dependency): remembers
/// a byte cursor, reads only NEW bytes each pass. The cursor is what makes
/// a FIXED marker safe across repeated calls — old sentinel echoes are
/// behind the cursor and can never satisfy a new command.
#[derive(Debug)]
pub struct LogTail {
    path: PathBuf,
    cursor: u64,
    pending: String,
}

impl LogTail {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cursor: 0,
            pending: String::new(),
        }
    }

    /// Read from the cursor until a line satisfying `is_marker` appears,
    /// appending every COMPLETE line seen before the marker into `collected`
    /// (EXCLUDING the marker line itself); any bytes read past the marker
    /// in the same poll stay queued in `pending` for the next call.
    ///
    /// `collected` is an out-parameter rather than a return value so a
    /// caller retrying the marker write (see `read_sentinel_with_retry`)
    /// can carry accumulated lines across attempts instead of losing
    /// whatever this call already gathered when it times out.
    pub async fn read_until(
        &mut self,
        is_marker: impl Fn(&str) -> bool,
        what: &str,
        timeout_ms: u64,
        collected: &mut Vec<String>,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let chunk = self.read_new_bytes().await?;
            self.pending.push_str(&chunk);
            while let Some(nl) = self.pending.find('\n') {
                let line = self.pending[..nl].to_string();
                self.pending.drain(..=nl);
                if is_marker(&line) {
                    return Ok(());
                }
                collected.push(line);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout {
                    what: what.to_string(),
                    waited_ms: timeout_ms,
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Reopen-and-seek each poll: at 25 ms cadence this is cheap and dodges
    /// log rotation/truncation edge cases that a held-open handle would hit.
    async fn read_new_bytes(&mut self) -> Result<String> {
        let ctx = || format!("tail log {}", self.path.display());
        let mut file = tokio::fs::File::open(&self.path)
            .await
            .map_err(|e| Error::Fifo {
                context: ctx(),
                source: e,
            })?;
        file.seek(std::io::SeekFrom::Start(self.cursor))
            .await
            .map_err(|e| Error::Fifo {
                context: ctx(),
                source: e,
            })?;
        let mut buf = String::new();
        let n = file
            .read_to_string(&mut buf)
            .await
            .map_err(|e| Error::Fifo {
                context: ctx(),
                source: e,
            })?;
        self.cursor += n as u64;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::super::sentinel;
    use super::*;

    /// Generic marker for tests that only exercise `LogTail` plumbing, not
    /// per-call uniqueness (see `sentinel.rs`'s own tests for that).
    const MARKER: &str = "= %1 (bin), $dead0001 (hex)\n";

    fn any_sentinel_line(line: &str) -> bool {
        sentinel::is_sentinel_family_line(line)
    }

    async fn write_file(path: &std::path::Path, contents: &str) {
        tokio::fs::write(path, contents).await.unwrap();
    }

    #[tokio::test]
    async fn log_tail_returns_lines_before_marker() {
        let dir = tempdir();
        let path = dir.join("log.txt");
        write_file(&path, &format!("hello\nworld\n{MARKER}")).await;

        let mut tail = LogTail::new(path);
        let mut lines = Vec::new();
        tail.read_until(any_sentinel_line, "test", 1_000, &mut lines)
            .await
            .unwrap();
        assert_eq!(lines, vec!["hello".to_string(), "world".to_string()]);
    }

    #[tokio::test]
    async fn log_tail_cursor_survives_repeat() {
        let dir = tempdir();
        let path = dir.join("log.txt");
        write_file(&path, &format!("first\n{MARKER}")).await;

        let mut tail = LogTail::new(path.clone());
        let mut first = Vec::new();
        tail.read_until(any_sentinel_line, "test", 1_000, &mut first)
            .await
            .unwrap();
        assert_eq!(first, vec!["first".to_string()]);

        let mut contents = tokio::fs::read_to_string(&path).await.unwrap();
        contents.push_str(&format!("second\n{MARKER}"));
        write_file(&path, &contents).await;

        let mut second = Vec::new();
        tail.read_until(any_sentinel_line, "test", 1_000, &mut second)
            .await
            .unwrap();
        assert_eq!(second, vec!["second".to_string()]);
    }

    /// Regression for the freeze-drain fix (ADR-21): per `read_until`'s own
    /// doc, text arriving in the same poll as — but after — a marker line
    /// stays queued in `pending` for the next call instead of being
    /// returned. Two DISTINCT sentinel values (one per call, matching
    /// production) all land in ONE chunk: marker 1 (a confirm), a stray
    /// banner line, marker 2 (a drain). Proves `drain_stdin_entry_text`'s
    /// mechanism: an UNdrained caller would have the banner text silently
    /// prepended to whatever it reads next; a second `read_until` (the
    /// drain) instead gets it back cleanly as ITS OWN `collected`.
    #[tokio::test]
    async fn log_tail_second_read_until_drains_stray_pending_text() {
        let dir = tempdir();
        let path = dir.join("log.txt");
        let marker1 = "= %1 (bin), $dead0001 (hex)\n";
        let marker2 = "= %1 (bin), $dead0002 (hex)\n";
        write_file(
            &path,
            &format!("entered debug mode\n{marker1}reading symbols from PRG...\n{marker2}"),
        )
        .await;

        let mut tail = LogTail::new(path);
        let is_marker1 = |l: &str| sentinel::is_this_sentinel_line(l, 0xDEAD_0001);
        let is_marker2 = |l: &str| sentinel::is_this_sentinel_line(l, 0xDEAD_0002);

        let mut confirm = Vec::new();
        tail.read_until(is_marker1, "confirm", 1_000, &mut confirm)
            .await
            .unwrap();
        assert_eq!(confirm, vec!["entered debug mode".to_string()]);

        let mut drained = Vec::new();
        tail.read_until(is_marker2, "drain", 1_000, &mut drained)
            .await
            .unwrap();
        assert_eq!(drained, vec!["reading symbols from PRG...".to_string()]);
    }

    #[tokio::test]
    async fn log_tail_times_out() {
        let dir = tempdir();
        let path = dir.join("log.txt");
        write_file(&path, "no marker here\n").await;

        let mut tail = LogTail::new(path);
        let mut collected = Vec::new();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            tail.read_until(any_sentinel_line, "test", 100, &mut collected),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(Error::Timeout { waited_ms: 100, .. })));
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shiraz-mcp-fifo-test-{}", uid()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn uid() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }
}
