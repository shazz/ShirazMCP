//! ShirazMCP — MCP server driving the Hatari Atari ST emulator.
//!
//! Layering (dependencies point downward only — see ARCHITECTURE.md):
//! `session` → `debug` → `analyze` → `ripper` → `tools`; `assemble` is a
//! leaf (vasm driver) consumed only by `tools`.
//! P1 ships `error`, `types`, `session` (real transitions + fifo + memory),
//! `debug` (typed wrappers + parsers), `analyze` (register annotator),
//! `tools` (lifecycle + debugger + memory).

pub mod analyze;
pub mod assemble;
pub mod audio;
pub mod debug;
pub mod error;
pub mod ripper;
pub mod session;
pub mod tools;
pub mod types;
