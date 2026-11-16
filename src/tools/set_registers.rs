//! `set_registers`: write any of D0-D7/A0-A6/USP/ISP/SR/PC in one call,
//! ordered so both stack pointers land regardless of the starting or final
//! CPU mode, then read everything back (ADR-47). Plan + diff are pure in
//! `set_registers_wire.rs`; this file is choreography only.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::debugger::regs_to_output;
use super::set_registers_sr::{sr_mask_for, sr_notes};
use super::set_registers_wire::{mismatches, plan_writes, validate, RegWrite};
use super::{to_mcp, wrong_state, AnySession, HatariServer};
use crate::debug::{self, setreg, DebugChannel};
use crate::session::HatariProcess;

pub use super::set_registers_wire::{SetRegistersArgs, SetRegistersOutput};

// `vis = "pub(crate)"`: see lifecycle.rs for the rationale (rmcp-2.2 gotcha).
#[tool_router(router = set_registers_router, vis = "pub(crate)")]
impl HatariServer {
    #[tool(
        name = "set_registers",
        description = "Set CPU registers in one call: any of d0-d7, a0-a6, \
                       usp, isp, sr, pc (numbers; omitted = unchanged). \
                       Session must be PAUSED. A7 is not a field — it \
                       aliases USP or ISP by SR's S bit; give usp and/or \
                       isp and both end up as requested whatever the \
                       starting or final mode (writes are ordered: SR \
                       mode first, each stack pointer through A7 while it \
                       is the active one, then D/A, PC last). Reads all \
                       registers back: ok=false + not_applied lists any \
                       field that did not take; notes lists caveats (SR \
                       bits the CPU lacks are dropped — 68000 has no bits \
                       14/12, so $B300 reads $A300 and still counts as \
                       applied; a set trace bit 15 sends execution to the \
                       Trace vector after one instruction). A failed write \
                       rolls the registers back to their original values."
    )]
    pub async fn set_registers(
        &self,
        Parameters(a): Parameters<SetRegistersArgs>,
    ) -> Result<Json<SetRegistersOutput>, McpError> {
        validate(&a)?;
        let mut guard = self.session.lock().await;
        let mask = sr_mask_for(guard.config());
        let proc = paused_proc(&mut guard)?;
        apply_and_verify(proc, &a, mask).await.map(Json)
    }
}

fn paused_proc(guard: &mut AnySession) -> Result<&mut HatariProcess, McpError> {
    match guard {
        AnySession::Paused(s) => Ok(s.process_mut()),
        AnySession::Running(_) => Err(wrong_state(
            "running",
            "paused — pause first so the writes are coherent",
        )),
        AnySession::Stopped(_) => Err(wrong_state("stopped", "paused")),
    }
}

/// Generic over the channel so the whole read → write → rollback/readback
/// sequence is unit-testable with a canned-response fake.
async fn apply_and_verify(
    proc: &mut impl DebugChannel,
    a: &SetRegistersArgs,
    sr_mask: u16,
) -> Result<SetRegistersOutput, McpError> {
    let orig = debug::registers(proc).await.map_err(to_mcp)?;
    let orig_sr = setreg::read_sr(proc).await.map_err(to_mcp)?;
    let plan = plan_writes(a, orig_sr);
    if let Err(e) = apply(proc, &plan).await {
        let back = plan_writes(&SetRegistersArgs::from_snapshot(&orig, orig_sr), orig_sr);
        let rollback = match apply(proc, &back).await {
            Ok(()) => "registers rolled back",
            Err(_) => "ROLLBACK ALSO FAILED — registers may be partially written",
        };
        return Err(McpError::internal_error(
            format!("{}; {rollback}", to_mcp(e).message),
            None,
        ));
    }
    let got = debug::registers(proc).await.map_err(to_mcp)?;
    let got_sr = setreg::read_sr(proc).await.map_err(to_mcp)?;
    let not_applied = mismatches(a, &got, got_sr, sr_mask);
    Ok(SetRegistersOutput {
        ok: not_applied.is_empty(),
        not_applied,
        notes: sr_notes(a.sr, sr_mask),
        writes: plan.iter().map(RegWrite::command).collect(),
        registers: regs_to_output(got),
        sr_raw: format!("${got_sr:04X}"),
        state: "paused".to_string(),
    })
}

