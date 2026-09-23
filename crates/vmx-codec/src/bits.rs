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

/// Bytes of headroom [`BitAcc::reserve`] guarantees: more than one block of
/// codes (64 codes of at most 67 bits, plus the 8-byte store).
const SLACK: usize = 1024;

/// The accumulator of a [`BitWriter`], usable on its own with the output
/// buffer passed separately. Being a small `Copy` value, a local copy stays
/// in registers across a hot loop (a `&mut BitWriter` would be reloaded and
/// stored for every code).
///
/// Flushing is branchless: every `put` stores the accumulator's pending bits
/// as 8 bytes at `pos` and advances `pos` by the whole bytes among them. The
/// bytes after `pos` are scratch that later stores overwrite, so the buffer
/// (whose length is only its initialised size) must keep 8 bytes of room
/// past `pos`; callers make sure of that with [`reserve`](Self::reserve).
#[derive(Clone, Copy, Default)]
pub(crate) struct BitAcc {
    acc: u64,
    /// Pending bits in the low end of `acc` (< 8 between calls).
    nbits: u32,
    /// Bytes of the stream written so far.
    pos: usize,
}

impl BitAcc {
    /// Ensures `out` has room for [`SLACK`] more bytes (one block of codes).
    #[inline(always)]
    pub(crate) fn reserve(&self, out: &mut Vec<u8>) {
        if out.len() < self.pos + SLACK {
            grow(out, self.pos + SLACK);
        }
    }

    /// Appends the low `n` bits of `v` (`n <= 64`).
    #[inline(always)]
    pub(crate) fn put(&mut self, v: u64, n: u32, out: &mut [u8]) {
        if n > 57 {
            *self = self.put_long(v, n, out);
        } else {
            self.put57(v, n, out);
        }
    }

    /// Out of line and by value, so callers' accumulators never have their
    /// address taken.
    #[cold]
    #[inline(never)]
    fn put_long(mut self, v: u64, n: u32, out: &mut [u8]) -> Self {
        self.put57(v >> 32, n - 32, out);
        self.put57(v & 0xFFFF_FFFF, 32, out);
        self
    }

    /// Appends the low `n <= 57` bits of `v`.
    #[inline(always)]
    fn put57(&mut self, v: u64, n: u32, out: &mut [u8]) {
        // At most 7 + 57 = 64 pending bits; bits above them are stale.
        self.acc = (self.acc << n) | (v & ((1u64 << n) - 1));
        self.nbits += n;
        // Left-justify the pending bits (a shift by 64 wraps to 0 when there
        // are none; the stored bytes are then scratch past `pos`).
        let top = self.acc.wrapping_shl(64 - self.nbits);
        out[self.pos..self.pos + 8].copy_from_slice(&top.to_be_bytes());
        self.pos += (self.nbits / 8) as usize;
        self.nbits %= 8;
    }

    /// Value code for `v >= 1`.
    #[inline(always)]
    pub(crate) fn put_value(&mut self, v: u32, out: &mut [u8]) {
        let bl = 32 - v.leading_zeros();
        self.put(v as u64, 2 * bl - 1, out);
    }

    /// Run code for `n >= 1`; nothing for `n == 0`.
    #[inline(always)]
    pub(crate) fn put_run(&mut self, n: u32, out: &mut [u8]) {
        if n == 0 {
            return;
        }
        let bl = 32 - n.leading_zeros();
        self.put((1u64 << (2 * bl - 1)) | n as u64, 2 * bl, out);
    }

    /// Run code for `run` (nothing if 0) followed by the value code for
    /// `v >= 1`, written with a single `put` when they fit in 57 bits.
    #[inline(always)]
    pub(crate) fn put_run_value(&mut self, run: u32, v: u32, out: &mut [u8]) {
        let vl = 2 * (32 - v.leading_zeros()) - 1;
        let rbl = 32 - run.leading_zeros();
        let rl = 2 * rbl;
        if rl + vl <= 57 {
            // run == 0 gives rl == 0 and an empty run code.
            let rc = ((1u64 << rl) >> 1) | run as u64;
            self.put57((rc << vl) | v as u64, rl + vl, out);
        } else {
            self.put_run(run, out);
            self.put_value(v, out);
        }
    }

    /// Pads with zero bits to the next byte boundary.
    #[inline(always)]
    pub(crate) fn align(&mut self, out: &mut [u8]) {
        self.put57(0, (8 - self.nbits) % 8, out);
    }
}

#[cold]
#[inline(never)]
fn grow(out: &mut Vec<u8>, min: usize) {
    out.resize(min.max(2 * out.len()), 0);
}

/// Growable MSB-first bit writer: a [`BitAcc`] and its output buffer.
#[derive(Default)]
pub(crate) struct BitWriter {
    pub(crate) buf: Vec<u8>,
    pub(crate) acc: BitAcc,
}

impl BitWriter {
    pub(crate) fn with_capacity(n: usize) -> Self {
        Self { buf: Vec::with_capacity(n), acc: BitAcc::default() }
    }

    /// Appends the low `n` bits of `v` (`n <= 64`).
    #[cfg(test)]
    pub(crate) fn put(&mut self, v: u64, n: u32) {
        self.acc.reserve(&mut self.buf);
        self.acc.put(v, n, &mut self.buf);
    }

    /// Value code for `v >= 1`.
    #[cfg(test)]
    pub(crate) fn put_value(&mut self, v: u32) {
        self.acc.reserve(&mut self.buf);
        self.acc.put_value(v, &mut self.buf);
    }

    /// Run code for `n >= 1`; nothing for `n == 0`.
    #[cfg(test)]
    pub(crate) fn put_run(&mut self, n: u32) {
        self.acc.reserve(&mut self.buf);
        self.acc.put_run(n, &mut self.buf);
    }

