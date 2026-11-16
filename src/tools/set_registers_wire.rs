//! Wire schema + pure write planning / readback diff for `set_registers`
//! (ADR-47). Split from `set_registers.rs` (choreography) — the
//! `profile_frame`/`profile_frame_wire.rs` precedent.
//!
//! Stack-pointer model (Hatari's UAE core, `newcpu.c` `MakeFromSR_x` +
//! `m68k_dumpstate`): A7 is the LIVE stack pointer; `regs.usp`/`regs.isp`
//! are banked copies swapped with A7 on every S-bit change, and `cpureg`
//! re-syncs the ACTIVE mode's copy from A7. So `r usp=`/`r isp=` on the
//! active mode's pointer is silently overwritten. The plan therefore never
//! writes `usp`/`isp` directly: it reaches each pointer through A7 while
//! that pointer is the active one, letting SR banking file it away.

use rmcp::ErrorData as McpError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::debugger::RegistersOutput;
use crate::debug::Registers;

/// SR supervisor bit.
pub(crate) const SR_S: u16 = 0x2000;

#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
pub struct SetRegistersArgs {
    pub d0: Option<u32>,
    pub d1: Option<u32>,
    pub d2: Option<u32>,
    pub d3: Option<u32>,
    pub d4: Option<u32>,
    pub d5: Option<u32>,
    pub d6: Option<u32>,
    pub d7: Option<u32>,
    pub a0: Option<u32>,
    pub a1: Option<u32>,
    pub a2: Option<u32>,
    pub a3: Option<u32>,
    pub a4: Option<u32>,
    pub a5: Option<u32>,
    pub a6: Option<u32>,
    /// User stack pointer (A7 is not settable directly — it aliases
    /// USP or ISP depending on SR's S bit; give the one you mean).
    pub usp: Option<u32>,
    /// Interrupt (supervisor) stack pointer.
    pub isp: Option<u32>,
    /// Full 16-bit status register (decimal JSON number, e.g. 9984 =
    /// $2700). Its S bit decides which stack pointer A7 ends up aliasing.
    pub sr: Option<u16>,
    /// Program counter (must be even). Written last.
    pub pc: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SetRegistersOutput {
    /// true iff every requested field read back with the requested value.
    pub ok: bool,
    /// Requested fields that did not take, "name: wanted $X, got $Y".
    pub not_applied: Vec<String>,
    /// The exact `r reg=value` debugger commands sent, in order.
    pub writes: Vec<String>,
    /// Registers read back after the writes.
    pub registers: RegistersOutput,
    /// Raw SR read back (`evaluate sr`), "$2700".
    pub sr_raw: String,
    /// Always "paused" — the tool requires a paused session.
    pub state: String,
}

/// One planned `r {name}=${value:x}` write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegWrite {
    pub name: &'static str,
    pub value: u32,
}

impl RegWrite {
    fn new(name: &'static str, value: u32) -> Self {
        Self { name, value }
    }

    pub(crate) fn command(&self) -> String {
        format!("r {}=${:x}", self.name, self.value)
    }
}

const D_NAMES: [&str; 8] = ["d0", "d1", "d2", "d3", "d4", "d5", "d6", "d7"];
const A_NAMES: [&str; 7] = ["a0", "a1", "a2", "a3", "a4", "a5", "a6"];

impl SetRegistersArgs {
    fn d(&self) -> [Option<u32>; 8] {
        [
            self.d0, self.d1, self.d2, self.d3, self.d4, self.d5, self.d6, self.d7,
        ]
    }

    fn a(&self) -> [Option<u32>; 7] {
        [
            self.a0, self.a1, self.a2, self.a3, self.a4, self.a5, self.a6,
        ]
    }

    fn is_empty(&self) -> bool {
        self.d().iter().chain(self.a().iter()).all(Option::is_none)
            && self.usp.is_none()
            && self.isp.is_none()
            && self.sr.is_none()
            && self.pc.is_none()
    }

