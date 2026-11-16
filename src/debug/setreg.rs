//! Register WRITES via Hatari's `r reg=value` (`debugcpu.c`
//! `DebugCpu_Register`) — the write-side sibling of `registers()`
//! (`cpureg`). One assignment per command: Hatari only reads `psArgs[1]`.

use crate::debug::DebugChannel;
use crate::error::{Error, Result};

/// `r {name}=${value:x}`. The `$` is mandatory: the value goes through
/// `Eval_Number`, whose default base is DECIMAL (the `memwrite` trap,
/// `debug::write_memwrite_line`). Success prints nothing; a bad name or
/// value prints `"\tError, usage: r or r xx=yyyy"` (title case — scanned
/// case-insensitively, like `memwrite`).
pub async fn set_register(chan: &mut impl DebugChannel, name: &str, value: u32) -> Result<()> {
    let cmd = format!("r {name}=${value:x}");
    let out = chan.debug(&cmd).await?;
    match out
        .lines()
        .find(|l| l.to_ascii_uppercase().contains("ERROR"))
    {
        Some(line) => Err(Error::Parse {
            context: cmd,
            input: line.trim().to_string(),
        }),
        None => Ok(()),
    }
}

/// The raw 16-bit SR (`evaluate sr`) — `Registers` only carries the
/// decoded flags, and a restore needs the exact word.
pub async fn read_sr(chan: &mut impl DebugChannel) -> Result<u16> {
    let v = crate::debug::evaluate(chan, "sr").await?;
    u16::try_from(v).map_err(|_| Error::Parse {
        context: "evaluate sr".to_string(),
        input: format!("${v:x}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        responses: Vec<String>,
        sent: Vec<String>,
    }

    impl DebugChannel for Fake {
        async fn debug(&mut self, cmd: &str) -> Result<String> {
            self.sent.push(cmd.to_string());
            Ok(self.responses.remove(0))
        }
    }

    #[tokio::test]
    async fn set_register_formats_hex_with_dollar() {
        let mut fake = Fake {
            responses: vec![String::new()],
            sent: vec![],
        };
        set_register(&mut fake, "a7", 0x7_FFFE).await.unwrap();
        assert_eq!(fake.sent, vec!["r a7=$7fffe".to_string()]);
    }

    #[tokio::test]
    async fn set_register_usage_error_fails() {
        let usage = "\tError, usage: r or r xx=yyyy\n\tWhere: xx=A0-A7, D0-D7, PC, SR, ISP, USP";
        let mut fake = Fake {
            responses: vec![usage.to_string()],
            sent: vec![],
        };
        let err = set_register(&mut fake, "q9", 1).await.unwrap_err();
        assert!(matches!(err, Error::Parse { .. }));
    }

    #[tokio::test]
    async fn read_sr_parses_evaluate() {
        let mut fake = Fake {
            responses: vec!["= %10011100000000 (bin), #9984 (dec), $2700 (hex)".to_string()],
            sent: vec![],
        };
        assert_eq!(read_sr(&mut fake).await.unwrap(), 0x2700);
        assert_eq!(fake.sent, vec!["evaluate sr".to_string()]);
    }
}