    /// See [`BitAcc::put_run_value`].
    #[cfg(test)]
    pub(crate) fn put_run_value(&mut self, run: u32, v: u32) {
        self.acc.reserve(&mut self.buf);
        self.acc.put_run_value(run, v, &mut self.buf);
    }

    /// Pads with zero bits to the next byte boundary.
    #[cfg(test)]
    pub(crate) fn align(&mut self) {
        self.acc.reserve(&mut self.buf);
        self.acc.align(&mut self.buf);
    }

    pub(crate) fn into_bytes(mut self) -> Vec<u8> {
        self.acc.reserve(&mut self.buf);
        self.acc.align(&mut self.buf);
        self.buf.truncate(self.acc.pos);
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

/// A [`BitReader`] with the next bits cached in a register, for the AC
/// symbol loop: most symbols then decode without a load on the dependency
/// chain. `bits` holds `count` valid bits MSB-first (zeros below them);
/// `next` is the next byte to load, so the stream position is
/// `8 * next - count`.
#[derive(Clone, Copy)]
pub(crate) struct AcCursor<'a> {
    data: &'a [u8],
    next: usize,
    bits: u64,
    count: u32,
}

impl<'a> AcCursor<'a> {
    pub(crate) fn new(r: BitReader<'a>) -> Self {
        let mut c = Self { data: r.data, next: r.pos >> 3, bits: 0, count: 0 };
        c.refill();
        let skip = (r.pos & 7) as u32;
        c.bits <<= skip;
        c.count -= skip;
        c
    }

    /// Back to a plain reader at the same position.
    pub(crate) fn reader(&self) -> BitReader<'a> {
        BitReader { data: self.data, pos: 8 * self.next - self.count as usize }
    }

    /// Tops the cache up to 57..=63 bits (past the end: `1` bits, as
    /// [`BitReader`] reads them).
    #[inline(always)]
    fn refill(&mut self) {
        let w = match self.data.get(self.next..self.next + 8) {
            Some(b) => u64::from_be_bytes(b.try_into().expect("8 bytes")),
            None => BitReader::peek_tail(self.data, self.next),
        };
        self.bits |= w >> self.count;
        let bytes = (63 - self.count) / 8;
        self.next += bytes as usize;
        self.count += 8 * bytes;
    }

    #[inline(always)]
    fn consume(&mut self, n: u32) {
        self.bits <<= n;
        self.count -= n;
    }

    /// Reads one symbol of the AC stream: a run code (`1` prefix) or a
    /// value code (`0` prefix). Same result and position as reading it with
    /// [`BitReader::bit`] and [`BitReader::code_tail`].
    #[inline(always)]
    pub(crate) fn ac_symbol(&mut self) -> Result<AcSymbol, Corrupt> {
        if self.count < 32 {
            self.refill();
        }
        // At least 32 valid bits: enough for any code of up to 2 + z + (z + 2)
        // bits with z <= 14 (coefficients below 2^16).
        let w = self.bits;
        if w >> 62 == 0b11 {
            self.consume(2);
            return Ok(AcSymbol::Run(1));
        }
        let run = w >> 63;
        let head = 1 + run as u32; // `0` or `10`
        let z = (w << head).leading_zeros();
        if z > 14 {
            let (c, sym) = self.ac_symbol_slow(head, run == 1)?;
            *self = c;
            return Ok(sym);
        }
        let n = z + 2;
        let v = (w << (head + z)) >> (64 - n);
        self.consume(head + z + n);
        Ok(if run == 1 { AcSymbol::Run(v) } else { AcSymbol::Value(v) })
    }

    /// Long or corrupt code: the bit-by-bit path, errors included. Out of
    /// line and by value, so the caller's cursor never has its address taken
    /// (which would keep it out of registers).
    #[cold]
    #[inline(never)]
    fn ac_symbol_slow(self, head: u32, run: bool) -> Result<(Self, AcSymbol), Corrupt> {
        let mut r = self.reader();
        r.pos += head as usize;
        let v = r.code_tail()?;
        Ok((Self::new(r), if run { AcSymbol::Run(v) } else { AcSymbol::Value(v) }))
    }
}

/// Error raised when a bitstream is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Corrupt;

/// MSB-first bit reader over one slice stream. Reads past the end return
/// `1` bits, like the `0xFF` guard bytes libvmx places after each stream, so
/// truncated input terminates quickly instead of looping.
#[derive(Clone, Copy)]
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
        let w = match self.data.get(byte..byte + 8) {
            Some(b) => u64::from_be_bytes(b.try_into().expect("8 bytes")),
            None => Self::peek_tail(self.data, byte),
        };
        w << (self.pos & 7)
    }

    /// The last bytes of the stream, padded with `0xFF`.
    #[cold]
    #[inline(never)]
    fn peek_tail(data: &[u8], byte: usize) -> u64 {
        let mut w = [0xFFu8; 8];
        if byte < data.len() {
            let n = data.len() - byte;
            w[..n].copy_from_slice(&data[byte..]);
        }
        u64::from_be_bytes(w)
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
            let want: Vec<u8> = bits.chunks(8).map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | b as u8)).collect();
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
    fn ac_cursor_matches_bitwise_reader() {
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
            // Start at every bit offset of the first byte.
            let mut b = BitReader::new(data);
            b.pos = case % 8;
            let mut a = AcCursor::new(b);
            for _ in 0..2000 {
                let (x, y) = (a.ac_symbol(), ac_symbol_bitwise(&mut b));
                assert_eq!(x, y, "case {case}");
                if x.is_err() {
                    break;
                }
                assert_eq!(a.reader().pos, b.pos, "case {case}");
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
