//! `count_hits`: exact execution counts for up to 16 PCs over N VBL frames
//! (ADR-47). One `:trace` breakpoint per PC (prints per hit, never stops),
//! one managed N-frame leg, then Hatari's own cumulative "matched N times"
//! counter is read from the run's output — never a line grep, which echoes
//! and listings inflate. Our breakpoints are removed on every exit;
//! pre-existing ones are left alone.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::count_hits_wire::{build_output, check_pcs, condition, indices_to_remove, validate};
use super::frames;
use super::managed;
use super::profile_frame::{frames_leg, live_proc};
use super::resolve::resolve_addr;
use super::{to_mcp, AnySession, HatariServer};
use crate::debug::breakpoint::{self as bp, Armed};
use crate::debug::{trace, DebugChannel};
use crate::types::Addr;

pub use super::count_hits_wire::{CountHitsArgs, CountHitsOutput, PcHits};

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = count_hits_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "count_hits",
        description = "Count exactly how many times each of 1..=16 PCs \
                       executes over N VBL frames (1..=500): arms one \
                       non-stopping :trace breakpoint per PC, runs exactly \
                       N frames, and reads Hatari's own per-breakpoint \
                       'matched N times' counter (immune to the echo/ \
                       listing lines that inflate a grep of hatari.log). \
                       Returns hits, hits_per_frame and rate_hz (hits/ \
                       frames × 50) per PC plus the VBL delta. Its \
                       breakpoints are removed afterwards; yours stay. \
                       Very hot PCs log every hit and slow emulation — \
                       raise timeout_ms. Leaves the session PAUSED at a \
                       frame boundary."
    )]
    pub async fn count_hits(
        &self,
        Parameters(a): Parameters<CountHitsArgs>,
    ) -> Result<Json<CountHitsOutput>, McpError> {
        validate(&a)?;
        let timeout = a
            .timeout_ms
            .unwrap_or_else(|| frames::default_timeout(a.frames));
        let mut guard = self.session.lock().await;
        managed::ensure_paused(&mut guard).await.map_err(to_mcp)?;
        let pcs = resolve_pcs(&mut guard, &a.pcs).await?;
        let vbl_before = frames::current_vbl(&mut guard).await?;
        frames::check_vbl_headroom(vbl_before, a.frames)?;
        let armed = arm_all(live_proc(&mut guard)?, &pcs).await?;
        let output = run_and_disarm(&mut guard, a.frames, timeout, &armed).await?;
        let vbl_after = frames::current_vbl(&mut guard).await?;
        let hits: Vec<u64> = armed
            .iter()
            .map(|a| trace::trace_hits(&output, a.index, &a.expression))
            .collect();
        Ok(Json(build_output(
            a.frames, vbl_before, vbl_after, &pcs, &hits,
        )))
    }
}

async fn resolve_pcs(guard: &mut AnySession, pcs: &[String]) -> Result<Vec<Addr>, McpError> {
    let proc = live_proc(guard)?;
    let mut out = Vec::with_capacity(pcs.len());
    for s in pcs {
        out.push(resolve_addr(proc, s).await?);
    }
    check_pcs(&out)?;
    Ok(out)
}

/// The N-frame leg, then ALWAYS remove our breakpoints — on success, on a
/// timeout (session Running) and on a foreign stop alike. A leg error wins
/// over a cleanup error, but a cleanup failure after success is an error.
async fn run_and_disarm(
    guard: &mut AnySession,
    n: u32,
    timeout: u64,
    armed: &[Armed],
) -> Result<String, McpError> {
    let leg = frames_leg(guard, n, timeout).await;
    let cleanup = remove_ours(live_proc(guard)?, armed).await;
    let output = leg?;
    cleanup.map_err(|e| {
        McpError::internal_error(
            format!(
                "{}; count_hits trace breakpoints may still be armed — \
                 breakpoint(action=list) and remove them",
                to_mcp(e).message
            ),
            None,
        )
    })?;
    Ok(output)
}

/// Arm one trace breakpoint per PC; returns Hatari's index + normalized
/// expression for each (request order). Any arm failure removes the ones
/// already armed before returning the error.
async fn arm_all(chan: &mut impl DebugChannel, pcs: &[Addr]) -> Result<Vec<Armed>, McpError> {
    let mut armed = Vec::with_capacity(pcs.len());
    for &pc in pcs {
        match trace::arm_trace(chan, &condition(pc)).await {
            Ok(a) => armed.push(a),
            Err(e) => {
                let _ = remove_ours(chan, &armed).await; // best-effort; arm error wins
                return Err(to_mcp(e));
            }
        }
    }
    Ok(armed)
}

async fn remove_ours(chan: &mut impl DebugChannel, armed: &[Armed]) -> crate::error::Result<()> {
    if armed.is_empty() {
        return Ok(());
    }
    let exprs: Vec<String> = armed.iter().map(|a| a.expression.clone()).collect();
    let entries = bp::list(chan).await?;
    for index in indices_to_remove(&entries, &exprs) {
        bp::remove(chan, index).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

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

    fn echo(index: u32, expr: &str) -> String {
        format!(
            "CPU condition breakpoint {index} with 1 condition(s) added:\n\t{expr}\n\
-> Trace (just show breakpoint info, instead of dropping to debugger)."
        )
    }

    #[tokio::test]
    async fn arm_all_then_remove_leaves_user_breakpoint() {
        let listing = "3 conditional CPU breakpoints:\n   1:\tpc = $1000 :once\n\
   2:\tpc = $1000 :trace\n   3:\tpc = $2000 :trace";
        let mut fake = Fake {
            responses: vec![
                echo(2, "pc = $1000"),
                echo(3, "pc = $2000"),
                listing.to_string(),
                "Removed CPU breakpoint 3:\n\tpc = $2000 :trace".to_string(),
                "Removed CPU breakpoint 2:\n\tpc = $1000 :trace".to_string(),
            ],
            sent: vec![],
        };
        let pcs = [Addr::new(0x1000), Addr::new(0x2000)];
        let armed = arm_all(&mut fake, &pcs).await.unwrap();
        let got: Vec<(u32, &str)> = armed
            .iter()
            .map(|a| (a.index, a.expression.as_str()))
            .collect();
        assert_eq!(got, vec![(2, "pc = $1000"), (3, "pc = $2000")]);
        remove_ours(&mut fake, &armed).await.unwrap();
        assert_eq!(
            fake.sent,
            vec![
                "b pc = $1000 :trace",
                "b pc = $2000 :trace",
                "b",
                "b 3",
                "b 2"
            ]
        );
    }

    #[tokio::test]
    async fn arm_failure_removes_already_armed() {
        let mut fake = Fake {
            responses: vec![
                echo(1, "pc = $1000"),
                "ERROR: bad condition".to_string(),
                "1 conditional CPU breakpoints:\n   1:\tpc = $1000 :trace".to_string(),
                "Removed CPU breakpoint 1:\n\tpc = $1000 :trace".to_string(),
            ],
            sent: vec![],
        };
        let pcs = [Addr::new(0x1000), Addr::new(0x2000)];
        assert!(arm_all(&mut fake, &pcs).await.is_err());
        assert_eq!(fake.sent.last().unwrap(), "b 1");
    }
}
