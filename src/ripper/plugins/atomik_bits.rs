//! Atomik bit-stream primitives: the backward recycled-sentinel reader
//! (§4.A.2) and the static match-offset tables (§4.A.5/§4.A.3 step 1).
//! Split out of `atomik_decode` for the 200-line budget (P9 §5, the same
//! "helper modules are `pub(crate)`-consumed and don't add registration
//! points" rule as `ice_decode`/`rnc_m1`/`rnc_m2`).

/// Backward reader; recycled-sentinel reload (§4.A.2 — NOT ICE's forced-1).
pub(crate) struct Stream<'a> {
    data: &'a [u8],
    pub(crate) pos: usize,
    pub(crate) reg: u8,
}

impl<'a> Stream<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: data.len(),
            reg: 0,
        }
    }

    pub(crate) fn take_byte(&mut self) -> Option<u8> {
        self.pos = self.pos.checked_sub(1)?;
        Some(self.data[self.pos])
    }

    /// §4.A.2: the shifted-out bit is recycled into bit 0 of the reloaded
    /// register instead of ICE's forced `1`.
    pub(crate) fn bit(&mut self) -> Option<bool> {
        let carry = self.reg & 0x80 != 0;
        self.reg <<= 1;
        if self.reg == 0 {
            let next = self.take_byte()?;
            let bit = next & 0x80 != 0;
            self.reg = (next << 1) | u8::from(carry);
            return Some(bit);
        }
        Some(carry)
    }

    /// MSB-first fold; `bits(0) == Some(0)` reading nothing.
    pub(crate) fn bits(&mut self, n: u32) -> Option<u32> {
        (0..n).try_fold(0u32, |acc, _| Some((acc << 1) | u32::from(self.bit()?)))
    }
}

pub(crate) const BASES: [u32; 7] = [32, 96, 352, 864, 1888, 3936, 8032];
pub(crate) const XBITS: [u32; 8] = [0, 1, 3, 4, 5, 6, 7, 8];

/// The 7-entry odd table (§4.A.3 step 1): the first 7 odd numbers, skipping
/// the one equal to `spec_byte`.
pub(crate) fn build_odd_table(spec_byte: u8) -> [u8; 7] {
    let mut odd = [0u8; 7];
    let mut d1: u8 = 1;
    for slot in odd.iter_mut() {
        if d1 == spec_byte {
            d1 = d1.wrapping_add(2);
        }
        *slot = d1;
        d1 = d1.wrapping_add(2);
    }
    odd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_table_skips_spec_byte() {
        assert_eq!(build_odd_table(5), [1, 3, 7, 9, 11, 13, 15]);
        assert_eq!(build_odd_table(0), [1, 3, 5, 7, 9, 11, 13]);
    }

    #[test]
    fn bits_zero_reads_nothing() {
        let data = [0xFFu8];
        let mut s = Stream::new(&data);
        s.reg = s.take_byte().unwrap();
        assert_eq!(s.bits(0), Some(0));
    }
}
