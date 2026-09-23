//! Portable, safe emulation of the handful of 128-bit integer SIMD operations
//! the libvmx transform uses.
//!
//! libvmx defines the VMX bitstream through its SSE/NEON code: rounding,
//! saturation and 16-bit wrap-around all affect the output. Writing the scalar
//! port in terms of these helpers (8 x i16 or 4 x i32 lanes, same semantics as
//! the Intel intrinsics of the same name) keeps it bit-exact with the C
//! reference while staying plain, `unsafe`-free Rust that LLVM can still
//! auto-vectorise.

#![allow(clippy::needless_range_loop)]
// On aarch64 and x86-64 the encoder and decoder use `crate::simd`; this
// portable path is then only the fallback for other targets and the oracle
// the tests compare the hardware paths against.
#![cfg_attr(all(not(test), any(target_arch = "aarch64", target_arch = "x86_64")), allow(dead_code))]

pub(crate) type V16 = [i16; 8];
pub(crate) type V32 = [i32; 4];

#[inline(always)]
fn map2(a: V16, b: V16, f: impl Fn(i16, i16) -> i16) -> V16 {
    let mut r = [0i16; 8];
    for i in 0..8 {
        r[i] = f(a[i], b[i]);
    }
    r
}

#[inline(always)]
fn map1(a: V16, f: impl Fn(i16) -> i16) -> V16 {
    let mut r = [0i16; 8];
    for i in 0..8 {
        r[i] = f(a[i]);
    }
    r
}

/// `_mm_set1_epi16`
#[inline(always)]
pub(crate) fn splat(v: i16) -> V16 {
    [v; 8]
}

/// `_mm_adds_epi16` (signed saturating add)
#[inline(always)]
pub(crate) fn adds(a: V16, b: V16) -> V16 {
    map2(a, b, i16::saturating_add)
}

/// `_mm_subs_epi16` (signed saturating subtract)
#[inline(always)]
pub(crate) fn subs(a: V16, b: V16) -> V16 {
    map2(a, b, i16::saturating_sub)
}

/// `_mm_add_epi16` (wrapping)
#[inline(always)]
pub(crate) fn add16(a: V16, b: V16) -> V16 {
    map2(a, b, i16::wrapping_add)
}

/// `_mm_mulhi_epi16`
#[inline(always)]
pub(crate) fn mulhi(a: V16, b: V16) -> V16 {
    map2(a, b, |x, y| ((x as i32 * y as i32) >> 16) as i16)
}

/// `_mm_mulhi_epu16`
#[inline(always)]
pub(crate) fn mulhi_u(a: V16, b: V16) -> V16 {
    map2(a, b, |x, y| (((x as u16 as u32) * (y as u16 as u32)) >> 16) as u16 as i16)
}

/// `_mm_mullo_epi16`
#[inline(always)]
pub(crate) fn mullo(a: V16, b: V16) -> V16 {
    map2(a, b, i16::wrapping_mul)
}

/// `_mm_srai_epi16`
#[inline(always)]
pub(crate) fn srai16(a: V16, n: u32) -> V16 {
    map1(a, |x| x >> n)
}

/// `_mm_slli_epi16`
#[inline(always)]
pub(crate) fn slli16(a: V16, n: u32) -> V16 {
    map1(a, |x| ((x as u16) << n) as i16)
}

/// `_mm_or_si128` with a 16-bit lane view
#[inline(always)]
pub(crate) fn or16(a: V16, b: V16) -> V16 {
    map2(a, b, |x, y| x | y)
}

/// `_mm_abs_epi16`
#[inline(always)]
pub(crate) fn abs16(a: V16) -> V16 {
    map1(a, i16::wrapping_abs)
}

/// `_mm_sign_epi16`
#[inline(always)]
pub(crate) fn sign16(a: V16, b: V16) -> V16 {
    map2(a, b, |x, s| {
        if s < 0 {
            x.wrapping_neg()
        } else if s == 0 {
            0
        } else {
            x
        }
    })
}

/// `_mm_min_epi16`
#[inline(always)]
pub(crate) fn min16(a: V16, b: V16) -> V16 {
    map2(a, b, |x, y| x.min(y))
}

/// `_mm_max_epi16`
#[inline(always)]
pub(crate) fn max16(a: V16, b: V16) -> V16 {
    map2(a, b, |x, y| x.max(y))
}

/// `_mm_madd_epi16`
#[inline(always)]
pub(crate) fn madd(a: V16, b: V16) -> V32 {
    let mut r = [0i32; 4];
    for i in 0..4 {
        r[i] = (a[2 * i] as i32 * b[2 * i] as i32).wrapping_add(a[2 * i + 1] as i32 * b[2 * i + 1] as i32);
    }
    r
}

/// `_mm_add_epi32`
#[inline(always)]
pub(crate) fn add32(a: V32, b: V32) -> V32 {
    [a[0].wrapping_add(b[0]), a[1].wrapping_add(b[1]), a[2].wrapping_add(b[2]), a[3].wrapping_add(b[3])]
}

