//! GEMDOS basepage discovery over the sentinel-synced debugger channel
//! (P16): `$4F2` → OSHEADER → (`os_beg+2` version gate) → `p_run` →
//! current basepage → TEXT/DATA/BSS geometry. The running program's load
//! address varies per run (ARCHITECTURE.md `debug/` contract) — this is the
//! only sanctioned way to obtain it. Layout authority:
//! refs/orionfuzion/r1pp3r/src/R1PP3R.S `FIX_CURR_BPAGE` (cited, not ported).
//! `ram_len` bounds the RAM-resident hops (sysbase, p_run, the basepage
//! itself); `os_beg` is bounded by the full 24-bit bus instead — on a
//! ROM-resident TOS the real OSHEADER lives in ROM ($E00000+/$FC0000+),
//! confirmed live against TOS 1.62 + the MJJ demo (ADR-31). p_run semantics
//! under Hatari's bundled TOS are likewise verified live (ADR-31).

use crate::error::{Error, Result};
use crate::types::Addr;

use super::{read_bytes, DebugChannel};

const SYSBASE_PTR: u32 = 0x4F2;
const OS_BEG_OFFSET: u32 = 8;
const OS_VERSION_OFFSET: u32 = 2;
const P_RUN_OFFSET: u32 = 0x28;
const MIN_TOS_VERSION: u16 = 0x0102;
/// 8 basepage fields (`p_lowtpa`..`p_blen`), 4 bytes each (§4.3).
const BASEPAGE_LEN: usize = 0x20;
const BASEPAGE_TPA_HEADER: usize = 0x100;

/// The current GEMDOS process's basepage, as read live (§4.3 fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Basepage {
    /// Address of the basepage itself (`p_run` dereferenced).
    pub addr: Addr,
    pub text_base: Addr,
    pub text_len: u32,
    pub data_base: Addr,
    pub data_len: u32,
    pub bss_base: Addr,
    pub bss_len: u32,
}

/// Read the current process's basepage (§4.3 wire sequence — six
/// sentinel-synced `m` reads). `ram_len` bounds the RAM-resident hops;
/// `os_beg` is bounded by the 24-bit bus instead (ADR-31 — it legitimately
/// lands in ROM on a ROM-resident TOS).
pub async fn current_basepage(chan: &mut impl DebugChannel, ram_len: usize) -> Result<Basepage> {
    let sysbase = hop_read(chan, SYSBASE_PTR, "sysbase", ram_len, "RAM").await?;
    let os_beg = hop_read(
        chan,
        sysbase + OS_BEG_OFFSET,
        "os_beg",
        Addr::BUS_LEN as usize,
        "the 24-bit bus",
    )
    .await?;
    check_tos_version(chan, os_beg).await?;
    let p_run = hop_read(chan, os_beg + P_RUN_OFFSET, "p_run", ram_len, "RAM").await?;
    let bp_addr = read_basepage_ptr(chan, p_run, ram_len).await?;
    let raw = read_exact(chan, bp_addr, BASEPAGE_LEN).await?;
    decode(bp_addr, &raw, ram_len)
}

/// One `m $addr n` round-trip that errors loud on a short reply — every hop
/// in this module goes through this (`read_bytes` truncates but never pads).
async fn read_exact(chan: &mut impl DebugChannel, addr: u32, n: usize) -> Result<Vec<u8>> {
    let bytes = read_bytes(chan, Addr::new(addr), n).await?;
    if bytes.len() != n {
        return Err(discovery_err(format!(
            "short read at ${addr:x}: got {} of {n} bytes",
            bytes.len()
        )));
    }
    Ok(bytes)
}

fn discovery_err(input: impl Into<String>) -> Error {
    Error::Parse {
        context: "basepage discovery".to_string(),
        input: input.into(),
    }
}

/// Zero or `>= ceiling` is out of reach — every pointer hop rejects it.
/// `ceiling`/`region` let RAM-resident hops (sysbase, p_run) stay
/// `ram_len`-bounded while `os_beg` is bounded by the 24-bit bus instead
/// (ADR-31 — the real OSHEADER legitimately sits in ROM).
fn hop(name: &'static str, v: u32, ceiling: usize, region: &'static str) -> Result<u32> {
    if v == 0 || v as usize >= ceiling {
        return Err(discovery_err(format!("{name} = ${v:x} outside {region}")));
    }
    Ok(v)
}

/// Read a 4-byte pointer at `addr` and validate it with `hop`.
async fn hop_read(
    chan: &mut impl DebugChannel,
    addr: u32,
    name: &'static str,
    ceiling: usize,
    region: &'static str,
) -> Result<u32> {
    hop(
        name,
        be32(&read_exact(chan, addr, 4).await?),
        ceiling,
        region,
    )
}

async fn check_tos_version(chan: &mut impl DebugChannel, os_beg: u32) -> Result<()> {
    let version = be16(&read_exact(chan, os_beg + OS_VERSION_OFFSET, 2).await?);
    if version < MIN_TOS_VERSION {
        return Err(Error::Session(format!(
            "TOS < 1.02: basepage discovery unsupported (found ${version:04x})"
        )));
    }
    Ok(())
}

