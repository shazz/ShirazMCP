use super::map;
use crate::types::Addr;

/// The last value written to the PSG register-select port ($FF8800). P1
/// tools pass `None` (no write-stream tracking yet — the PSG-aware path is
/// exercised by unit tests and by P2's disasm annotations).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PsgSelect(pub u8);

/// ARCHITECTURE.md shape. `register` = component/group ("Shifter", "PSG",
/// or the dataset section name); `detail` = human description. Tool JSON
/// renders it as `label = "{register} — {detail}"` (§8.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    pub addr: Addr,
    pub register: &'static str,
    pub detail: String,
}

/// YM2149 register names, index 0..=15.
pub const PSG_REG_NAMES: [&str; 16] = [
    "channel A period fine",
    "channel A period coarse",
    "channel B period fine",
    "channel B period coarse",
    "channel C period fine",
    "channel C period coarse",
    "noise period",
    "mixer control",
    "channel A level",
    "channel B level",
    "channel C level",
    "envelope period fine",
    "envelope period coarse",
    "envelope shape",
    "I/O port A",
    "I/O port B",
];

/// Annotate a bus address. Lookup order (first hit wins): PSG special-case,
/// the palette-register range, the dataset, the word-register fallback.
pub fn annotate(addr: Addr, psg: Option<PsgSelect>) -> Option<Annotation> {
    // `bus24`, not a blind mask: $01FF8240 (TT-RAM) is not a palette register.
    let a = Addr::bus24(addr.raw())?;
    psg_annotation(a, psg)
        .or_else(|| palette_annotation(a))
        .or_else(|| map::lookup(a).map(|e| from_map(addr, e.group, e.desc.to_string())))
        .or_else(|| {
            map::lookup_word_base(a)
                .map(|e| from_map(addr, e.group, format!("{} (low byte)", e.desc)))
        })
}

fn from_map(addr: Addr, register: &'static str, detail: String) -> Annotation {
    Annotation {
        addr,
        register,
        detail,
    }
}

fn psg_annotation(a: u32, psg: Option<PsgSelect>) -> Option<Annotation> {
    if a == 0xFF8800 {
        return Some(from_map(
            Addr::new(a),
            "PSG",
            "register select / read data".to_string(),
        ));
    }
    if a == 0xFF8802 {
        let detail = match psg {
            Some(PsgSelect(n)) => {
                let name = PSG_REG_NAMES[(n & 0x0F) as usize];
                format!("write data → register {n} ({name})")
            }
            None => "write data".to_string(),
        };
        return Some(from_map(Addr::new(a), "PSG", detail));
    }
    None
}

fn palette_annotation(a: u32) -> Option<Annotation> {
    if (0xFF8240..=0xFF825F).contains(&a) {
        let index = (a - 0xFF8240) / 2;
        return Some(from_map(
            Addr::new(a),
            "Shifter",
            format!("palette register {index}"),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotate_32_bit_io_alias_but_not_tt_ram() {
        let a = annotate(Addr::new(0xFFFF_8240), None).unwrap();
        assert_eq!(a.detail, "palette register 0");
        // TT-RAM look-alike must not be taken for the palette register.
        assert!(annotate(Addr::new(0x01FF_8240), None).is_none());
    }

    #[test]
    fn annotate_palette_zero() {
        let a = annotate(Addr::new(0xFF8240), None).unwrap();
        assert_eq!(a.register, "Shifter");
        assert_eq!(a.detail, "palette register 0");
    }

    #[test]
    fn annotate_palette_index() {
        let a = annotate(Addr::new(0xFF825E), None).unwrap();
        assert_eq!(a.detail, "palette register 15");
    }

    #[test]
    fn annotate_masks_high_bits() {
        assert_eq!(
            annotate(Addr::new(0xFFFF8240), None),
            annotate(Addr::new(0xFF8240), None)
        );
    }

    #[test]
    fn annotate_psg_select_aware() {
        let a = annotate(Addr::new(0xFF8802), Some(PsgSelect(7))).unwrap();
        assert_eq!(a.detail, "write data → register 7 (mixer control)");
    }

    #[test]
    fn annotate_psg_unselected() {
        let a = annotate(Addr::new(0xFF8802), None).unwrap();
        assert_eq!(a.detail, "write data");
    }

    #[test]
    fn annotate_plain_ram_is_none() {
        assert_eq!(annotate(Addr::new(0x050000), None), None);
    }
}
