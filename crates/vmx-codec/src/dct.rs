//! 8x8 forward DCT + quantisation + zig-zag, and dequantisation + inverse DCT.
//!
//! A line-by-line port of `VMX_FDCT_8X8_QUANT_ZIG_128*` and
//! `VMX_ZIG_INVQUANTIZE_IDCT_8X8_128*` from libvmx (MIT, Open Media Transport
//! Contributors). The arithmetic is written once, generic over
//! [`crate::lanes::Isa`], in the same 128-bit operations as the reference, so
//! every rounding, saturation and wrap-around step matches it exactly whether
//! it runs on the portable lanes or on NEON / SSE2 (`crate::simd`).

use crate::lanes::Isa;
use crate::tables::*;

/// Views an 8-element slice of a constant table as an array (the bounds
/// check folds away once the offsets are constant).
#[inline(always)]
fn a8<T>(s: &[T]) -> &[T; 8] {
    s.try_into().expect("8-lane slice")
}

/// Sample precision of a plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Depth {
    /// 8-bit samples.
    Eight,
    /// 10-bit samples (carried MSB-aligned in 16-bit words).
    Ten,
}

/// Quantisation tables for one quality preset.
#[derive(Clone)]
pub(crate) struct QuantTables {
    /// Dequantisation multipliers (natural order).
    pub decode: [u16; 64],
    /// Encoder correction (`[0..64]`), reciprocal (`[64..128]`) and scale
    /// (`[128..192]`), natural order.
    pub encode: [u16; 192],
}

/// Bit length of a 16-bit value (`flss` in libvmx).
fn flss(v: u16) -> i32 {
    16 - v.leading_zeros() as i32
}

/// Port of libvmx `createReciprocal`: division by `divisor` as two unsigned
/// high multiplies.
fn reciprocal(divisor: u16) -> [u16; 3] {
    let b = flss(divisor) - 1;
    let mut r = 16 + b;
    let d = divisor as u32;
    let mut fq = (1u32 << r) / d;
    let fr = (1u32 << r) % d;
    let mut c = (divisor / 2) as u32;
    if fr == 0 {
        fq >>= 1;
        r -= 1;
    } else if fr <= d / 2 {
        c += 1;
    } else {
        fq += 1;
    }
    let s = 1u32 << (32 - r);
    [c as u16, fq as u16, s as u16]
}

impl QuantTables {
    /// Builds the tables for quality preset `index` (0 = finest).
    pub(crate) fn new(index: usize) -> Self {
        let mut decode = [0u16; 64];
        let mut encode = [0u16; 192];
        for y in 0..64 {
            decode[y] = if y == 0 {
                DEFAULT_QUANTIZATION_MATRIX[0]
            } else {
                DEFAULT_QUANTIZATION_MATRIX[y].wrapping_mul(QUALITY[index])
            };
            let rc = reciprocal(decode[y]);
            encode[y] = rc[0];
            encode[y + 64] = rc[1];
            encode[y + 128] = rc[2];
        }
        Self { decode, encode }
    }
}

/// One row pass of the forward transform (`ROW n` blocks in libvmx).
#[inline(always)]
fn fdct_row<S: Isa>(input: S::V16, tab: &[u16; 32], round: S::V32, shift: u32) -> S::V16 {
    let x0 = input;
    let x1 = S::shufflehi::<0b0001_1011>(x0);
    let x0 = S::shuffle32_16::<0b0100_0100>(x0);
    let x1 = S::shuffle32_16::<0b1110_1110>(x1);
    let s = S::adds(x0, x1);
    let d = S::subs(x0, x1);
    let x0 = S::unpacklo32(s, d);
    let x2 = S::shuffle32_16::<0b0100_1110>(x0);

    let t1 = S::madd(x2, S::load_u(a8(&tab[8..16])));
    let t2 = S::madd(x0, S::load_u(a8(&tab[16..24])));
    let t3 = S::madd(x2, S::load_u(a8(&tab[24..32])));
    let t4 = S::madd(x0, S::load_u(a8(&tab[0..8])));

    let a = S::srai32(S::add32(S::add32(t4, t1), round), shift);
    let b = S::srai32(S::add32(S::add32(t3, t2), round), shift);
    S::packs32(a, b)
}

