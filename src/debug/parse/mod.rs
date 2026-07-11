//! Pure parsers for Hatari debugger log output. Every function takes `&str`
//! and returns `Result<T>`; failures are always `Error::Parse`. No regex —
//! split/trim/`from_str_radix` only. Every function is pinned to lines in
//! `tests/fixtures/hatari_session.log`. Split (§3 file-size cleanup) by
//! family: `cpureg.rs` (register block), `values.rs` (numeric extraction:
//! `evaluate`/`m`), `status.rs` (outcome lines: `symbols`/`screenshot`) —
//! this file keeps only the shared error helper, the test-fixture slicer,
//! and re-exports.

mod cpureg;
mod info_mfp;
mod info_ym;
mod status;
mod values;

use crate::error::Error;

pub use cpureg::parse_cpureg;
pub use info_mfp::{parse_info_mfp, MfpRegs};
pub use info_ym::parse_info_ym;
pub use status::{parse_screenshot, parse_symbols};
pub use values::{parse_evaluate, parse_memdump};

pub(super) fn parse_err(context: &str, input: impl Into<String>) -> Error {
    Error::Parse {
        context: context.to_string(),
        input: input.into(),
    }
}

/// Test-only support shared with `debug::mod`'s canned-channel tests and
/// this module's children: slice the block of output belonging to one
/// `> <cmd>` echo out of the fixture.
#[cfg(test)]
pub(crate) fn fixture_block(fixture: &str, cmd: &str) -> String {
    let echo = format!("> {cmd}");
    let start = fixture.find(&echo).map(|i| i + echo.len()).unwrap_or(0);
    let rest = &fixture[start..];
    let end = rest.find("\n> ").unwrap_or(rest.len());
    rest[..end].trim_start_matches('\n').to_string()
}