async fn apply(proc: &mut impl DebugChannel, plan: &[RegWrite]) -> crate::error::Result<()> {
    for w in plan {
        setreg::set_register(proc, w.name, w.value).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

    const M: u16 = crate::tools::set_registers_sr::SR_MASK_68000;

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

    /// `cpureg` in the fixture shape (tests/fixtures/hatari_session.log).
    fn cpureg(a7: u32, usp: u32, isp: u32, s: u8, pc: u32) -> String {
        format!(
            "  D0 00000000   D1 00000000   D2 00000000   D3 00000000 \n\
  D4 00000000   D5 00000000   D6 00000000   D7 00000000 \n\
  A0 00000000   A1 00000000   A2 00000000   A3 00000000 \n\
  A4 00000000   A5 00000000   A6 00000000   A7 {a7:08X} \n\
USP  {usp:08X} ISP  {isp:08X} \n\
T=00 S={s} M=0 X=0 N=0 Z=0 V=0 C=0 IMASK=7 STP=0\n\
{pc:08x} 4e71                     nop\n\
Next PC: {:08x}",
            pc + 2
        )
    }

    fn sr_eval(v: u16) -> String {
        format!("= %0 (bin), #{v} (dec), ${v:x} (hex)")
    }

    /// user mode, both stacks + SR to supervisor: the exact command
    /// stream, then a clean readback.
    #[tokio::test]
    async fn applies_plan_and_reports_ok() {
        let a = SetRegistersArgs {
            sr: Some(0x2700),
            usp: Some(0x1000),
            isp: Some(0x2000),
            ..Default::default()
        };
        let mut responses = vec![cpureg(0x500, 0x500, 0x600, 0, 0x4000), sr_eval(0x0300)];
        responses.extend(std::iter::repeat_n(String::new(), 4));
        responses.push(cpureg(0x2000, 0x1000, 0x2000, 1, 0x4000));
        responses.push(sr_eval(0x2700));
        let mut fake = Fake {
            responses,
            sent: vec![],
        };
        let out = apply_and_verify(&mut fake, &a, M).await.unwrap();
        assert!(out.ok, "{:?}", out.not_applied);
        assert_eq!(
            &fake.sent[2..6],
            ["r sr=$700", "r a7=$1000", "r sr=$2700", "r a7=$2000"]
        );
        assert_eq!(out.sr_raw, "$2700");
    }

    /// A field that reads back different is reported, not hidden.
    #[tokio::test]
    async fn readback_mismatch_sets_ok_false() {
        let a = SetRegistersArgs {
            isp: Some(0x2000),
            ..Default::default()
        };
        let mut fake = Fake {
            responses: vec![
                cpureg(0x500, 0x500, 0x600, 1, 0x4000),
                sr_eval(0x2700),
                String::new(),
                cpureg(0x600, 0x500, 0x600, 1, 0x4000),
                sr_eval(0x2700),
            ],
            sent: vec![],
        };
        let out = apply_and_verify(&mut fake, &a, M).await.unwrap();
        assert!(!out.ok);
        assert_eq!(out.not_applied, vec!["isp: wanted $2000, got $600"]);
    }

    /// 68000: $B300 reads back $A300 (bit 12 unimplemented) — ok, with a
    /// dropped-bit note and a trace-bit warning.
    #[tokio::test]
    async fn sr_bit12_dropped_is_ok_with_notes() {
        let a = SetRegistersArgs {
            sr: Some(0xB300),
            ..Default::default()
        };
        let mut fake = Fake {
            responses: vec![
                cpureg(0x500, 0x500, 0x600, 1, 0x4000),
                sr_eval(0x2700),
                String::new(),
                cpureg(0x500, 0x500, 0x600, 1, 0x4000),
                sr_eval(0xA300),
            ],
            sent: vec![],
        };
        let out = apply_and_verify(&mut fake, &a, M).await.unwrap();
        assert!(out.ok, "{:?}", out.not_applied);
        assert_eq!(out.sr_raw, "$A300");
        assert_eq!(out.notes.len(), 2, "{:?}", out.notes);
    }

    /// A rejected write triggers a rollback to the original snapshot.
    #[tokio::test]
    async fn failed_write_rolls_back() {
        let a = SetRegistersArgs {
            d0: Some(1),
            d1: Some(2),
            ..Default::default()
        };
        let usage = "\tError, usage: r or r xx=yyyy".to_string();
        let mut responses = vec![
            cpureg(0x500, 0x500, 0x600, 1, 0x4000),
            sr_eval(0x2700),
            String::new(),
            usage,
        ];
        responses.extend(std::iter::repeat_n(String::new(), 20));
        let mut fake = Fake {
            responses,
            sent: vec![],
        };
        let err = apply_and_verify(&mut fake, &a, M).await.unwrap_err();
        assert!(err.message.contains("rolled back"), "{}", err.message);
        assert_eq!(fake.sent[4], "r sr=$700"); // rollback starts (USP detour)
        assert_eq!(fake.sent.last().unwrap(), "r pc=$4000");
    }
}
