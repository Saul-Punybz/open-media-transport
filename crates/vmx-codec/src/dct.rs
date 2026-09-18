//! 8x8 forward DCT + quantisation + zig-zag, and dequantisation + inverse DCT.
//!
//! A line-by-line port of `VMX_FDCT_8X8_QUANT_ZIG_128*` and
//! `VMX_ZIG_INVQUANTIZE_IDCT_8X8_128*` from libvmx (MIT, Open Media Transport
//! Contributors). The arithmetic is expressed with [`crate::lanes`] so every
//! rounding, saturation and wrap-around step matches the reference exactly.

use crate::lanes::*;
use crate::tables::*;

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
fn fdct_row(input: V16, tab: &[u16; 32], round: V32, shift: u32) -> V16 {
    let x0 = input;
    let x1 = shufflehi(x0, 0b0001_1011);
    let x0 = shuffle32_16(x0, 0b0100_0100);
    let x1 = shuffle32_16(x1, 0b1110_1110);
    let s = adds(x0, x1);
    let d = subs(x0, x1);
    let x0 = unpacklo32(s, d);
    let x2 = shuffle32_16(x0, 0b0100_1110);

    let t1 = madd(x2, ld(&tab[8..16]));
    let t2 = madd(x0, ld(&tab[16..24]));
    let t3 = madd(x2, ld(&tab[24..32]));
    let t4 = madd(x0, ld(&tab[0..8]));

    let a = srai32(add32(add32(t4, t1), round), shift);
    let b = srai32(add32(add32(t3, t2), round), shift);
    packs32(a, b)
}

/// Forward DCT, quantisation and zig-zag of one 8x8 block.
///
/// `rows` holds the already-widened input samples (8-bit: `0..=255`,
/// 10-bit: `0..=1023`), `add` is the level shift (-128 / -512 for luma and
/// alpha, 0 for chroma). Returns the 64 quantised coefficients in zig-zag
/// order.
pub(crate) fn fdct_quant_zig(rows: &[V16; 8], depth: Depth, matrix: &[u16; 192], add: i16) -> [i16; 64] {
    let col_shift = match depth {
        Depth::Eight => 3,
        Depth::Ten => 1,
    };
    const ROW_SHIFT: u32 = 16;
    let round: V32 = [1 << (ROW_SHIFT - 1); 4];

    let vadd = splat(add);
    let in0 = adds(rows[0], vadd);
    let in1 = adds(rows[1], vadd);
    let in2 = adds(rows[2], vadd);
    let in3 = adds(rows[3], vadd);
    let in4 = adds(rows[4], vadd);
    let in5 = adds(rows[5], vadd);
    let in6 = adds(rows[6], vadd);
    let in7 = adds(rows[7], vadd);

    // Column pass (all eight columns at once).
    let mut xmm0 = in0;
    let mut xmm2 = in2;
    let xmm3 = xmm0;
    let xmm4 = xmm2;
    let mut xmm7 = in7;
    let mut xmm5 = in5;

    xmm0 = subs(xmm0, xmm7);
    xmm7 = adds(xmm7, xmm3);
    xmm2 = subs(xmm2, xmm5);
    xmm5 = adds(xmm5, xmm4);

    let mut xmm3 = in3;
    let mut xmm4 = in4;
    let xmm1 = xmm3;
    xmm3 = subs(xmm3, xmm4);
    xmm4 = adds(xmm4, xmm1);

    let mut xmm6 = in6;
    let mut xmm1 = in1;
    let tmp = xmm1;
    xmm1 = subs(xmm1, xmm6);
    xmm6 = adds(xmm6, tmp);

    let mut tm03 = subs(xmm7, xmm4);
    let mut tm12 = subs(xmm6, xmm5);
    xmm4 = adds(xmm4, xmm4);
    xmm5 = adds(xmm5, xmm5);

    let mut tp03 = adds(xmm4, tm03);
    let mut tp12 = adds(xmm5, tm12);

    xmm2 = slli16(xmm2, col_shift + 1);
    xmm1 = slli16(xmm1, col_shift + 1);
    tp03 = slli16(tp03, col_shift);
    tp12 = slli16(tp12, col_shift);
    tm03 = slli16(tm03, col_shift);
    tm12 = slli16(tm12, col_shift);
    xmm3 = slli16(xmm3, col_shift);
    xmm0 = slli16(xmm0, col_shift);

    let c4 = subs(tp03, tp12);
    let diff = subs(xmm1, xmm2);
    tp12 = adds(tp12, tp12);
    xmm2 = adds(xmm2, xmm2);
    let c0 = adds(tp12, c4);

    let sum = adds(xmm2, diff);

    let tan2v = splat(FDCT_TAN2 as i16);
    let c6 = subs(mulhi(tan2v, tm03), tm12);
    let c2 = adds(mulhi(tan2v, tm12), tm03);

    let sqrt2v = splat(FDCT_SQRT2 as i16);
    let rounder = splat(FDCT_ROUND1);

    let mut tp65 = mulhi(sum, sqrt2v);
    let c2 = or16(c2, rounder);
    let c6 = or16(c6, rounder);
    let tm65 = mulhi(diff, sqrt2v);
    tp65 = or16(tp65, rounder);

    let tm465 = subs(xmm3, tm65);
    let tm765 = subs(xmm0, tp65);
    let tp765 = adds(tp65, xmm0);
    let tp465 = adds(tm65, xmm3);

    let tan3v = splat(FDCT_TAN3 as i16);
    let tan1v = splat(FDCT_TAN1 as i16);

    let tmp3 = adds(mulhi(tm465, tan3v), tm465);
    let tmp4 = mulhi(tp465, tan1v);
    let tmp5 = adds(mulhi(tm765, tan3v), tm765);
    let tmp6 = mulhi(tp765, tan1v);

    let c1 = adds(tmp4, tp765);
    let c3 = subs(tm765, tmp3);
    let c5 = adds(tm465, tmp5);
    let c7 = subs(tmp6, tp465);

    // Row pass.
    let r = [
        fdct_row(c0, &FTAB1, round, ROW_SHIFT),
        fdct_row(c1, &FTAB2, round, ROW_SHIFT),
        fdct_row(c2, &FTAB3, round, ROW_SHIFT),
        fdct_row(c3, &FTAB4, round, ROW_SHIFT),
        fdct_row(c4, &FTAB1, round, ROW_SHIFT),
        fdct_row(c5, &FTAB4, round, ROW_SHIFT),
        fdct_row(c6, &FTAB3, round, ROW_SHIFT),
        fdct_row(c7, &FTAB2, round, ROW_SHIFT),
    ];

    // Quantisation: |x| + correction, two unsigned high multiplies, sign.
    let mut nat = [0i16; 64];
    for (k, rv) in r.iter().enumerate() {
        let o = k * 8;
        let mut b = abs16(*rv);
        b = add16(b, ld(&matrix[o..o + 8]));
        b = mulhi_u(b, ld(&matrix[64 + o..64 + o + 8]));
        b = mulhi_u(b, ld(&matrix[128 + o..128 + o + 8]));
        let q = sign16(b, *rv);
        nat[o..o + 8].copy_from_slice(&q);
    }

    let mut zz = [0i16; 64];
    for (i, z) in zz.iter_mut().enumerate() {
        *z = nat[ZIGZAG[i]];
    }
    zz
}