/// Forward DCT, quantisation and zig-zag of one 8x8 block.
///
/// `rows` holds the already-widened input samples (8-bit: `0..=255`,
/// 10-bit: `0..=1023`), `add` is the level shift (-128 / -512 for luma and
/// alpha, 0 for chroma). Returns the 64 quantised coefficients in zig-zag
/// order.
pub(crate) fn fdct_quant_zig<S: Isa>(rows: &[[i16; 8]; 8], depth: Depth, matrix: &[u16; 192], add: i16) -> [i16; 64] {
    let col_shift = match depth {
        Depth::Eight => 3,
        Depth::Ten => 1,
    };
    const ROW_SHIFT: u32 = 16;
    let round = S::splat32(1 << (ROW_SHIFT - 1));

    let vadd = S::splat(add);
    let rows = rows.map(|r| S::load(&r));
    let in0 = S::adds(rows[0], vadd);
    let in1 = S::adds(rows[1], vadd);
    let in2 = S::adds(rows[2], vadd);
    let in3 = S::adds(rows[3], vadd);
    let in4 = S::adds(rows[4], vadd);
    let in5 = S::adds(rows[5], vadd);
    let in6 = S::adds(rows[6], vadd);
    let in7 = S::adds(rows[7], vadd);

    // Column pass (all eight columns at once).
    let mut xmm0 = in0;
    let mut xmm2 = in2;
    let xmm3 = xmm0;
    let xmm4 = xmm2;
    let mut xmm7 = in7;
    let mut xmm5 = in5;

    xmm0 = S::subs(xmm0, xmm7);
    xmm7 = S::adds(xmm7, xmm3);
    xmm2 = S::subs(xmm2, xmm5);
    xmm5 = S::adds(xmm5, xmm4);

    let mut xmm3 = in3;
    let mut xmm4 = in4;
    let xmm1 = xmm3;
    xmm3 = S::subs(xmm3, xmm4);
    xmm4 = S::adds(xmm4, xmm1);

    let mut xmm6 = in6;
    let mut xmm1 = in1;
    let tmp = xmm1;
    xmm1 = S::subs(xmm1, xmm6);
    xmm6 = S::adds(xmm6, tmp);

    let mut tm03 = S::subs(xmm7, xmm4);
    let mut tm12 = S::subs(xmm6, xmm5);
    xmm4 = S::adds(xmm4, xmm4);
    xmm5 = S::adds(xmm5, xmm5);

    let mut tp03 = S::adds(xmm4, tm03);
    let mut tp12 = S::adds(xmm5, tm12);

    xmm2 = S::slli16(xmm2, col_shift + 1);
    xmm1 = S::slli16(xmm1, col_shift + 1);
    tp03 = S::slli16(tp03, col_shift);
    tp12 = S::slli16(tp12, col_shift);
    tm03 = S::slli16(tm03, col_shift);
    tm12 = S::slli16(tm12, col_shift);
    xmm3 = S::slli16(xmm3, col_shift);
    xmm0 = S::slli16(xmm0, col_shift);

    let c4 = S::subs(tp03, tp12);
    let diff = S::subs(xmm1, xmm2);
    tp12 = S::adds(tp12, tp12);
    xmm2 = S::adds(xmm2, xmm2);
    let c0 = S::adds(tp12, c4);

    let sum = S::adds(xmm2, diff);

    let tan2v = S::splat(FDCT_TAN2 as i16);
    let c6 = S::subs(S::mulhi(tan2v, tm03), tm12);
    let c2 = S::adds(S::mulhi(tan2v, tm12), tm03);

    let sqrt2v = S::splat(FDCT_SQRT2 as i16);
    let rounder = S::splat(FDCT_ROUND1);

    let mut tp65 = S::mulhi(sum, sqrt2v);
    let c2 = S::or16(c2, rounder);
    let c6 = S::or16(c6, rounder);
    let tm65 = S::mulhi(diff, sqrt2v);
    tp65 = S::or16(tp65, rounder);

    let tm465 = S::subs(xmm3, tm65);
    let tm765 = S::subs(xmm0, tp65);
    let tp765 = S::adds(tp65, xmm0);
    let tp465 = S::adds(tm65, xmm3);

    let tan3v = S::splat(FDCT_TAN3 as i16);
    let tan1v = S::splat(FDCT_TAN1 as i16);

    let tmp3 = S::adds(S::mulhi(tm465, tan3v), tm465);
    let tmp4 = S::mulhi(tp465, tan1v);
    let tmp5 = S::adds(S::mulhi(tm765, tan3v), tm765);
    let tmp6 = S::mulhi(tp765, tan1v);

    let c1 = S::adds(tmp4, tp765);
    let c3 = S::subs(tm765, tmp3);
    let c5 = S::adds(tm465, tmp5);
    let c7 = S::subs(tmp6, tp465);

    // Row pass.
    let r = [
        fdct_row::<S>(c0, &FTAB1, round, ROW_SHIFT),
        fdct_row::<S>(c1, &FTAB2, round, ROW_SHIFT),
        fdct_row::<S>(c2, &FTAB3, round, ROW_SHIFT),
        fdct_row::<S>(c3, &FTAB4, round, ROW_SHIFT),
        fdct_row::<S>(c4, &FTAB1, round, ROW_SHIFT),
        fdct_row::<S>(c5, &FTAB4, round, ROW_SHIFT),
        fdct_row::<S>(c6, &FTAB3, round, ROW_SHIFT),
        fdct_row::<S>(c7, &FTAB2, round, ROW_SHIFT),
    ];

    // Quantisation: |x| + correction, two unsigned high multiplies, sign.
    let mut nat = [0i16; 64];
    for (k, rv) in r.iter().enumerate() {
        let o = k * 8;
        let mut b = S::abs16(*rv);
        b = S::add16(b, S::load_u(a8(&matrix[o..o + 8])));
        b = S::mulhi_u(b, S::load_u(a8(&matrix[64 + o..64 + o + 8])));
        b = S::mulhi_u(b, S::load_u(a8(&matrix[128 + o..128 + o + 8])));
        let q = S::sign16(b, *rv);
        nat[o..o + 8].copy_from_slice(&S::store(q));
    }

    let mut zz = [0i16; 64];
    for (i, z) in zz.iter_mut().enumerate() {
        *z = nat[ZIGZAG[i]];
    }
    zz
}

