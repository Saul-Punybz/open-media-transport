//! MSB-first bit writer/reader and the VMX Exp-Golomb style codes.
//!
//! VMX uses two code families, both bit-identical to libvmx:
//!
//! * **value code** (`0` prefix): an unsigned `v >= 2` is written as
//!   `bitlen(v) - 1` zero bits followed by the `bitlen(v)` bits of `v`.
//! * **run code** (`1` prefix): a count `n >= 1` is written as a `1`, then
//!   `bitlen(n) - 1` zero bits, then the `bitlen(n)` bits of `n`.
//!
//! Signed values are mapped with [`to_code`] / [`from_code`].

/// Maps a signed coefficient to its value-code integer (`2|v|+1` for
/// positive, `2|v|` for negative; never 0 or 1 for non-zero input).
#[inline(always)]
pub(crate) fn to_code(v: i32) -> u32 {
    if v > 0 {
        (2 * v + 1) as u32
    } else {
        (-2 * v) as u32
    }
}

/// Inverse of [`to_code`], with the 16-bit truncation libvmx applies.
#[inline(always)]
pub(crate) fn from_code(code: u64) -> i16 {
    let i = code.wrapping_sub(1);
    let odd = i & 1;
    let x = i.wrapping_add(odd);
    ((x >> 1).wrapping_sub(x.wrapping_mul(odd))) as i16
}

/// Growable MSB-first bit writer.
#[derive(Default)]
pub(crate) struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    pub(crate) fn with_capacity(n: usize) -> Self {
        Self { buf: Vec::with_capacity(n), acc: 0, nbits: 0 }
    }

    /// Appends the low `n` bits of `v` (`n <= 64`).
    #[inline(always)]
    pub(crate) fn put(&mut self, v: u64, n: u32) {
        if n > 32 {
            self.put(v >> 32, n - 32);
            self.put(v & 0xFFFF_FFFF, 32);
            return;
        }
        if n == 0 {
            return;
        }
        self.acc = (self.acc << n) | (v & ((1u64 << n) - 1));
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.buf.push((self.acc >> self.nbits) as u8);
        }
    }

    /// Value code for `v >= 1`.
    #[inline(always)]
    pub(crate) fn put_value(&mut self, v: u32) {
        let bl = 32 - v.leading_zeros();
        self.put(v as u64, 2 * bl - 1);
    }

    /// Run code for `n >= 1`; nothing for `n == 0`.
    #[inline(always)]
    pub(crate) fn put_run(&mut self, n: u32) {
        if n == 0 {
            return;
        }
        let bl = 32 - n.leading_zeros();
        self.put((1u64 << (2 * bl - 1)) | n as u64, 2 * bl);
    }

    /// Pads with zero bits to the next byte boundary.
    pub(crate) fn align(&mut self) {
        if self.nbits > 0 {
            let pad = 8 - self.nbits;
            self.put(0, pad);
        }
    }

    pub(crate) fn into_bytes(mut self) -> Vec<u8> {
        self.align();
        self.buf
    }
}

/// Error raised when a bitstream is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Corrupt;

/// MSB-first bit reader over one slice stream. Reads past the end return
/// `1` bits, like the `0xFF` guard bytes libvmx places after each stream, so
/// truncated input terminates quickly instead of looping.
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// Absolute bit position.
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Peeks up to 57 bits (MSB-first) without consuming them.
    #[inline(always)]
    fn peek64(&self) -> u64 {
        let byte = self.pos >> 3;
        let mut w = [0xFFu8; 8];
        if byte + 8 <= self.data.len() {
            w.copy_from_slice(&self.data[byte..byte + 8]);
        } else if byte < self.data.len() {
            let n = self.data.len() - byte;
            w[..n].copy_from_slice(&self.data[byte..]);
        }
        u64::from_be_bytes(w) << (self.pos & 7)
    }

    #[inline(always)]
    pub(crate) fn bit(&mut self) -> u32 {
        let b = (self.peek64() >> 63) as u32;
        self.pos += 1;
        b
    }

    /// Counts and consumes leading zero bits (at most 56).
    #[inline(always)]
    pub(crate) fn zeros(&mut self) -> Result<u32, Corrupt> {
        let z = self.peek64().leading_zeros();
        if z > 56 {
            return Err(Corrupt);
        }
        self.pos += z as usize;
        Ok(z)
    }

    /// Reads `n <= 56` bits.
    #[inline(always)]
    pub(crate) fn bits(&mut self, n: u32) -> u64 {
        if n == 0 {
            return 0;
        }
        let v = self.peek64() >> (64 - n);
        self.pos += n as usize;
        v
    }

    /// Reads the tail of a value or run code whose leading `0` (value) or
    /// `1` + `0` (run) has already been consumed: remaining zeros, then
    /// `zeros + 2` bits.
    #[inline(always)]
    pub(crate) fn code_tail(&mut self) -> Result<u64, Corrupt> {
        let z = self.zeros()?;
        let n = z + 2;
        if n > 56 {
            return Err(Corrupt);
        }
        Ok(self.bits(n))
    }

    /// Skips to the next byte boundary.
    pub(crate) fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_mapping_roundtrip() {
        for v in -3000i32..3000 {
            if v == 0 {
                continue;
            }
            assert_eq!(from_code(to_code(v) as u64) as i32, v);
        }
    }

    #[test]
    fn run_and_value_codes() {
        let mut w = BitWriter::default();
        w.put_run(1); // 11
        w.put_run(2); // 1010
        w.put_value(2); // 010
        w.put_value(2049); // 11 zeros + 12 bits
        let b = w.into_bytes();
        let mut r = BitReader::new(&b);
        assert_eq!(r.bit(), 1);
        assert_eq!(r.bit(), 1);
        assert_eq!(r.bit(), 1);
        assert_eq!(r.bit(), 0);
        assert_eq!(r.code_tail().unwrap(), 2);
        assert_eq!(r.bit(), 0);
        assert_eq!(r.code_tail().unwrap(), 2);
        assert_eq!(r.bit(), 0);
        assert_eq!(r.code_tail().unwrap(), 2049);
    }
}