/// `_mm_sub_epi32`
#[inline(always)]
pub(crate) fn sub32(a: V32, b: V32) -> V32 {
    [a[0].wrapping_sub(b[0]), a[1].wrapping_sub(b[1]), a[2].wrapping_sub(b[2]), a[3].wrapping_sub(b[3])]
}

/// `_mm_srai_epi32`
#[inline(always)]
pub(crate) fn srai32(a: V32, n: u32) -> V32 {
    [a[0] >> n, a[1] >> n, a[2] >> n, a[3] >> n]
}

/// `_mm_packs_epi32`
#[inline(always)]
pub(crate) fn packs32(a: V32, b: V32) -> V16 {
    let s = |x: i32| x.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    [s(a[0]), s(a[1]), s(a[2]), s(a[3]), s(b[0]), s(b[1]), s(b[2]), s(b[3])]
}

/// `_mm_shuffle_epi32` applied to a 32-bit lane vector
#[inline(always)]
pub(crate) fn shuffle32(a: V32, imm: u8) -> V32 {
    let i = |k: u8| a[((imm >> (2 * k)) & 3) as usize];
    [i(0), i(1), i(2), i(3)]
}

/// `_mm_shuffle_epi32` applied to a vector viewed as 16-bit lanes
#[inline(always)]
pub(crate) fn shuffle32_16(a: V16, imm: u8) -> V16 {
    let mut r = [0i16; 8];
    for k in 0..4 {
        let src = ((imm >> (2 * k)) & 3) as usize;
        r[2 * k] = a[2 * src];
        r[2 * k + 1] = a[2 * src + 1];
    }
    r
}

/// `_mm_shufflelo_epi16`
#[inline(always)]
pub(crate) fn shufflelo(a: V16, imm: u8) -> V16 {
    let mut r = a;
    for k in 0..4 {
        r[k] = a[((imm >> (2 * k)) & 3) as usize];
    }
    r
}

/// `_mm_shufflehi_epi16`
#[inline(always)]
pub(crate) fn shufflehi(a: V16, imm: u8) -> V16 {
    let mut r = a;
    for k in 0..4 {
        r[4 + k] = a[4 + ((imm >> (2 * k)) & 3) as usize];
    }
    r
}

/// `_mm_unpacklo_epi32` on 16-bit lane vectors
#[inline(always)]
pub(crate) fn unpacklo32(a: V16, b: V16) -> V16 {
    [a[0], a[1], b[0], b[1], a[2], a[3], b[2], b[3]]
}

/// `_mm_packus_epi16` for one vector, returning 8 bytes.
#[inline(always)]
pub(crate) fn packus8(a: V16) -> [u8; 8] {
    let mut r = [0u8; 8];
    for i in 0..8 {
        r[i] = a[i].clamp(0, 255) as u8;
    }
    r
}

/// Reinterprets a table of unsigned 16-bit constants as signed lanes.
#[inline(always)]
pub(crate) fn ld(t: &[u16]) -> V16 {
    let mut r = [0i16; 8];
    for i in 0..8 {
        r[i] = t[i] as i16;
    }
    r
}

/// The 128-bit integer operations the transforms are written in, as an
/// instruction-set abstraction. [`Scalar`] implements them with the portable
/// lane emulation above; `crate::simd` implements them with NEON (aarch64)
/// and SSE2 (x86-64). Every implementation must give bit-identical results
/// for every input; the transforms are written once, generic over this trait,
/// so both paths run the same sequence of operations.
pub(crate) trait Isa: Copy {
    /// Eight 16-bit lanes.
    type V16: Copy;
    /// Four 32-bit lanes.
    type V32: Copy;
    fn splat(v: i16) -> Self::V16;
    fn splat32(v: i32) -> Self::V32;
    fn load(a: &[i16; 8]) -> Self::V16;
    /// Loads unsigned table constants reinterpreted as signed lanes.
    fn load_u(a: &[u16; 8]) -> Self::V16;
    fn store(v: Self::V16) -> [i16; 8];
    fn adds(a: Self::V16, b: Self::V16) -> Self::V16;
    fn subs(a: Self::V16, b: Self::V16) -> Self::V16;
    fn add16(a: Self::V16, b: Self::V16) -> Self::V16;
    fn mulhi(a: Self::V16, b: Self::V16) -> Self::V16;
    fn mulhi_u(a: Self::V16, b: Self::V16) -> Self::V16;
    fn mullo(a: Self::V16, b: Self::V16) -> Self::V16;
    fn srai16(a: Self::V16, n: u32) -> Self::V16;
    fn slli16(a: Self::V16, n: u32) -> Self::V16;
    fn or16(a: Self::V16, b: Self::V16) -> Self::V16;
    fn abs16(a: Self::V16) -> Self::V16;
    fn sign16(a: Self::V16, b: Self::V16) -> Self::V16;
    fn min16(a: Self::V16, b: Self::V16) -> Self::V16;
    fn max16(a: Self::V16, b: Self::V16) -> Self::V16;
    fn madd(a: Self::V16, b: Self::V16) -> Self::V32;
    fn add32(a: Self::V32, b: Self::V32) -> Self::V32;
    fn sub32(a: Self::V32, b: Self::V32) -> Self::V32;
    fn srai32(a: Self::V32, n: u32) -> Self::V32;
    fn packs32(a: Self::V32, b: Self::V32) -> Self::V16;
    fn shuffle32<const IMM: i32>(a: Self::V32) -> Self::V32;
    fn shuffle32_16<const IMM: i32>(a: Self::V16) -> Self::V16;
    fn shufflelo<const IMM: i32>(a: Self::V16) -> Self::V16;
    fn shufflehi<const IMM: i32>(a: Self::V16) -> Self::V16;
    fn unpacklo32(a: Self::V16, b: Self::V16) -> Self::V16;
    fn packus8(a: Self::V16) -> [u8; 8];

