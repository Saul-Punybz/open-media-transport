//! Constant tables, ported from libvmx `vmxcodec_common.h` and
//! `vmxcodec_x86.cpp` / `vmxcodec_arm.cpp` (MIT, Open Media Transport
//! Contributors).

/// Rows of one slice. Every plane is coded in horizontal slices of 16 lines.
pub(crate) const SLICE_HEIGHT: usize = 16;

/// Number of quality presets.
pub(crate) const QUALITY_COUNT: usize = 25;

/// Quantiser multipliers, indexed by quality preset (`100 - quality`).
pub(crate) const QUALITY: [u16; QUALITY_COUNT] =
    [1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 18, 20, 22, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64];

/// Minimum quality of the OMT profiles.
pub(crate) const OMT_MIN_QUALITY: i32 = 52;
/// Upper bound accepted by `set_quality`.
pub(crate) const MAX_QUALITY: i32 = 98;

/// Base quantisation matrix (natural order).
pub(crate) const DEFAULT_QUANTIZATION_MATRIX: [u16; 64] = [
    16, 16, 19, 22, 26, 27, 29, 34, //
    16, 16, 22, 24, 27, 29, 34, 37, //
    19, 22, 26, 27, 29, 34, 34, 38, //
    22, 22, 26, 27, 29, 34, 37, 40, //
    22, 26, 27, 29, 32, 35, 40, 48, //
    26, 27, 29, 32, 35, 40, 48, 58, //
    26, 27, 29, 34, 38, 46, 56, 69, //
    27, 29, 35, 38, 46, 56, 69, 83,
];

/// Zig-zag scan: `ZIGZAG[i]` is the natural index of the i-th coefficient.
pub(crate) const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, //
    17, 24, 32, 25, 18, 11, 4, 5, //
    12, 19, 26, 33, 40, 48, 41, 34, //
    27, 20, 13, 6, 7, 14, 21, 28, //
    35, 42, 49, 56, 57, 50, 43, 36, //
    29, 22, 15, 23, 30, 37, 44, 51, //
    58, 59, 52, 45, 38, 31, 39, 46, //
    53, 60, 61, 54, 47, 55, 62, 63,
];

/// One row of the libvmx bitrate table:
/// (profile, minimum height, target Mbps, DC shift, minimum quality, threads).
pub(crate) struct BitrateRow {
    pub profile: u8,
    pub min_height: u32,
    pub target_mbps: u32,
    pub dc_shift: u8,
    pub min_quality: i32,
    /// Worker threads libvmx starts for this size (informational).
    #[allow(dead_code)]
    pub threads: u32,
}

const fn row(profile: u8, min_height: u32, target_mbps: u32, dc_shift: u8, min_quality: i32, threads: u32) -> BitrateRow {
    BitrateRow { profile, min_height, target_mbps, dc_shift, min_quality, threads }
}

const HQ: u8 = 99;
const SQ: u8 = 66;
const LQ: u8 = 33;
const OHQ: u8 = 199;
const OSQ: u8 = 166;
const OLQ: u8 = 133;
const OMQ: i32 = OMT_MIN_QUALITY;

/// Highest resolutions first; the first row whose profile matches and whose
/// `min_height` is <= the frame height wins.
pub(crate) const BITRATE_TABLE: [BitrateRow; 36] = [
    row(HQ, 4320, 1320, 0, 80, 8),
    row(OHQ, 4320, 1200, 0, OMQ, 8),
    row(SQ, 4320, 660, 3, 60, 8),
    row(OSQ, 4320, 600, 3, OMQ, 8),
    row(LQ, 4320, 440, 3, 60, 8),
    row(OLQ, 4320, 400, 3, OMQ, 8),
    row(HQ, 2160, 800, 0, 80, 4),
    row(OHQ, 2160, 600, 0, OMQ, 4),
    row(SQ, 2160, 400, 3, 60, 4),
    row(OSQ, 2160, 300, 3, OMQ, 4),
    row(LQ, 2160, 266, 3, 60, 4),
    row(OLQ, 2160, 200, 3, OMQ, 4),
    row(HQ, 1440, 504, 0, 80, 4),
    row(OHQ, 1440, 450, 0, OMQ, 4),
    row(SQ, 1440, 252, 3, 60, 4),
    row(OSQ, 1440, 300, 0, OMQ, 4),
    row(LQ, 1440, 168, 3, 60, 4),
    row(OLQ, 1440, 120, 3, OMQ, 4),
    row(HQ, 1080, 260, 0, 80, 2),
    row(OHQ, 1080, 260, 0, OMQ, 2),
    row(SQ, 1080, 130, 3, 60, 2),
    row(OSQ, 1080, 200, 0, OMQ, 2),
    row(LQ, 1080, 86, 3, 60, 2),
    row(OLQ, 1080, 86, 3, OMQ, 2),
    row(HQ, 720, 136, 0, 80, 2),
    row(OHQ, 720, 136, 0, OMQ, 2),
    row(SQ, 720, 68, 3, 60, 2),
    row(OSQ, 720, 68, 3, OMQ, 2),
    row(LQ, 720, 45, 3, 60, 2),
    row(OLQ, 720, 45, 3, OMQ, 2),
    row(HQ, 0, 72, 0, 80, 2),
    row(OHQ, 0, 72, 0, OMQ, 2),
    row(SQ, 0, 36, 3, 60, 2),
    row(OSQ, 0, 36, 3, OMQ, 2),
    row(LQ, 0, 24, 3, 60, 2),
    row(OLQ, 0, 24, 3, OMQ, 2),
];

