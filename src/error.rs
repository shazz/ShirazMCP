use crate::types::PluginId;

/// Crate-wide result alias. All `src/` library code returns this; `anyhow`
/// appears only in `main.rs`.
pub type Result<T> = std::result::Result<T, Error>;

/// The matchable error taxonomy (ADR-1). Categories, not a grab bag:
/// callers branch on the variant, humans read the message.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Session lifecycle misuse or failure (bad state, spawn failure, …).
    #[error("session error: {0}")]
    Session(String),

    /// I/O on the Hatari command fifo or its log. Not `#[from]`: fifo
    /// errors must always carry call-site context.
    #[error("fifo I/O ({context}): {source}")]
    Fifo {
        context: String,
        #[source]
        source: std::io::Error,
    },

    /// The sentinel echo never appeared in the log (ADR-7).
    #[error("timeout after {waited_ms} ms waiting for: {what}")]
    Timeout { what: String, waited_ms: u64 },

    /// Hatari log output did not match the expected shape.
    #[error("parse error ({context}): unexpected input {input:?}")]
    Parse { context: String, input: String },

    /// A ripper plugin failed (not "found nothing" — that is an empty Vec).
    #[error("ripper plugin '{plugin}': {message}")]
    Ripper { plugin: PluginId, message: String },

    /// The external assembler (vasm) could not run: missing binary, spawn or
    /// temp-file I/O failure, timeout, oversized output. Diagnostics in the
    /// *user's source* are data (`AsmResult`), never this error.
    #[error("assembler unavailable ({context}): {message}")]
    Assembler { context: String, message: String },

    /// Uncontextualized I/O at the app fringe. Prefer `Fifo`/`Session` with
    /// context in library code; this `#[from]` exists for `?` at boundaries.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn session_display() {
        let e = Error::Session("start is deferred to P1".to_string());
        assert_eq!(e.to_string(), "session error: start is deferred to P1");
    }

    #[test]
    fn fifo_display() {
        let e = Error::Fifo {
            context: "write cmd".to_string(),
            source: io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed"),
        };
        assert_eq!(e.to_string(), "fifo I/O (write cmd): pipe closed");
    }

    #[test]
    fn timeout_display() {
        let e = Error::Timeout {
            what: "sentinel echo".to_string(),
            waited_ms: 5000,
        };
        assert_eq!(
            e.to_string(),
            "timeout after 5000 ms waiting for: sentinel echo"
        );
    }

    #[test]
    fn parse_display() {
        let e = Error::Parse {
            context: "cpureg".to_string(),
            input: "???".to_string(),
        };
        assert_eq!(
            e.to_string(),
            "parse error (cpureg): unexpected input \"???\""
        );
    }

    #[test]
    fn ripper_display() {
        let e = Error::Ripper {
            plugin: PluginId::new("pack-ice"),
            message: "truncated header".to_string(),
        };
        assert_eq!(e.to_string(), "ripper plugin 'pack-ice': truncated header");
    }

    #[test]
    fn assembler_display() {
        let e = Error::Assembler {
            context: "spawn".to_string(),
            message: "vasm not found".to_string(),
        };
        assert_eq!(
            e.to_string(),
            "assembler unavailable (spawn): vasm not found"
        );
    }

    #[test]
    fn io_display_and_from() {
        let e: Error = io::Error::new(io::ErrorKind::NotFound, "gone").into();
        assert!(matches!(e, Error::Io(_)));
        assert_eq!(e.to_string(), "I/O error: gone");
    }
}
