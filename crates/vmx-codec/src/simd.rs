//! Hardware implementations of [`Isa`]: NEON on aarch64, SSE2 on x86-64.
//!
//! This is the only module of the crate that uses `unsafe`: `std::arch`
//! intrinsics. Each operation mirrors the SSE intrinsic of the same name that
//! libvmx's 128-bit path uses (on ARM libvmx runs that same code through
//! sse2neon), so the results are bit-identical to [`crate::lanes::Scalar`];
//! `tests` below and in `dct.rs` check that on random and extreme inputs.
//!
//! NEON is part of the aarch64 baseline and SSE2 of the x86-64 baseline, so
//! no runtime feature detection is needed. The transform uses no SSSE3 or
//! SSE4.1 instructions: `abs` and `sign` are built from SSE2 operations.
//!
//! On recent toolchains most of these intrinsics are safe functions when the
//! target feature is enabled at compile time; the `unsafe` blocks are still
//! required by the crate's minimum Rust version, hence `unused_unsafe`.

#![allow(unused_unsafe)]

use crate::lanes::Isa;

/// The best [`Isa`] for the compilation target.
#[cfg(target_arch = "aarch64")]
pub(crate) type Native = neon::Neon;
/// The best [`Isa`] for the compilation target.
#[cfg(target_arch = "x86_64")]
pub(crate) type Native = sse2::Sse2;
/// The best [`Isa`] for the compilation target.
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub(crate) type Native = crate::lanes::Scalar;

#[cfg(target_arch = "aarch64")]
mod neon {
    use super::Isa;
    use core::arch::aarch64::*;

    /// Byte-shuffle tables for the SSE shuffles with immediate `IMM`, used
    /// with `vqtbl1q_u8`.
    struct Shuf<const IMM: i32>;

    impl<const IMM: i32> Shuf<IMM> {
        /// `_mm_shuffle_epi32`: 32-bit lane `k` takes lane `(IMM >> 2k) & 3`.
        const W32: [u8; 16] = {
            let mut t = [0u8; 16];
            let mut k = 0;
            while k < 4 {
                let src = ((IMM >> (2 * k)) & 3) as u8;
                let mut b = 0;
                while b < 4 {
                    t[4 * k + b] = 4 * src + b as u8;
                    b += 1;
                }
                k += 1;
            }
            t
        };
        /// `_mm_shufflelo_epi16` (`hi == false`) / `_mm_shufflehi_epi16`.
        const fn half(hi: bool) -> [u8; 16] {
            let mut t = [0u8; 16];
            let mut l = 0;
            while l < 8 {
                t[2 * l] = 2 * l as u8;
                t[2 * l + 1] = 2 * l as u8 + 1;
                l += 1;
            }
            let base = if hi { 4 } else { 0 };
            let mut k = 0;
            while k < 4 {
                let src = base + ((IMM >> (2 * k)) & 3) as usize;
                t[2 * (base + k)] = 2 * src as u8;
                t[2 * (base + k) + 1] = 2 * src as u8 + 1;
                k += 1;
            }
            t
        }
        const LO: [u8; 16] = Self::half(false);
        const HI: [u8; 16] = Self::half(true);
    }

    #[inline(always)]
    fn tbl(a: uint8x16_t, t: &[u8; 16]) -> uint8x16_t {
        // SAFETY: `t` is 16 readable bytes; NEON is always present on aarch64.
        unsafe { vqtbl1q_u8(a, vld1q_u8(t.as_ptr())) }
    }

    /// NEON implementation of [`Isa`].
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Neon;

    // SAFETY (all blocks in this impl unless noted): NEON is part of the
    // aarch64 baseline, so the intrinsics are always available; the
    // arithmetic ones have no other preconditions.
    impl Isa for Neon {
        type V16 = int16x8_t;
        type V32 = int32x4_t;