// ---------------------------------------------------------------------------
// Forward DCT (fixed point, AP-922 style) constants.
// ---------------------------------------------------------------------------

pub(crate) const FDCT_ROUND1: i16 = 1;
pub(crate) const FDCT_TAN1: u16 = 13036;
pub(crate) const FDCT_TAN2: u16 = 27146;
pub(crate) const FDCT_TAN3: u16 = 43790;
pub(crate) const FDCT_SQRT2: u16 = 23170;

pub(crate) const FTAB1: [u16; 32] = [
    16384, 16384, 22725, 19266, 56669, 44129, 42811, 52663, //
    16384, 16384, 12873, 4520, 21407, 8867, 19266, 61016, //
    16384, 49152, 12873, 42811, 21407, 56669, 19266, 42811, //
    49152, 16384, 4520, 19266, 8867, 44129, 4520, 52663,
];
pub(crate) const FTAB2: [u16; 32] = [
    22725, 22725, 31521, 26722, 53237, 35844, 34015, 47681, //
    22725, 22725, 17855, 6270, 29692, 12299, 26722, 59266, //
    22725, 42811, 17855, 34015, 29692, 53237, 26722, 34015, //
    42811, 22725, 6270, 26722, 12299, 35844, 6270, 47681,
];
pub(crate) const FTAB3: [u16; 32] = [
    21407, 21407, 29692, 25172, 53951, 37567, 35844, 48717, //
    21407, 21407, 16819, 5906, 27969, 11585, 25172, 59630, //
    21407, 44129, 16819, 35844, 27969, 53951, 25172, 35844, //
    44129, 21407, 5906, 25172, 11585, 37567, 5906, 48717,
];
pub(crate) const FTAB4: [u16; 32] = [
    19266, 19266, 26722, 22654, 55110, 40364, 38814, 50399, //
    19266, 19266, 15137, 5315, 25172, 10426, 22654, 60221, //
    19266, 46270, 15137, 38814, 25172, 55110, 22654, 38814, //
    46270, 19266, 5315, 22654, 10426, 40364, 5315, 50399,
];

// ---------------------------------------------------------------------------
// Inverse DCT constants.
// ---------------------------------------------------------------------------

pub(crate) const TG_1_16: i16 = 13036;
pub(crate) const TG_2_16: i16 = 27146;
pub(crate) const TG_3_16: i16 = -21746;
pub(crate) const COS_4_16: i16 = -19195;

pub(crate) const TAB_I_04: [i16; 32] = [
    16384, 21407, 16384, 8867, 16384, -8867, 16384, -21407, //
    16384, 8867, -16384, -21407, -16384, 21407, 16384, -8867, //
    22725, 19266, 19266, -4520, 12873, -22725, 4520, -12873, //
    12873, 4520, -22725, -12873, 4520, 19266, 19266, -22725,
];
pub(crate) const TAB_I_17: [i16; 32] = [
    22725, 29692, 22725, 12299, 22725, -12299, 22725, -29692, //
    22725, 12299, -22725, -29692, -22725, 29692, 22725, -12299, //
    31521, 26722, 26722, -6270, 17855, -31521, 6270, -17855, //
    17855, 6270, -31521, -17855, 6270, 26722, 26722, -31521,
];
pub(crate) const TAB_I_26: [i16; 32] = [
    21407, 27969, 21407, 11585, 21407, -11585, 21407, -27969, //
    21407, 11585, -21407, -27969, -21407, 27969, 21407, -11585, //
    29692, 25172, 25172, -5906, 16819, -29692, 5906, -16819, //
    16819, 5906, -29692, -16819, 5906, 25172, 25172, -29692,
];
pub(crate) const TAB_I_35: [i16; 32] = [
    19266, 25172, 19266, 10426, 19266, -10426, 19266, -25172, //
    19266, 10426, -19266, -25172, -19266, 25172, 19266, -10426, //
    26722, 22654, 22654, -5315, 15137, -26722, 5315, -15137, //
    15137, 5315, -26722, -15137, 5315, 22654, 22654, -26722,
];