/// Step 5 (§4.3): zero is a distinct `Error::Session`, not a `hop` failure —
/// it means "no process", not "corrupt pointer".
async fn read_basepage_ptr(
    chan: &mut impl DebugChannel,
    p_run: u32,
    ram_len: usize,
) -> Result<u32> {
    let bp_addr = be32(&read_exact(chan, p_run, 4).await?);
    if bp_addr == 0 {
        return Err(Error::Session(
            "no GEMDOS process is running (p_run holds 0)".to_string(),
        ));
    }
    if !bp_addr.is_multiple_of(2) || bp_addr as usize + BASEPAGE_TPA_HEADER > ram_len {
        return Err(discovery_err(format!(
            "basepage ${bp_addr:x} invalid (odd, or +$100 exceeds RAM)"
        )));
    }
    Ok(bp_addr)
}

/// §5.2 sanity checks: TEXT/DATA must be contiguous (the §6.2 mapping
/// depends on it) and the whole TEXT+DATA image must fit in RAM.
fn decode(bp_addr: u32, raw: &[u8], ram_len: usize) -> Result<Basepage> {
    let text_base = be32(&raw[0x08..0x0C]);
    let text_len = be32(&raw[0x0C..0x10]);
    let data_base = be32(&raw[0x10..0x14]);
    let data_len = be32(&raw[0x14..0x18]);
    let bss_base = be32(&raw[0x18..0x1C]);
    let bss_len = be32(&raw[0x1C..0x20]);
    let text_end = u64::from(text_base) + u64::from(text_len);
    if text_end != u64::from(data_base) {
        return Err(basepage_err(
            "non-contiguous TEXT/DATA — not a standard Pexec load",
        ));
    }
    if text_end + u64::from(data_len) > ram_len as u64 {
        return Err(basepage_err("TEXT+DATA extends past RAM"));
    }
    Ok(Basepage {
        addr: Addr::new(bp_addr),
        text_base: Addr::new(text_base),
        text_len,
        data_base: Addr::new(data_base),
        data_len,
        bss_base: Addr::new(bss_base),
        bss_len,
    })
}