/// One row of the inverse transform (the paired `r_xmm` blocks in libvmx).
#[inline(always)]
fn idct_row<S: Isa>(a: S::V16, tab: &[i16; 32], round: S::V32, shift: u32) -> S::V16 {
    let x0 = S::shufflelo::<0xd8>(a);
    let x1 = S::madd(S::shuffle32_16::<0x00>(x0), S::load(a8(&tab[0..8])));
    let x3 = S::madd(S::shuffle32_16::<0x55>(x0), S::load(a8(&tab[16..24])));
    let x0 = S::shufflehi::<0xd8>(x0);
    let x2 = S::madd(S::shuffle32_16::<0xaa>(x0), S::load(a8(&tab[8..16])));
    let x0 = S::madd(S::shuffle32_16::<0xff>(x0), S::load(a8(&tab[24..32])));
    let x1 = S::add32(S::add32(x1, round), x2);
    let x0 = S::add32(x0, x3);
    let lo = S::srai32(S::add32(x0, x1), shift);
    let hi = S::srai32(S::sub32(x1, x0), shift);
    S::packs32(lo, S::shuffle32::<0x1b>(hi))
}

/// Dequantisation + inverse DCT of one block given in zig-zag order. Returns
/// the eight output rows (level shift applied, not yet clamped).
fn idct_core<S: Isa>(zz: &[i16; 64], depth: Depth, matrix: &[u16; 64], add: i16) -> [S::V16; 8] {
    let (deq_shift, row_round, row_shift, col_round, col_corr, col_shift) = match depth {
        Depth::Eight => (4, 1024, 11, 32i16, 31i16, 6),
        Depth::Ten => (2, 2048, 12, 16i16, 15i16, 5),
    };
    let mut nat = [0i16; 64];
    for (i, &z) in zz.iter().enumerate() {
        nat[ZIGZAG[i]] = z;
    }
    let a: [S::V16; 8] = core::array::from_fn(|k| {
        let o = k * 8;
        S::srai16(S::mullo(S::load(a8(&nat[o..o + 8])), S::load_u(a8(&matrix[o..o + 8]))), deq_shift)
    });
    let round = S::splat32(row_round);
    let row0 = idct_row::<S>(a[0], &TAB_I_04, round, row_shift);
    let row2 = idct_row::<S>(a[2], &TAB_I_26, round, row_shift);
    let row4 = idct_row::<S>(a[4], &TAB_I_04, round, row_shift);
    let row6 = idct_row::<S>(a[6], &TAB_I_26, round, row_shift);
    let row3 = idct_row::<S>(a[3], &TAB_I_35, round, row_shift);
    let row1 = idct_row::<S>(a[1], &TAB_I_17, round, row_shift);
    let row5 = idct_row::<S>(a[5], &TAB_I_35, round, row_shift);
    let row7 = idct_row::<S>(a[7], &TAB_I_17, round, row_shift);

    // Column pass, transcribed from the reference register by register.
    let one = S::splat(1);
    let rnd_col = S::splat(col_round);
    let rnd_corr = S::splat(col_corr);

    let mut r1 = S::splat(TG_3_16);
    let mut r2 = row5;
    let mut r3 = row3;
    let mut r0 = S::mulhi(row5, r1);
    r1 = S::mulhi(r1, r3);
    let mut r5 = S::splat(TG_1_16);
    let mut r6 = row7;
    let mut r4 = S::mulhi(row7, r5);
    r0 = S::adds(r0, r2);
    r5 = S::mulhi(r5, row1);
    r1 = S::adds(r1, r3);
    let mut r7 = row6;
    r0 = S::adds(r0, r3);
    r3 = S::splat(TG_2_16);
    r2 = S::subs(r2, r1);
    r7 = S::mulhi(r7, r3);
    r1 = r0;
    r3 = S::mulhi(r3, row2);
    r5 = S::subs(r5, r6);
    r4 = S::adds(r4, row1);
    r0 = S::adds(r0, r4);
    r0 = S::adds(r0, one);
    r4 = S::subs(r4, r1);
    r6 = r5;
    r5 = S::subs(r5, r2);
    r5 = S::adds(r5, one);
    r6 = S::adds(r6, r2);
    let temp7 = r0;
    r1 = r4;
    r0 = S::splat(COS_4_16);
    r4 = S::adds(r4, r5);
    r2 = S::splat(COS_4_16);
    r2 = S::mulhi(r2, r4);
    let temp3 = r6;
    r1 = S::subs(r1, r5);
    r7 = S::adds(r7, row2);
    r3 = S::subs(r3, row6);
    r6 = row0;
    r0 = S::mulhi(r0, r1);
    r5 = row4;
    r5 = S::adds(r5, r6);
    r6 = S::subs(r6, row4);
    r4 = S::adds(r4, r2);
    r4 = S::or16(r4, one);
    r0 = S::adds(r0, r1);
    r0 = S::or16(r0, one);
    r2 = r5;
    r5 = S::adds(r5, r7);
    r1 = r6;
    r5 = S::adds(r5, rnd_col);
    r2 = S::subs(r2, r7);
    r7 = temp7;
    r6 = S::adds(r6, r3);
    r6 = S::adds(r6, rnd_col);
    r7 = S::adds(r7, r5);
    r7 = S::srai16(r7, col_shift);
    r1 = S::subs(r1, r3);
    r1 = S::adds(r1, rnd_corr);
    r3 = r6;
    r2 = S::adds(r2, rnd_corr);
    r6 = S::adds(r6, r4);

    let vadd = S::splat(add);
    let out0 = S::adds(r7, vadd);
    r6 = S::srai16(r6, col_shift);
    let out1 = S::adds(r6, vadd);

    r7 = r1;
    r1 = S::adds(r1, r0);
    r1 = S::srai16(r1, col_shift);
    r6 = temp3;
    r7 = S::subs(r7, r0);
    r7 = S::srai16(r7, col_shift);
    let out2 = S::adds(r1, vadd);
    r5 = S::subs(r5, temp7);
    r5 = S::srai16(r5, col_shift);
    let out7 = S::adds(r5, vadd);
    r3 = S::subs(r3, r4);
    r6 = S::adds(r6, r2);
    r2 = S::subs(r2, temp3);
    r6 = S::srai16(r6, col_shift);
    r2 = S::srai16(r2, col_shift);
    let out3 = S::adds(r6, vadd);
    r3 = S::srai16(r3, col_shift);
    let out4 = S::adds(r2, vadd);
    let out5 = S::adds(r7, vadd);
    let out6 = S::adds(r3, vadd);

    [out0, out1, out2, out3, out4, out5, out6, out7]
}