    /// Every register of a snapshot as a request — the rollback plan.
    pub(crate) fn from_snapshot(r: &Registers, sr: u16) -> Self {
        let [d0, d1, d2, d3, d4, d5, d6, d7] = r.d.map(Some);
        let [a0, a1, a2, a3, a4, a5, a6, _] = r.a.map(Some);
        Self {
            d0,
            d1,
            d2,
            d3,
            d4,
            d5,
            d6,
            d7,
            a0,
            a1,
            a2,
            a3,
            a4,
            a5,
            a6,
            usp: Some(r.usp),
            isp: Some(r.isp),
            sr: Some(sr),
            pc: Some(r.pc.raw()),
        }
    }
}

pub(crate) fn validate(a: &SetRegistersArgs) -> Result<(), McpError> {
    if a.is_empty() {
        return Err(McpError::invalid_params(
            "give at least one register to set",
            None,
        ));
    }
    if a.pc.is_some_and(|pc| pc % 2 != 0) {
        return Err(McpError::invalid_params(
            "pc must be even (an odd PC raises an address error on resume)",
            None,
        ));
    }
    Ok(())
}

/// Order the writes (pure; ADR-47). `cur_sr` = the SR before any write.
/// 1. Inactive-mode stack pointer requested: switch to the OPPOSITE of the
///    final mode (final SR with S flipped) and write it through A7.
/// 2. Final SR (requested, or the original — restores the mode after 1).
/// 3. Active-mode stack pointer through A7.
/// 4. D0-D7, A0-A6. 5. PC last.
///
/// Correct from either starting mode, and under either reading of which
/// banked copy Hatari ignores — only A7 and SR banking are relied on.
pub(crate) fn plan_writes(a: &SetRegistersArgs, cur_sr: u16) -> Vec<RegWrite> {
    let final_sr = a.sr.unwrap_or(cur_sr);
    let final_super = final_sr & SR_S != 0;
    let (active, inactive) = if final_super {
        (a.isp, a.usp)
    } else {
        (a.usp, a.isp)
    };
    let mut w = Vec::new();
    if let Some(v) = inactive {
        w.push(RegWrite::new("sr", u32::from(final_sr ^ SR_S)));
        w.push(RegWrite::new("a7", v));
    }
    if a.sr.is_some() || inactive.is_some() {
        w.push(RegWrite::new("sr", u32::from(final_sr)));
    }
    if let Some(v) = active {
        w.push(RegWrite::new("a7", v));
    }
    push_some(&mut w, &D_NAMES, &a.d());
    push_some(&mut w, &A_NAMES, &a.a());
    if let Some(pc) = a.pc {
        w.push(RegWrite::new("pc", pc));
    }
    w
}