fn basepage_err(input: impl Into<String>) -> Error {
    Error::Parse {
        context: "basepage".to_string(),
        input: input.into(),
    }
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Pads `bytes` to whole 16-byte-pair lines, the shape `parse_memdump`
    /// accepts.
    fn dump(addr: u32, bytes: &[u8]) -> String {
        bytes
            .chunks(16)
            .enumerate()
            .map(|(i, chunk)| {
                let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
                format!(
                    "{:08X}: {}   {}",
                    addr + (i * 16) as u32,
                    hex.join(" "),
                    ".".repeat(chunk.len())
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    const RAM_LEN: usize = 1 << 20;
    const SYSBASE: u32 = 0x0000_5000;
    const P_RUN: u32 = 0x0000_602C;
    const BP_ADDR: u32 = 0x0002_0000;

    fn basepage_block() -> Vec<u8> {
        let mut b = vec![0u8; 0x20];
        b[0x08..0x0C].copy_from_slice(&0x0002_0100u32.to_be_bytes()); // tbase
        b[0x0C..0x10].copy_from_slice(&0x0000_1000u32.to_be_bytes()); // tlen
        b[0x10..0x14].copy_from_slice(&0x0002_1100u32.to_be_bytes()); // dbase
        b[0x14..0x18].copy_from_slice(&0x0000_0200u32.to_be_bytes()); // dlen
        b[0x18..0x1C].copy_from_slice(&0x0002_1300u32.to_be_bytes()); // bbase
        b[0x1C..0x20].copy_from_slice(&0x0000_0080u32.to_be_bytes()); // blen
        b
    }

    /// Six canned responses, in wire order, for the happy-path world (§9.3):
    /// a RAM-loaded TOS where os_beg stays in RAM (self-referencing sysbase).
    fn happy_world(version: u16, bp_ptr: u32) -> Vec<String> {
        vec![
            dump(0, &SYSBASE.to_be_bytes()),
            dump(0, &SYSBASE.to_be_bytes()), // os_beg = sysbase (self)
            dump(0, &version.to_be_bytes()),
            dump(0, &P_RUN.to_be_bytes()),
            dump(0, &bp_ptr.to_be_bytes()),
            dump(BP_ADDR, &basepage_block()),
        ]
    }

    /// ADR-31: mirrors the live ROM-resident-TOS world (verified against
    /// real Hatari + TOS 1.62 + the MJJ demo) — sysbase is a RAM shadow,
    /// but `8(sysbase)` escapes into ROM to the real OSHEADER.
    const OS_BEG_ROM: u32 = 0x00E0_0000;

    fn rom_world(version: u16, bp_ptr: u32) -> Vec<String> {
        vec![
            dump(0, &SYSBASE.to_be_bytes()),
            dump(0, &OS_BEG_ROM.to_be_bytes()),
            dump(0, &version.to_be_bytes()),
            dump(0, &P_RUN.to_be_bytes()),
            dump(0, &bp_ptr.to_be_bytes()),
            dump(BP_ADDR, &basepage_block()),
        ]
    }

    #[tokio::test]
    async fn discovery_command_sequence() {
        let mut fake = Fake {
            responses: happy_world(0x0104, BP_ADDR),
            sent: vec![],
        };
        let bp = current_basepage(&mut fake, RAM_LEN).await.unwrap();
        assert_eq!(
            fake.sent,
            vec![
                "m $4f2 4",
                "m $5008 4",
                "m $5002 2",
                "m $5028 4",
                "m $602c 4",
                "m $20000 32",
            ]
        );
        assert_eq!(bp.addr, Addr::new(BP_ADDR));
        assert_eq!(bp.text_base, Addr::new(0x0002_0100));
        assert_eq!(bp.text_len, 0x1000);
        assert_eq!(bp.data_base, Addr::new(0x0002_1100));
        assert_eq!(bp.data_len, 0x200);
        assert_eq!(bp.bss_base, Addr::new(0x0002_1300));
        assert_eq!(bp.bss_len, 0x80);
    }

    #[tokio::test]
    async fn old_tos_rejected() {
        let mut fake = Fake {
            responses: happy_world(0x0100, BP_ADDR),
            sent: vec![],
        };
        let err = current_basepage(&mut fake, RAM_LEN).await.unwrap_err();
        assert!(matches!(err, Error::Session(m) if m.contains("TOS < 1.02")));
    }

    #[tokio::test]
    async fn p_run_null_is_session_error() {
        let mut fake = Fake {
            responses: happy_world(0x0104, 0),
            sent: vec![],
        };
        let err = current_basepage(&mut fake, RAM_LEN).await.unwrap_err();
        assert!(matches!(err, Error::Session(m) if m.contains("no GEMDOS process")));
    }

    #[tokio::test]
    async fn pointer_out_of_ram_rejected() {
        let mut world = happy_world(0x0104, BP_ADDR);
        world[0] = dump(0, &(RAM_LEN as u32).to_be_bytes()); // sysbase >= ram_len
        let mut fake = Fake {
            responses: world,
            sent: vec![],
        };
        let err = current_basepage(&mut fake, RAM_LEN).await.unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "basepage discovery"));
    }

    #[tokio::test]
    async fn non_contiguous_data_rejected() {
        let mut block = basepage_block();
        block[0x10..0x14].copy_from_slice(&0x0002_9999u32.to_be_bytes()); // dbase mismatch
        let mut world = happy_world(0x0104, BP_ADDR);
        world[5] = dump(BP_ADDR, &block);
        let mut fake = Fake {
            responses: world,
            sent: vec![],
        };
        let err = current_basepage(&mut fake, RAM_LEN).await.unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "basepage"));
    }

    #[tokio::test]
    async fn ranges_exceed_ram_rejected() {
        let mut block = basepage_block();
        block[0x0C..0x10].copy_from_slice(&0xFFFF_0000u32.to_be_bytes()); // huge tlen
        block[0x10..0x14].copy_from_slice(
            &(0x0002_0100u32.wrapping_add(0xFFFF_0000)).to_be_bytes(), // keep contiguity
        );
        let mut world = happy_world(0x0104, BP_ADDR);
        world[5] = dump(BP_ADDR, &block);
        let mut fake = Fake {
            responses: world,
            sent: vec![],
        };
        let err = current_basepage(&mut fake, RAM_LEN).await.unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "basepage"));
    }

    /// ADR-31: os_beg legitimately escapes into ROM on a ROM-resident TOS —
    /// the wire sequence re-anchors to os_beg's own address for the version
    /// and p_run reads, and discovery still succeeds.
    #[tokio::test]
    async fn rom_resident_os_header_world_discovers_basepage() {
        let mut fake = Fake {
            responses: rom_world(0x0162, BP_ADDR),
            sent: vec![],
        };
        let bp = current_basepage(&mut fake, RAM_LEN).await.unwrap();
        assert_eq!(
            fake.sent,
            vec![
                "m $4f2 4",
                "m $5008 4",
                "m $e00002 2",
                "m $e00028 4",
                "m $602c 4",
                "m $20000 32",
            ]
        );
        assert_eq!(bp.addr, Addr::new(BP_ADDR));
    }

    /// ADR-31: os_beg is bounded by the 24-bit bus, not `ram_len` — but a
    /// value beyond even that bus still fails loud.
    #[tokio::test]
    async fn os_beg_beyond_bus_rejected() {
        let mut world = happy_world(0x0104, BP_ADDR);
        world[1] = dump(0, &0xFF00_0000u32.to_be_bytes());
        let mut fake = Fake {
            responses: world,
            sent: vec![],
        };
        let err = current_basepage(&mut fake, RAM_LEN).await.unwrap_err();
        assert!(matches!(err, Error::Parse { context, .. } if context == "basepage discovery"));
    }
}
