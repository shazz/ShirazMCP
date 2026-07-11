//! ADR-3: `Session<Stopped>` must not expose `pause()`.
use shiraz_mcp::session::{Session, SessionConfig};

fn main() {
    let s = Session::new(SessionConfig::default()); // Session<Stopped>
    let _ = s.pause();
}
