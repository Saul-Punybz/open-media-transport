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
///
/// Bits collect in a 64-bit accumulator and leave it 32 at a time; only the
/// low `nbits` (< 32 between calls) bits of `acc` are pending, anything
/// above them has already been written.
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
            self.put_long(v, n);
        } else {
            self.put32(v, n);
        }
    }

    #[cold]
    #[inline(never)]
    fn put_long(&mut self, v: u64, n: u32) {
        self.put32(v >> 32, n - 32);
        self.put32(v & 0xFFFF_FFFF, 32);
    }

    /// Appends the low `n <= 32` bits of `v`.
    #[inline(always)]
    fn put32(&mut self, v: u64, n: u32) {
        self.acc = (self.acc << n) | (v & ((1u64 << n) - 1));
        self.nbits += n;
        if self.nbits >= 32 {
            self.nbits -= 32;
            self.buf.extend_from_slice(&((self.acc >> self.nbits) as u32).to_be_bytes());
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

    /// Run code for `run` (nothing if 0) followed by the value code for
    /// `v >= 1`, written with a single `put` when they fit in 32 bits.
    #[inline(always)]
    pub(crate) fn put_run_value(&mut self, run: u32, v: u32) {
        let vl = 2 * (32 - v.leading_zeros()) - 1;
        let rbl = 32 - run.leading_zeros();
        let rl = 2 * rbl;
        if rl + vl <= 32 {
            // run == 0 gives rl == 0 and an empty run code.
            let rc = ((1u64 << rl) >> 1) | run as u64;
            self.put32((rc << vl) | v as u64, rl + vl);
        } else {
            self.put_run(run);
            self.put_value(v);
        }
    }

    /// Pads with zero bits to the next byte boundary.
    pub(crate) fn align(&mut self) {
        self.put(0, (8 - self.nbits % 8) % 8);
    }

    pub(crate) fn into_bytes(mut self) -> Vec<u8> {
        self.align();
        while self.nbits > 0 {
            self.nbits -= 8;
            self.buf.push((self.acc >> self.nbits) as u8);
        }
        self.buf
    }
}

/// One decoded AC stream symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AcSymbol {
    /// A run of this many zero coefficients.
    Run(u64),
    /// A non-zero coefficient's value code (see [`from_code`]).
    Value(u64),
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

    /// Reads one symbol of the AC stream: a run code (`1` prefix) or a value
    /// code (`0` prefix). Same result and position as reading it with
    /// [`bit`](Self::bit) and [`code_tail`](Self::code_tail), but from a
    /// single 64-bit peek whenever the whole code is inside it.
    #[inline(always)]
    pub(crate) fn ac_symbol(&mut self) -> Result<AcSymbol, Corrupt> {
        // A peek holds at least 57 valid bits. The longest code handled here
        // is 2 + z + (z + 2) bits with z <= 26.
        let w = self.peek64();
        if w >> 62 == 0b11 {
            self.pos += 2;
            return Ok(AcSymbol::Run(1));
        }
        let run = w >> 63;
        let head = 1 + run as u32; // `0` or `10`
        let z = (w << head).leading_zeros();
        if z <= 26 {
            let n = z + 2;
            let v = (w << (head + z)) >> (64 - n);
            self.pos += (head + z + n) as usize;
            return Ok(if run == 1 { AcSymbol::Run(v) } else { AcSymbol::Value(v) });
        }
        // Long or corrupt code: the bit-by-bit path, errors included.
        self.pos += head as usize;
        let v = self.code_tail()?;
        Ok(if run == 1 { AcSymbol::Run(v) } else { AcSymbol::Value(v) })
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
    fn writer_matches_bitwise_reference() {
        let mut seed = 0x1234_5678_9ABC_DEF1u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            let mut w = BitWriter::default();
            let mut bits: Vec<bool> = Vec::new();
            for _ in 0..(rnd() % 500) {
                match rnd() % 8 {
                    0 => {
                        w.align();
                        while bits.len() % 8 != 0 {
                            bits.push(false);
                        }
                    }
                    k => {
                        let n = if k == 1 { (rnd() % 65) as u32 } else { (rnd() % 34) as u32 };
                        let v = rnd();
                        w.put(v, n);
                        bits.extend((0..n).rev().map(|i| (v >> i) & 1 == 1));
                    }
                }
            }
            while bits.len() % 8 != 0 {
                bits.push(false);
            }
            let want: Vec<u8> =
                bits.chunks(8).map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | b as u8)).collect();
            assert_eq!(w.into_bytes(), want);
        }
    }

    #[test]
    fn combined_run_value_matches_separate_codes() {
        let mut seed = 0x0DDB_1A5E_5BAD_5EEDu64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let (mut a, mut b) = (BitWriter::default(), BitWriter::default());
        for _ in 0..100_000 {
            // Runs up to a whole 8K slice plane; value codes up to 2 * 32768 + 1.
            let run = match rnd() % 3 {
                0 => 0,
                1 => (rnd() % 64) as u32,
                _ => (rnd() % 200_000) as u32,
            };
            let v = (((rnd() % 65_537) as u32) >> (rnd() % 17)).max(1);
            a.put_run_value(run, v);
            b.put_run(run);
            b.put_value(v);
        }
        assert!(a.into_bytes() == b.into_bytes());
    }

    /// The old symbol reader: one bit at a time.
    fn ac_symbol_bitwise(r: &mut BitReader) -> Result<AcSymbol, Corrupt> {
        if r.bit() == 1 {
            if r.bit() == 1 {
                Ok(AcSymbol::Run(1))
            } else {
                Ok(AcSymbol::Run(r.code_tail()?))
            }
        } else {
            Ok(AcSymbol::Value(r.code_tail()?))
        }
    }

    #[test]
    fn ac_symbol_matches_bitwise_reader() {
        let mut seed = 0xA5A5_5A5A_1234_4321u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for case in 0..3000 {
            // Real code sequences, sparse bits (long zero prefixes, corrupt
            // codes), and random bytes, each possibly truncated.
            let data: Vec<u8> = match case % 3 {
                0 => {
                    let mut w = BitWriter::default();
                    for _ in 0..(rnd() % 300) {
                        let v = (((rnd() % 65_537) as u32) >> (rnd() % 17)).max(1);
                        w.put_run_value((rnd() % 100_000) as u32 >> (rnd() % 17), v);
                    }
                    w.into_bytes()
                }
                1 => (0..rnd() % 64).map(|_| if rnd() % 16 == 0 { rnd() as u8 } else { 0 }).collect(),
                _ => (0..rnd() % 64).map(|_| rnd() as u8).collect(),
            };
            let cut = if data.is_empty() { 0 } else { (rnd() as usize) % (data.len() + 1) };
            let data = if case % 2 == 0 { &data[..] } else { &data[..cut] };
            let (mut a, mut b) = (BitReader::new(data), BitReader::new(data));
            for _ in 0..2000 {
                let (x, y) = (a.ac_symbol(), ac_symbol_bitwise(&mut b));
                assert_eq!(x, y, "case {case}");
                if x.is_err() {
                    break;
                }
                assert_eq!(a.pos, b.pos, "case {case}");
            }
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