/// One row of the inverse transform (the paired `r_xmm` blocks in libvmx).
#[inline(always)]
fn idct_row(a: V16, tab: &[i16; 32], round: V32, shift: u32) -> V16 {
    let x0 = shufflelo(a, 0xd8);
    let x1 = madd(shuffle32_16(x0, 0x00), lds(&tab[0..8]));
    let x3 = madd(shuffle32_16(x0, 0x55), lds(&tab[16..24]));
    let x0 = shufflehi(x0, 0xd8);
    let x2 = madd(shuffle32_16(x0, 0xaa), lds(&tab[8..16]));
    let x0 = madd(shuffle32_16(x0, 0xff), lds(&tab[24..32]));
    let x1 = add32(add32(x1, round), x2);
    let x0 = add32(x0, x3);
    let lo = srai32(add32(x0, x1), shift);
    let hi = srai32(sub32(x1, x0), shift);
    packs32(lo, shuffle32(hi, 0x1b))
}

/// Output rows of one inverse-transformed block before packing.
pub(crate) type IdctRows = [V16; 8];

/// Dequantisation + inverse DCT of one block given in zig-zag order. Returns
/// the eight output rows (level shift applied, not yet clamped).
fn idct_core(zz: &[i16; 64], depth: Depth, matrix: &[u16; 64], add: i16) -> IdctRows {
    let (deq_shift, row_round, row_shift, col_round, col_corr, col_shift) = match depth {
        Depth::Eight => (4, 1024, 11, 32i16, 31i16, 6),
        Depth::Ten => (2, 2048, 12, 16i16, 15i16, 5),
    };
    let mut nat = [0i16; 64];
    for (i, &z) in zz.iter().enumerate() {
        nat[ZIGZAG[i]] = z;
    }
    let mut a = [[0i16; 8]; 8];
    for (k, row) in a.iter_mut().enumerate() {
        let o = k * 8;
        *row = srai16(mullo(lds(&nat[o..o + 8]), ld(&matrix[o..o + 8])), deq_shift);
    }
    let round: V32 = [row_round; 4];
    let row0 = idct_row(a[0], &TAB_I_04, round, row_shift);
    let row2 = idct_row(a[2], &TAB_I_26, round, row_shift);
    let row4 = idct_row(a[4], &TAB_I_04, round, row_shift);
    let row6 = idct_row(a[6], &TAB_I_26, round, row_shift);
    let row3 = idct_row(a[3], &TAB_I_35, round, row_shift);
    let row1 = idct_row(a[1], &TAB_I_17, round, row_shift);
    let row5 = idct_row(a[5], &TAB_I_35, round, row_shift);
    let row7 = idct_row(a[7], &TAB_I_17, round, row_shift);

    // Column pass, transcribed from the reference register by register.
    let one = splat(1);
    let rnd_col = splat(col_round);
    let rnd_corr = splat(col_corr);

    let mut r1 = splat(TG_3_16);
    let mut r2 = row5;
    let mut r3 = row3;
    let mut r0 = mulhi(row5, r1);
    r1 = mulhi(r1, r3);
    let mut r5 = splat(TG_1_16);
    let mut r6 = row7;
    let mut r4 = mulhi(row7, r5);
    r0 = adds(r0, r2);
    r5 = mulhi(r5, row1);
    r1 = adds(r1, r3);
    let mut r7 = row6;
    r0 = adds(r0, r3);
    r3 = splat(TG_2_16);
    r2 = subs(r2, r1);
    r7 = mulhi(r7, r3);
    r1 = r0;
    r3 = mulhi(r3, row2);
    r5 = subs(r5, r6);
    r4 = adds(r4, row1);
    r0 = adds(r0, r4);
    r0 = adds(r0, one);
    r4 = subs(r4, r1);
    r6 = r5;
    r5 = subs(r5, r2);
    r5 = adds(r5, one);
    r6 = adds(r6, r2);
    let temp7 = r0;
    r1 = r4;
    r0 = splat(COS_4_16);
    r4 = adds(r4, r5);
    r2 = splat(COS_4_16);
    r2 = mulhi(r2, r4);
    let temp3 = r6;
    r1 = subs(r1, r5);
    r7 = adds(r7, row2);
    r3 = subs(r3, row6);
    r6 = row0;
    r0 = mulhi(r0, r1);
    r5 = row4;
    r5 = adds(r5, r6);
    r6 = subs(r6, row4);
    r4 = adds(r4, r2);
    r4 = or16(r4, one);
    r0 = adds(r0, r1);
    r0 = or16(r0, one);
    r2 = r5;
    r5 = adds(r5, r7);
    r1 = r6;
    r5 = adds(r5, rnd_col);
    r2 = subs(r2, r7);
    r7 = temp7;
    r6 = adds(r6, r3);
    r6 = adds(r6, rnd_col);
    r7 = adds(r7, r5);
    r7 = srai16(r7, col_shift);
    r1 = subs(r1, r3);
    r1 = adds(r1, rnd_corr);
    r3 = r6;
    r2 = adds(r2, rnd_corr);
    r6 = adds(r6, r4);

    let vadd = splat(add);
    let out0 = adds(r7, vadd);
    r6 = srai16(r6, col_shift);
    let out1 = adds(r6, vadd);

    r7 = r1;
    r1 = adds(r1, r0);
    r1 = srai16(r1, col_shift);
    r6 = temp3;
    r7 = subs(r7, r0);
    r7 = srai16(r7, col_shift);
    let out2 = adds(r1, vadd);
    r5 = subs(r5, temp7);
    r5 = srai16(r5, col_shift);
    let out7 = adds(r5, vadd);
    r3 = subs(r3, r4);
    r6 = adds(r6, r2);
    r2 = subs(r2, temp3);
    r6 = srai16(r6, col_shift);
    r2 = srai16(r2, col_shift);
    let out3 = adds(r6, vadd);
    r3 = srai16(r3, col_shift);
    let out4 = adds(r2, vadd);
    let out5 = adds(r7, vadd);
    let out6 = adds(r3, vadd);

    [out0, out1, out2, out3, out4, out5, out6, out7]
}