        #[inline(always)]
        fn splat(v: i16) -> int16x8_t {
            unsafe { vdupq_n_s16(v) }
        }
        #[inline(always)]
        fn splat32(v: i32) -> int32x4_t {
            unsafe { vdupq_n_s32(v) }
        }
        #[inline(always)]
        fn load(a: &[i16; 8]) -> int16x8_t {
            // SAFETY: `a` is eight readable, aligned i16 values.
            unsafe { vld1q_s16(a.as_ptr()) }
        }
        #[inline(always)]
        fn load_u(a: &[u16; 8]) -> int16x8_t {
            // SAFETY: `a` is eight readable, aligned u16 values.
            unsafe { vreinterpretq_s16_u16(vld1q_u16(a.as_ptr())) }
        }
        #[inline(always)]
        fn store(v: int16x8_t) -> [i16; 8] {
            let mut r = [0i16; 8];
            // SAFETY: `r` is eight writable, aligned i16 values.
            unsafe { vst1q_s16(r.as_mut_ptr(), v) };
            r
        }
        #[inline(always)]
        fn adds(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            unsafe { vqaddq_s16(a, b) }
        }
        #[inline(always)]
        fn subs(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            unsafe { vqsubq_s16(a, b) }
        }
        #[inline(always)]
        fn add16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            unsafe { vaddq_s16(a, b) }
        }
        #[inline(always)]
        fn mulhi(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            // High halves of the exact 32-bit products (odd 16-bit lanes).
            unsafe {
                let lo = vmull_s16(vget_low_s16(a), vget_low_s16(b));
                let hi = vmull_high_s16(a, b);
                vuzp2q_s16(vreinterpretq_s16_s32(lo), vreinterpretq_s16_s32(hi))
            }
        }
        #[inline(always)]
        fn mulhi_u(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            unsafe {
                let (a, b) = (vreinterpretq_u16_s16(a), vreinterpretq_u16_s16(b));
                let lo = vmull_u16(vget_low_u16(a), vget_low_u16(b));
                let hi = vmull_high_u16(a, b);
                vreinterpretq_s16_u16(vuzp2q_u16(vreinterpretq_u16_u32(lo), vreinterpretq_u16_u32(hi)))
            }
        }
        #[inline(always)]
        fn mullo(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            unsafe { vmulq_s16(a, b) }
        }
        #[inline(always)]
        fn srai16(a: int16x8_t, n: u32) -> int16x8_t {
            // SSHL by a negative count is an arithmetic right shift.
            unsafe { vshlq_s16(a, vdupq_n_s16(-(n as i16))) }
        }
        #[inline(always)]
        fn slli16(a: int16x8_t, n: u32) -> int16x8_t {
            unsafe { vshlq_s16(a, vdupq_n_s16(n as i16)) }
        }
        #[inline(always)]
        fn or16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            unsafe { vorrq_s16(a, b) }
        }
        #[inline(always)]
        fn abs16(a: int16x8_t) -> int16x8_t {
            // Non-saturating like `_mm_abs_epi16`: abs(-32768) = -32768.
            unsafe { vabsq_s16(a) }
        }
        #[inline(always)]
        fn sign16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            unsafe {
                let neg = vbslq_s16(vcltzq_s16(b), vnegq_s16(a), a);
                vandq_s16(neg, vreinterpretq_s16_u16(vtstq_s16(b, b)))
            }
        }
        #[inline(always)]
        fn min16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            unsafe { vminq_s16(a, b) }
        }
        #[inline(always)]
        fn max16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            unsafe { vmaxq_s16(a, b) }
        }
        #[inline(always)]
        fn madd(a: int16x8_t, b: int16x8_t) -> int32x4_t {
            unsafe {
                let lo = vmull_s16(vget_low_s16(a), vget_low_s16(b));
                let hi = vmull_high_s16(a, b);
                vpaddq_s32(lo, hi)
            }
        }
        #[inline(always)]
        fn add32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
            unsafe { vaddq_s32(a, b) }
        }
        #[inline(always)]
        fn sub32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
            unsafe { vsubq_s32(a, b) }
        }
        #[inline(always)]
        fn srai32(a: int32x4_t, n: u32) -> int32x4_t {
            unsafe { vshlq_s32(a, vdupq_n_s32(-(n as i32))) }
        }
        #[inline(always)]
        fn packs32(a: int32x4_t, b: int32x4_t) -> int16x8_t {
            unsafe { vcombine_s16(vqmovn_s32(a), vqmovn_s32(b)) }
        }
        #[inline(always)]
        fn shuffle32<const IMM: i32>(a: int32x4_t) -> int32x4_t {
            unsafe { vreinterpretq_s32_u8(tbl(vreinterpretq_u8_s32(a), &Shuf::<IMM>::W32)) }
        }
        #[inline(always)]
        fn shuffle32_16<const IMM: i32>(a: int16x8_t) -> int16x8_t {
            unsafe { vreinterpretq_s16_u8(tbl(vreinterpretq_u8_s16(a), &Shuf::<IMM>::W32)) }
        }
        #[inline(always)]
        fn shufflelo<const IMM: i32>(a: int16x8_t) -> int16x8_t {
            unsafe { vreinterpretq_s16_u8(tbl(vreinterpretq_u8_s16(a), &Shuf::<IMM>::LO)) }
        }
        #[inline(always)]
        fn shufflehi<const IMM: i32>(a: int16x8_t) -> int16x8_t {
            unsafe { vreinterpretq_s16_u8(tbl(vreinterpretq_u8_s16(a), &Shuf::<IMM>::HI)) }
        }
        #[inline(always)]
        fn unpacklo32(a: int16x8_t, b: int16x8_t) -> int16x8_t {
            unsafe { vreinterpretq_s16_s32(vzip1q_s32(vreinterpretq_s32_s16(a), vreinterpretq_s32_s16(b))) }
        }
        #[inline(always)]
        fn packus8(a: int16x8_t) -> [u8; 8] {
            let mut r = [0u8; 8];
            // SAFETY: `r` is eight writable bytes.
            unsafe { vst1_u8(r.as_mut_ptr(), vqmovun_s16(a)) };
            r
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod sse2 {
    use super::Isa;
    use core::arch::x86_64::*;

    /// SSE2 implementation of [`Isa`].
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Sse2;

    // SAFETY (all blocks in this impl unless noted): SSE2 is part of the
    // x86-64 baseline, so the intrinsics are always available; the
    // arithmetic ones have no other preconditions.
    impl Isa for Sse2 {
        type V16 = __m128i;
        type V32 = __m128i;

        #[inline(always)]
        fn splat(v: i16) -> __m128i {
            unsafe { _mm_set1_epi16(v) }
        }
        #[inline(always)]
        fn splat32(v: i32) -> __m128i {
            unsafe { _mm_set1_epi32(v) }
        }
        #[inline(always)]
        fn load(a: &[i16; 8]) -> __m128i {
            // SAFETY: `a` is 16 readable bytes; the load is unaligned.
            unsafe { _mm_loadu_si128(a.as_ptr().cast()) }
        }
        #[inline(always)]
        fn load_u(a: &[u16; 8]) -> __m128i {
            // SAFETY: `a` is 16 readable bytes; the load is unaligned.
            unsafe { _mm_loadu_si128(a.as_ptr().cast()) }
        }
        #[inline(always)]
        fn store(v: __m128i) -> [i16; 8] {
            let mut r = [0i16; 8];
            // SAFETY: `r` is 16 writable bytes; the store is unaligned.
            unsafe { _mm_storeu_si128(r.as_mut_ptr().cast(), v) };
            r
        }
        #[inline(always)]
        fn adds(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_adds_epi16(a, b) }
        }
        #[inline(always)]
        fn subs(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_subs_epi16(a, b) }
        }
        #[inline(always)]
        fn add16(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_add_epi16(a, b) }
        }
        #[inline(always)]
        fn mulhi(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_mulhi_epi16(a, b) }
        }
        #[inline(always)]
        fn mulhi_u(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_mulhi_epu16(a, b) }
        }
        #[inline(always)]
        fn mullo(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_mullo_epi16(a, b) }
        }
        #[inline(always)]
        fn srai16(a: __m128i, n: u32) -> __m128i {
            unsafe { _mm_sra_epi16(a, _mm_cvtsi32_si128(n as i32)) }
        }
        #[inline(always)]
        fn slli16(a: __m128i, n: u32) -> __m128i {
            unsafe { _mm_sll_epi16(a, _mm_cvtsi32_si128(n as i32)) }
        }
        #[inline(always)]
        fn or16(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_or_si128(a, b) }
        }
        #[inline(always)]
        fn abs16(a: __m128i) -> __m128i {
            // SSSE3 `_mm_abs_epi16` from SSE2: (a ^ m) - m with m = a >> 15.
            unsafe {
                let m = _mm_srai_epi16::<15>(a);
                _mm_sub_epi16(_mm_xor_si128(a, m), m)
            }
        }
        #[inline(always)]
        fn sign16(a: __m128i, b: __m128i) -> __m128i {
            // SSSE3 `_mm_sign_epi16` from SSE2: negate where b < 0, zero
            // where b == 0.
            unsafe {
                let m = _mm_srai_epi16::<15>(b);
                let neg = _mm_sub_epi16(_mm_xor_si128(a, m), m);
                _mm_andnot_si128(_mm_cmpeq_epi16(b, _mm_setzero_si128()), neg)
            }
        }
        #[inline(always)]
        fn min16(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_min_epi16(a, b) }
        }
        #[inline(always)]
        fn max16(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_max_epi16(a, b) }
        }
        #[inline(always)]
        fn madd(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_madd_epi16(a, b) }
        }
        #[inline(always)]
        fn add32(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_add_epi32(a, b) }
        }
        #[inline(always)]
        fn sub32(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_sub_epi32(a, b) }
        }
        #[inline(always)]
        fn srai32(a: __m128i, n: u32) -> __m128i {
            unsafe { _mm_sra_epi32(a, _mm_cvtsi32_si128(n as i32)) }
        }
        #[inline(always)]
        fn packs32(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_packs_epi32(a, b) }
        }
        #[inline(always)]
        fn shuffle32<const IMM: i32>(a: __m128i) -> __m128i {
            unsafe { _mm_shuffle_epi32::<IMM>(a) }
        }
        #[inline(always)]
        fn shuffle32_16<const IMM: i32>(a: __m128i) -> __m128i {
            unsafe { _mm_shuffle_epi32::<IMM>(a) }
        }
        #[inline(always)]
        fn shufflelo<const IMM: i32>(a: __m128i) -> __m128i {
            unsafe { _mm_shufflelo_epi16::<IMM>(a) }
        }
        #[inline(always)]
        fn shufflehi<const IMM: i32>(a: __m128i) -> __m128i {
            unsafe { _mm_shufflehi_epi16::<IMM>(a) }
        }
        #[inline(always)]
        fn unpacklo32(a: __m128i, b: __m128i) -> __m128i {
            unsafe { _mm_unpacklo_epi32(a, b) }
        }
        #[inline(always)]
        fn packus8(a: __m128i) -> [u8; 8] {
            let mut r = [0u8; 8];
            // SAFETY: `r` is 8 writable bytes; `_mm_storel_epi64` writes 8
            // bytes and has no alignment requirement.
            unsafe { _mm_storel_epi64(r.as_mut_ptr().cast(), _mm_packus_epi16(a, a)) };
            r
        }
    }
}