    /// Bit `i` is set when `zz[i]` is non-zero (libvmx's
    /// `packs` + `cmpeq` + `movemask` of a coded block).
    #[inline(always)]
    fn nonzero_mask(zz: &[i16; 64]) -> u64 {
        let mut m = 0u64;
        for (i, &c) in zz.iter().enumerate() {
            m |= ((c != 0) as u64) << i;
        }
        m
    }
}

/// Portable implementation of [`Isa`]: the lane emulation of this module.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scalar;

impl Isa for Scalar {
    type V16 = [i16; 8];
    type V32 = [i32; 4];
    #[inline(always)]
    fn splat(v: i16) -> V16 {
        splat(v)
    }
    #[inline(always)]
    fn splat32(v: i32) -> V32 {
        [v; 4]
    }
    #[inline(always)]
    fn load(a: &[i16; 8]) -> V16 {
        *a
    }
    #[inline(always)]
    fn load_u(a: &[u16; 8]) -> V16 {
        ld(a)
    }
    #[inline(always)]
    fn store(v: V16) -> [i16; 8] {
        v
    }
    #[inline(always)]
    fn adds(a: V16, b: V16) -> V16 {
        adds(a, b)
    }
    #[inline(always)]
    fn subs(a: V16, b: V16) -> V16 {
        subs(a, b)
    }
    #[inline(always)]
    fn add16(a: V16, b: V16) -> V16 {
        add16(a, b)
    }
    #[inline(always)]
    fn mulhi(a: V16, b: V16) -> V16 {
        mulhi(a, b)
    }
    #[inline(always)]
    fn mulhi_u(a: V16, b: V16) -> V16 {
        mulhi_u(a, b)
    }
    #[inline(always)]
    fn mullo(a: V16, b: V16) -> V16 {
        mullo(a, b)
    }
    #[inline(always)]
    fn srai16(a: V16, n: u32) -> V16 {
        srai16(a, n)
    }
    #[inline(always)]
    fn slli16(a: V16, n: u32) -> V16 {
        slli16(a, n)
    }
    #[inline(always)]
    fn or16(a: V16, b: V16) -> V16 {
        or16(a, b)
    }
    #[inline(always)]
    fn abs16(a: V16) -> V16 {
        abs16(a)
    }
    #[inline(always)]
    fn sign16(a: V16, b: V16) -> V16 {
        sign16(a, b)
    }
    #[inline(always)]
    fn min16(a: V16, b: V16) -> V16 {
        min16(a, b)
    }
    #[inline(always)]
    fn max16(a: V16, b: V16) -> V16 {
        max16(a, b)
    }
    #[inline(always)]
    fn madd(a: V16, b: V16) -> V32 {
        madd(a, b)
    }
    #[inline(always)]
    fn add32(a: V32, b: V32) -> V32 {
        add32(a, b)
    }
    #[inline(always)]
    fn sub32(a: V32, b: V32) -> V32 {
        sub32(a, b)
    }
    #[inline(always)]
    fn srai32(a: V32, n: u32) -> V32 {
        srai32(a, n)
    }
    #[inline(always)]
    fn packs32(a: V32, b: V32) -> V16 {
        packs32(a, b)
    }
    #[inline(always)]
    fn shuffle32<const IMM: i32>(a: V32) -> V32 {
        shuffle32(a, IMM as u8)
    }
    #[inline(always)]
    fn shuffle32_16<const IMM: i32>(a: V16) -> V16 {
        shuffle32_16(a, IMM as u8)
    }
    #[inline(always)]
    fn shufflelo<const IMM: i32>(a: V16) -> V16 {
        shufflelo(a, IMM as u8)
    }
    #[inline(always)]
    fn shufflehi<const IMM: i32>(a: V16) -> V16 {
        shufflehi(a, IMM as u8)
    }
    #[inline(always)]
    fn unpacklo32(a: V16, b: V16) -> V16 {
        unpacklo32(a, b)
    }
    #[inline(always)]
    fn packus8(a: V16) -> [u8; 8] {
        packus8(a)
    }
}
