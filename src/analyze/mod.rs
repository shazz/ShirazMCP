//! Analysis over a `MemImage` / debugger output. Pure functions over
//! `crate::types` only — never `session` or `debug` (the testability seam
//! and layering contract).

pub mod annotate;
pub mod cheat_set;
pub mod cheatscan;
pub mod cycles;
pub mod disasm;
pub mod dmasnd;
pub mod map;
pub mod xrefs;
pub mod xrefs_ops;
pub mod xrefs_scan;
pub mod ym;

pub use annotate::{annotate, Annotation, PsgSelect};
pub use cheat_set::CandidateSet;
pub use cheatscan::{filter, read_value, seed, Predicate, SeedFilter, Width};
pub use disasm::{disasm, Insn};
pub use xrefs::{writer_at, xrefs, RefAccess, RefMode, WriterGuess, Xref, XrefScan};