fn push_some(w: &mut Vec<RegWrite>, names: &[&'static str], values: &[Option<u32>]) {
    for (name, v) in names.iter().zip(values) {
        if let Some(v) = v {
            w.push(RegWrite::new(name, *v));
        }
    }
}

/// Requested fields whose read-back value differs (pure).
pub(crate) fn mismatches(a: &SetRegistersArgs, got: &Registers, got_sr: u16) -> Vec<String> {
    let mut out = Vec::new();
    let mut check = |name: &str, want: Option<u32>, have: u32| {
        if let Some(w) = want.filter(|w| *w != have) {
            out.push(format!("{name}: wanted ${w:x}, got ${have:x}"));
        }
    };
    for (i, name) in D_NAMES.iter().enumerate() {
        check(name, a.d()[i], got.d[i]);
    }
    for (i, name) in A_NAMES.iter().enumerate() {
        check(name, a.a()[i], got.a[i]);
    }
    check("usp", a.usp, got.usp);
    check("isp", a.isp, got.isp);
    check("sr", a.sr.map(u32::from), u32::from(got_sr));
    check("pc", a.pc, got.pc.raw());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::SrFlags;
    use crate::types::Addr;

    fn cmds(w: &[RegWrite]) -> Vec<String> {
        w.iter().map(RegWrite::command).collect()
    }

    /// user -> supervisor with both stacks: USP (inactive in the final
    /// supervisor mode) goes in while S=0, ISP after the final SR.
    #[test]
    fn plan_user_to_super_sets_both_stacks_through_a7() {
        let a = SetRegistersArgs {
            sr: Some(0x2700),
            usp: Some(0x1000),
            isp: Some(0x2000),
            ..Default::default()
        };
        assert_eq!(
            cmds(&plan_writes(&a, 0x0300)),
            vec!["r sr=$700", "r a7=$1000", "r sr=$2700", "r a7=$2000"]
        );
    }

    /// supervisor -> user with both stacks: ISP (inactive in the final
    /// user mode) goes in while S=1, USP after the final SR.
    #[test]
    fn plan_super_to_user_sets_both_stacks_through_a7() {
        let a = SetRegistersArgs {
            sr: Some(0x0000),
            usp: Some(0x1000),
            isp: Some(0x2000),
            ..Default::default()
        };
        assert_eq!(
            cmds(&plan_writes(&a, 0x2700)),
            vec!["r sr=$2000", "r a7=$2000", "r sr=$0", "r a7=$1000"]
        );
    }

    /// No SR given: the current mode is the final mode, and the original
    /// SR is restored after the inactive-pointer detour.
    #[test]
    fn plan_without_sr_keeps_current_mode() {
        let a = SetRegistersArgs {
            isp: Some(0x8000),
            ..Default::default()
        };
        // In user mode ISP is inactive: detour through S=1, then restore.
        assert_eq!(
            cmds(&plan_writes(&a, 0x0010)),
            vec!["r sr=$2010", "r a7=$8000", "r sr=$10"]
        );
        // In supervisor mode ISP is active: one A7 write, SR untouched.
        assert_eq!(cmds(&plan_writes(&a, 0x2700)), vec!["r a7=$8000"]);
    }

    #[test]
    fn plan_orders_data_address_then_pc_last() {
        let a = SetRegistersArgs {
            pc: Some(0x4000),
            d7: Some(7),
            a0: Some(0xa0),
            d0: Some(1),
            ..Default::default()
        };
        assert_eq!(
            cmds(&plan_writes(&a, 0x2700)),
            vec!["r d0=$1", "r d7=$7", "r a0=$a0", "r pc=$4000"]
        );
    }

    #[test]
    fn validate_rejects_empty_and_odd_pc() {
        assert!(validate(&SetRegistersArgs::default()).is_err());
        let odd = SetRegistersArgs {
            pc: Some(0x1001),
            ..Default::default()
        };
        assert!(validate(&odd).unwrap_err().message.contains("even"));
        let ok = SetRegistersArgs {
            d0: Some(0),
            ..Default::default()
        };
        assert!(validate(&ok).is_ok());
    }

    fn regs() -> Registers {
        Registers {
            d: [0, 1, 2, 3, 4, 5, 6, 7],
            a: [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0x2000],
            usp: 0x1000,
            isp: 0x2000,
            sr: SrFlags {
                trace: 0,
                supervisor: true,
                x: false,
                n: false,
                z: false,
                v: false,
                c: false,
                imask: 7,
            },
            pc: Addr::new(0x4000),
        }
    }

    #[test]
    fn mismatches_names_fields_that_did_not_take() {
        let a = SetRegistersArgs {
            d1: Some(1),
            isp: Some(0x3000),
            sr: Some(0x2700),
            ..Default::default()
        };
        assert!(mismatches(&a, &regs(), 0x2700).len() == 1);
        assert_eq!(
            mismatches(&a, &regs(), 0x2300),
            vec![
                "isp: wanted $3000, got $2000",
                "sr: wanted $2700, got $2300"
            ]
        );
    }

    #[test]
    fn snapshot_round_trips_through_plan() {
        let r = regs();
        let back = SetRegistersArgs::from_snapshot(&r, 0x2700);
        assert!(mismatches(&back, &r, 0x2700).is_empty());
        let plan = cmds(&plan_writes(&back, 0x2700));
        // Supervisor snapshot: USP via the S=0 detour, ISP after, PC last.
        assert_eq!(
            &plan[..4],
            ["r sr=$700", "r a7=$1000", "r sr=$2700", "r a7=$2000"]
        );
        assert_eq!(plan.last().unwrap(), "r pc=$4000");
        assert_eq!(plan.len(), 4 + 8 + 7 + 1);
    }
}