/// Dequantise + inverse DCT, writing 8-bit samples to `dst` (row stride
/// `stride` bytes).
pub(crate) fn idct8(zz: &[i16; 64], matrix: &[u16; 64], dst: &mut [u8], stride: usize, add: i16) {
    let rows = idct_core(zz, Depth::Eight, matrix, add);
    for (k, row) in rows.iter().enumerate() {
        dst[k * stride..k * stride + 8].copy_from_slice(&packus8(*row));
    }
}

/// Dequantise + inverse DCT, writing 16-bit samples (10 significant bits,
/// MSB-aligned) to `dst` (row stride `stride` samples).
pub(crate) fn idct16(zz: &[i16; 64], matrix: &[u16; 64], dst: &mut [u16], stride: usize, add: i16) {
    let rows = idct_core(zz, Depth::Ten, matrix, add);
    let hi = splat(1023);
    let lo = splat(0);
    for (k, row) in rows.iter().enumerate() {
        let v = slli16(max16(min16(*row, hi), lo), 6);
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

    #[test]
    fn reciprocal_of_16() {
        assert_eq!(reciprocal(16), [8, 32768, 8192]);
    }

    #[test]
    fn flat_block_roundtrip() {
        let q = QuantTables::new(0);
        let rows = [[200i16; 8]; 8];
        let zz = fdct_quant_zig(&rows, Depth::Eight, &q.encode, -128);
        assert!(zz[1..].iter().all(|&c| c == 0));
        let mut out = [0u8; 64];
        idct8(&zz, &q.decode, &mut out, 8, 128);
        assert!(out.iter().all(|&p| p == 200), "{out:?}");
    }
}
