//! ADR-3: `Session<Running>` must not expose `start()`.
use shiraz_mcp::session::{Running, Session};

fn misuse(s: Session<Running>) {
    let _ = s.start();
}

fn main() {}