/// Dequantise + inverse DCT, writing 8-bit samples to `dst` (row stride
/// `stride` bytes).
pub(crate) fn idct8<S: Isa>(zz: &[i16; 64], matrix: &[u16; 64], dst: &mut [u8], stride: usize, add: i16) {
    let rows = idct_core::<S>(zz, Depth::Eight, matrix, add);
    for (k, row) in rows.iter().enumerate() {
        dst[k * stride..k * stride + 8].copy_from_slice(&S::packus8(*row));
    }
}

/// Dequantise + inverse DCT, writing 16-bit samples (10 significant bits,
/// MSB-aligned) to `dst` (row stride `stride` samples).
pub(crate) fn idct16<S: Isa>(zz: &[i16; 64], matrix: &[u16; 64], dst: &mut [u16], stride: usize, add: i16) {
    let rows = idct_core::<S>(zz, Depth::Ten, matrix, add);
    let hi = S::splat(1023);
    let lo = S::splat(0);
    for (k, row) in rows.iter().enumerate() {
        let v = S::store(S::slli16(S::max16(S::min16(*row, hi), lo), 6));
        for (i, s) in v.iter().enumerate() {
            dst[k * stride + i] = *s as u16;
        }
    }
}

/// Fills an 8x8 8-bit block with its DC value (`VMX_BROADCAST_DC_8X8_128`).
pub(crate) fn broadcast_dc8(dc: i16, dst: &mut [u8], stride: usize, add: i16) {
    let v = dc.wrapping_add(4) >> 3;
    let v = v.wrapping_add(add).clamp(0, 255) as u8;
    for k in 0..8 {
        dst[k * stride..k * stride + 8].fill(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lanes::Scalar;
    use crate::simd::Native;
    use crate::tables::QUALITY_COUNT;

    /// Deterministic xorshift generator for the equivalence tests.
    pub(crate) struct Rng(pub u64);

    impl Rng {
        pub(crate) fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// A 16-bit value biased towards the ends of the range, where
        /// saturation and wrap-around happen.
        pub(crate) fn edgy(&mut self) -> i16 {
            const EDGES: [i16; 10] = [i16::MIN, i16::MIN + 1, -16384, -1024, -1, 0, 1, 1023, 16383, i16::MAX];
            match self.next() % 4 {
                0 => EDGES[(self.next() % EDGES.len() as u64) as usize],
                1 => (self.next() % 1024) as i16,
                2 => (self.next() % 256) as i16,
                _ => self.next() as i16,
            }
        }
    }

    const ITERS: usize = 100_000;

    #[test]
    fn fdct_native_matches_scalar() {
        let presets: Vec<QuantTables> = (0..QUALITY_COUNT).map(QuantTables::new).collect();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for iter in 0..ITERS {
            let depth = if iter % 2 == 0 { Depth::Eight } else { Depth::Ten };
            let add = match iter % 5 {
                0 => -128,
                1 => -512,
                2 => 0,
                _ => rng.edgy(),
            };
            // Real presets, and arbitrary matrices for the quantiser corners.
            let mut matrix = presets[iter % QUALITY_COUNT].encode;
            if iter % 7 == 0 {
                matrix.iter_mut().for_each(|m| *m = rng.next() as u16);
            }
            let mode = rng.next() % 3;
            let mut rows = [[0i16; 8]; 8];
            for v in rows.iter_mut().flatten() {
                *v = match mode {
                    0 => (rng.next() % 256) as i16,
                    1 => (rng.next() % 1024) as i16,
                    _ => rng.edgy(),
                };
            }
            let a = fdct_quant_zig::<Native>(&rows, depth, &matrix, add);
            let b = fdct_quant_zig::<Scalar>(&rows, depth, &matrix, add);
            assert_eq!(a, b, "{depth:?} add {add} rows {rows:?} matrix {matrix:?}");
        }
    }

    #[test]
    fn idct_native_matches_scalar() {
        let presets: Vec<QuantTables> = (0..QUALITY_COUNT).map(QuantTables::new).collect();
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        for iter in 0..ITERS {
            let add = match iter % 4 {
                0 => 128,
                1 => 512,
                2 => 0,
                _ => rng.edgy(),
            };
            let mut matrix = presets[iter % QUALITY_COUNT].decode;
            if iter % 7 == 0 {
                matrix.iter_mut().for_each(|m| *m = rng.next() as u16);
            }
            // Sparse small coefficients (what real streams hold) or anything.
            let mut zz = [0i16; 64];
            let dense = rng.next() % 2 == 0;
            for z in zz.iter_mut() {
                *z = if dense {
                    rng.edgy()
                } else if rng.next() % 4 == 0 {
                    (rng.next() % 64) as i16 - 32
                } else {
                    0
                };
            }
            let (mut a8, mut b8) = ([0u8; 80], [0u8; 80]);
            idct8::<Native>(&zz, &matrix, &mut a8, 10, add);
            idct8::<Scalar>(&zz, &matrix, &mut b8, 10, add);
            assert_eq!(a8, b8, "8-bit add {add} zz {zz:?} matrix {matrix:?}");
            let (mut a16, mut b16) = ([0u16; 72], [0u16; 72]);
            idct16::<Native>(&zz, &matrix, &mut a16, 9, add);
            idct16::<Scalar>(&zz, &matrix, &mut b16, 9, add);
            assert_eq!(a16, b16, "10-bit add {add} zz {zz:?} matrix {matrix:?}");
        }
    }

    #[test]
    fn reciprocal_of_16() {
        assert_eq!(reciprocal(16), [8, 32768, 8192]);
    }

    #[test]
    fn flat_block_roundtrip() {
        let q = QuantTables::new(0);
        let rows = [[200i16; 8]; 8];
        let zz = fdct_quant_zig::<Native>(&rows, Depth::Eight, &q.encode, -128);
        assert!(zz[1..].iter().all(|&c| c == 0));
        let mut out = [0u8; 64];
        idct8::<Native>(&zz, &q.decode, &mut out, 8, 128);
        assert!(out.iter().all(|&p| p == 200), "{out:?}");
    }
}
