//! Phantom state markers for the Session FSM (ADR-3).
//! Sealed: outside code can name the states but never add one.

use crate::session::spawn::HatariProcess;

mod sealed {
    pub trait Sealed {}
}

/// Compile-time session state. `NAME` is the wire value `status` reports.
pub trait SessionState: sealed::Sealed + Send + Sync + 'static {
    const NAME: &'static str;
    /// What a session in this state owns: nothing when Stopped, the live
    /// process when Running/Paused. Makes "Stopped with a live process"
    /// unrepresentable (no `Option`).
    type Data: Send + Sync + std::fmt::Debug;
}

/// No Hatari process exists.
#[derive(Debug)]
pub struct Stopped;

/// Hatari is running (emulation advancing).
#[derive(Debug)]
pub struct Running;

/// Hatari is frozen — the freezer-cartridge "freeze then rip" state.
#[derive(Debug)]
pub struct Paused;

impl sealed::Sealed for Stopped {}
impl sealed::Sealed for Running {}
impl sealed::Sealed for Paused {}

impl SessionState for Stopped {
    const NAME: &'static str = "stopped";
    type Data = ();
}
impl SessionState for Running {
    const NAME: &'static str = "running";
    type Data = HatariProcess;
}
impl SessionState for Paused {
    const NAME: &'static str = "paused";
    type Data = HatariProcess;
}

/// States with a live Hatari process (debug/memory/screenshot are legal).
/// Sealed like `SessionState`; implemented for `Running` and `Paused` only.
pub trait LiveState: SessionState<Data = HatariProcess> {}
impl LiveState for Running {}
impl LiveState for Paused {}
