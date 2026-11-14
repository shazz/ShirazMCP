//! `watch` — a managed one-shot value-change breakpoint (P14 §7.1). Hatari
//! has no bus-access breakpoints (§3.1): this arms `(addr) ! (addr) :once`
//! and reports the writing instruction (best-effort, backward-decoded).

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::managed::{self, Fire};
use super::mem::scan::{hex_encode, parse_addr};
use super::watch_wire::{be_value, cap_output_tail, format_value, watch_condition, WriterOut};
use super::{to_mcp, HatariServer};
use crate::analyze;
use crate::error::Error;
use crate::session::{memory, AnySession};
use crate::types::Addr;

// Re-exported so live integration tests (`tests/re_live.rs`) can call
// `HatariServer::watch` directly with typed args — `watch_wire` itself
// stays `pub(crate)` (analyze.rs/rip_wire.rs convention).
pub use super::watch_wire::{WatchArgs, WatchOn, WatchOutput};

#[tool_router(router = watch_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "watch",
        description = "Watch a memory location and run until its VALUE \
                       CHANGES (managed one-shot; Hatari observes values \
                       not bus cycles — a write of the same value is \
                       invisible, and reads are never observable: use \
                       xrefs for 'who reads this'). Fires ⇒ session ends \
                       PAUSED with the PC that hit — the instruction AFTER \
                       the writer — plus a best-effort decode of the \
                       writing instruction; timeout ⇒ watch disarmed, \
                       session left RUNNING. This answers 'which of N \
                       copies is live / who updates this?' directly. \
                       Emulation runs slower while armed (per-instruction \
                       check). Watching hardware registers whose reads \
                       have side effects (ACIA/MFP data regs) perturbs the \
                       machine; palette/video registers are safe."
    )]
    pub async fn watch(
        &self,
        Parameters(a): Parameters<WatchArgs>,
    ) -> Result<Json<WatchOutput>, McpError> {
        let limit = self.session.lock().await.config().bus_limit();
        let (addr, len) = validate_watch_args(&a, limit)?;
        let timeout_ms = a.timeout_ms.unwrap_or(10_000);
        let mut guard = self.session.lock().await;
        managed::ensure_paused(&mut guard).await.map_err(to_mcp)?;
        let old = read_value(&mut guard, addr, len).await?;
        let condition = watch_condition(addr, len);
        let fire = managed::run_to_breakpoint(&mut guard, &condition, timeout_ms)
            .await
            .map_err(to_mcp)?;
        Ok(Json(build_output(&mut guard, addr, len, old, fire).await?))
    }
}

fn validate_watch_args(a: &WatchArgs, limit: u64) -> Result<(Addr, u32), McpError> {
    let addr = parse_addr(&a.addr)?;
    let len = a.len.unwrap_or(2);
    if ![1, 2, 4].contains(&len) {
        return Err(McpError::invalid_params("len must be 1, 2 or 4", None));
    }
    if len != 1 && addr.raw() % 2 != 0 {
        return Err(McpError::invalid_params(
            "addr must be even for len 2/4",
            None,
        ));
    }
    if u64::from(addr.raw()) + u64::from(len) > limit {
        let msg = if limit == u64::from(Addr::BUS_LEN) {
            "addr + len exceeds the 24-bit ST bus (addr24=false for 32-bit)"
        } else {
            "addr + len exceeds the 32-bit address space"
        };
        return Err(McpError::invalid_params(msg, None));
    }
    Ok((addr, len))
}

async fn read_value(guard: &mut AnySession, addr: Addr, len: u32) -> Result<u32, McpError> {
    let proc = guard
        .process_mut()
        .ok_or_else(|| to_mcp(Error::Session("no live session".to_string())))?;
    let bytes = crate::debug::read_bytes(proc, addr, len as usize)
        .await
        .map_err(to_mcp)?;
    Ok(be_value(&bytes))
}

async fn build_output(
    guard: &mut AnySession,
    addr: Addr,
    len: u32,
    old: u32,
    fire: Fire,
) -> Result<WatchOutput, McpError> {
    match fire {
        Fire::TimedOut => Ok(timed_out_output(addr, len, old)),
        Fire::Fired { pc, output, ours } => {
            fired_output(guard, addr, len, old, pc, output, ours).await
        }
    }
}

fn timed_out_output(addr: Addr, len: u32, old: u32) -> WatchOutput {
    WatchOutput {
        fired: false,
        ours: None,
        addr: addr.to_string(),
        len,
        old_value: format_value(old, len),
        new_value: None,
        pc: None,
        writer: None,
        output: None,
        state: "running".to_string(),
    }
}

async fn fired_output(
    guard: &mut AnySession,
    addr: Addr,
    len: u32,
    old: u32,
    pc: u32,
    output: String,
    ours: bool,
) -> Result<WatchOutput, McpError> {
    let new = read_value(guard, addr, len).await?;
    let writer = backscan_writer(guard, pc, addr, len).await;
    Ok(WatchOutput {
        fired: true,
        ours: Some(ours),
        addr: addr.to_string(),
        len,
        old_value: format_value(old, len),
        new_value: Some(format_value(new, len)),
        pc: Some(Addr::new(pc).to_string()),
        writer,
        output: Some(cap_output_tail(output)),
        state: "paused".to_string(),
    })
}

/// Best-effort backward decode of the instruction that ended at `pc` (§3.1
/// fact 3). Acquisition/decode failure is NOT a call failure — logged and
/// degraded to `None`, same as `writer_at` returning `None` (no writer
/// found, e.g. an interrupt landed on the boundary).
async fn backscan_writer(
    guard: &mut AnySession,
    pc: u32,
    addr: Addr,
    len: u32,
) -> Option<WriterOut> {
    if pc < 10 {
        return None;
    }
    let proc = guard.process_mut()?;
    let mem = match memory::acquire_range(proc, Addr::new(pc - 10), 10).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "watch: writer backscan acquire failed");
            return None;
        }
    };
    analyze::writer_at(&mem, Addr::new(pc), addr, len).map(writer_out)
}

fn writer_out(guess: analyze::WriterGuess) -> WriterOut {
    WriterOut {
        addr: guess.insn.addr.to_string(),
        mnemonic: guess.insn.mnemonic,
        ops: guess.insn.ops,
        bytes_hex: hex_encode(&guess.insn.bytes),
        exact: guess.references_target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(addr: &str, len: Option<u32>) -> WatchArgs {
        WatchArgs {
            addr: addr.to_string(),
            len,
            on: None,
            timeout_ms: None,
        }
    }

    #[test]
    fn watch_range_follows_the_session_bus() {
        let st = u64::from(Addr::BUS_LEN);
        assert!(validate_watch_args(&args("$01000000", Some(4)), st).is_err());
        let ok = validate_watch_args(&args("$01000000", Some(4)), 1 << 32).unwrap();
        assert_eq!(ok.0.raw(), 0x0100_0000);
        let err = validate_watch_args(&args("$FFFFFFFE", Some(4)), 1 << 32).unwrap_err();
        assert!(err.message.contains("32-bit"));
    }
}
