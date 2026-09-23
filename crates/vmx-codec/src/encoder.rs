//! VMX encoder.

use crate::bits::BitWriter;
use crate::convert::{frame_to_planes16, frame_to_planes8, Planes, FILL16, FILL8};
use crate::dct::QuantTables;
use crate::frame::{Frame, PixelFormat};
use crate::layout::{quality_preset, write, Layout};
use crate::simd::Native;
use crate::slice::{encode_plane, level_shift, Sample};
use crate::tables::{BITRATE_TABLE, MAX_QUALITY, QUALITY_COUNT, SLICE_HEIGHT};
use crate::Error;

/// Encoding profile. Selects the bitrate target, the minimum quality the
/// rate control may fall to, and the DC precision (see [`EncodingParameters`]).
///
/// The `Omt*` profiles are the ones Open Media Transport senders use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Low quality (vMix instant replay).
    Lq,
    /// Standard quality (vMix instant replay).
    Sq,
    /// High quality (vMix instant replay). libvmx's default.
    #[default]
    Hq,
    /// Open Media Transport, low quality.
    OmtLq,
    /// Open Media Transport, standard quality.
    OmtSq,
    /// Open Media Transport, high quality.
    OmtHq,
}

impl Profile {
    /// The `VMX_PROFILE` value used by libvmx.
    pub fn id(self) -> u8 {
        match self {
            Profile::Lq => 33,
            Profile::Sq => 66,
            Profile::Hq => 99,
            Profile::OmtLq => 133,
            Profile::OmtSq => 166,
            Profile::OmtHq => 199,
        }
    }
}

/// Encoder settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderConfig {
    /// Width in pixels: even, 16..=7680.
    pub width: usize,
    /// Height in pixels: 16..=4320.
    pub height: usize,
    /// Rate-control profile.
    pub profile: Profile,
    /// Worker threads for slice-parallel encoding (1 = encode on the calling
    /// thread). The output does not depend on this value.
    pub threads: usize,
}

impl EncoderConfig {
    /// Single-threaded [`Profile::Hq`] configuration.
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, profile: Profile::Hq, threads: 1 }
    }
}

/// Rate-control and precision parameters (`VMX_Get/SetEncodingParameters`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncodingParameters {
    /// A frame smaller than this many bytes raises the quality of the next frame.
    pub frame_min: i32,
    /// A frame larger than this many bytes lowers the quality of the next frame.
    pub frame_max: i32,
    /// Floor for the quality, whatever the bitrate.
    pub min_quality: i32,
    /// DC precision: 0 = 11 bit, 1 = 10 bit, 2 = 9 bit, 3 = 8 bit (0..=15 accepted).
    pub dc_shift: u32,
}

/// Port of `VMX_CalculateBitrate`, float rounding included.
fn bytes_per_frame(target_mbps: u32, min: bool) -> i32 {
    let mut t = target_mbps as f32;
    t /= (60 * 8) as f32;
    t *= 1_048_576.0;
    t = if min { (t as f64 * 0.95) as f32 } else { (t as f64 * 1.05) as f32 };
    t as i32
}

/// Compresses frames into VMX.
///
/// ```
/// use vmx_codec::{Decoder, Encoder, EncoderConfig, Frame, PixelFormat};
///
/// let mut enc = Encoder::new(EncoderConfig::new(64, 32)).unwrap();
/// let mut frame = Frame::new(64, 32, PixelFormat::Uyvy);
/// frame.planes[0].data.fill(128);
/// let packet = enc.encode(&frame).unwrap();
///
/// let mut dec = Decoder::new(64, 32).unwrap();
/// let out = dec.decode(&packet, PixelFormat::Uyvy).unwrap();
/// assert_eq!(out.planes[0].data, frame.planes[0].data);
/// ```
pub struct Encoder {
    layout: Layout,
    params: EncodingParameters,
    quality: i32,
    preset: usize,
    presets: Vec<QuantTables>,
    threads: usize,
    planes8: Option<Planes<u8>>,
    planes16: Option<Planes<u16>>,
}

impl Encoder {
    /// Creates an encoder (port of `VMX_Create`).
    pub fn new(config: EncoderConfig) -> Result<Self, Error> {
        let layout = Layout::new(config.width, config.height)?;
        let mut params = EncodingParameters { frame_min: 0, frame_max: 0, min_quality: 80, dc_shift: 0 };
        if let Some(row) =
            BITRATE_TABLE.iter().find(|r| r.profile == config.profile.id() && config.height as u32 >= r.min_height)
        {
            params = EncodingParameters {
                frame_min: bytes_per_frame(row.target_mbps, true),
                frame_max: bytes_per_frame(row.target_mbps, false),
                min_quality: row.min_quality,
                dc_shift: row.dc_shift as u32,
            };
        }
        let mut e = Self {
            layout,
            params,
            quality: 0,
            preset: 0,
            presets: (0..QUALITY_COUNT).map(QuantTables::new).collect(),
            threads: config.threads.max(1),
            planes8: None,
            planes16: None,
        };
        e.set_quality(80);
        Ok(e)
    }

    /// Sets the quality (0..=100, clamped to `min_quality..=98`) for the next
    /// frame, overriding the rate control for that frame (`VMX_SetQuality`).
    pub fn set_quality(&mut self, q: i32) {
        let q = q.min(MAX_QUALITY).max(self.params.min_quality);
        let (q, idx) = quality_preset(q);
        self.quality = q;
        self.preset = idx;
    }

    /// Quality that will be used for the next frame (`VMX_GetQuality`).
    pub fn quality(&self) -> i32 {
        self.quality
    }

    /// Current rate-control parameters.
    pub fn encoding_parameters(&self) -> EncodingParameters {
        self.params
    }

    /// Replaces the rate-control parameters from the next frame on.
    pub fn set_encoding_parameters(&mut self, p: EncodingParameters) {
        self.params = EncodingParameters { dc_shift: p.dc_shift.min(15), ..p };
    }

    /// Changes the number of worker threads (output is unaffected).
    pub fn set_threads(&mut self, threads: usize) {
        self.threads = threads.max(1);
    }

    /// Encodes one frame and returns the compressed VMX frame.
    ///
    /// After each frame the quality is nudged towards the profile's bitrate
    /// window, exactly as libvmx does in `VMX_SaveTo`.
    pub fn encode(&mut self, frame: &Frame) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        self.encode_into(frame, &mut out)?;
        Ok(out)
    }

    /// Like [`Encoder::encode`], appending to `out`.
    pub fn encode_into(&mut self, frame: &Frame, out: &mut Vec<u8>) -> Result<(), Error> {
        let l = self.layout;
        if frame.width != l.width || frame.height != l.height {
            return Err(Error::FrameSizeMismatch);
        }
        frame.validate()?;
        let is420 = matches!(frame.format, PixelFormat::Nv12 | PixelFormat::I420);
        if is420 && frame.height % 2 != 0 {
            return Err(Error::InvalidFrame("4:2:0 input needs an even height"));
        }
        let interlaced = frame.interlaced && l.interlace_capable();
        if is420 && interlaced {
            return Err(Error::Unsupported("interlaced 4:2:0 input"));
        }
        let nplanes = if frame.format.has_alpha() { 4 } else { 3 };
        let matrix = &self.presets[self.preset].encode;
        let dc_shift = self.params.dc_shift;

        let (dc, ac) = if frame.format.is_10bit() {
            let pl = self.planes16.get_or_insert_with(|| Planes::new(&l, FILL16));
            frame_to_planes16(frame, &l, interlaced, pl);
            encode_slices(pl, &l, nplanes, matrix, dc_shift, self.threads)
        } else {
            let pl = self.planes8.get_or_insert_with(|| Planes::new(&l, FILL8));
            frame_to_planes8(frame, &l, interlaced, pl);
            encode_slices(pl, &l, nplanes, matrix, dc_shift, self.threads)
        };

        let start = out.len();
        write(out, &l, interlaced, self.quality, dc_shift, &dc, &ac);
        self.adjust_bitrate((out.len() - start) as i32);
        Ok(())
    }

    /// Port of `VMX_AdjustBitrate`.
    fn adjust_bitrate(&mut self, len: i32) {
        let (min, max) = (self.params.frame_min, self.params.frame_max);
        if len == 0 || min == 0 || max == 0 {
            return;
        }
        let q = self.quality;
        let min_q = self.params.min_quality;
        if len < min {
            if q < min_q {
                self.set_quality(min_q);
            } else if q < 76 {
                self.set_quality(q + 4);
            } else if q < 92 {
                self.set_quality(q + 2);
            } else if q < 99 {
                self.set_quality(q + 1);
            }
        } else if len > max {
            if q > 92 {
                self.set_quality(q - 1);
            } else if q > min_q {
                self.set_quality(q - 2);
            } else {
                self.set_quality(min_q);
            }
        }
    }
}

type Streams = (Vec<Vec<u8>>, Vec<Vec<u8>>);

fn encode_one_slice<T: Sample>(
    pl: &Planes<T>,
    s: usize,
    nplanes: usize,
    matrix: &[u16; 192],
    dc_shift: u32,
) -> (Vec<u8>, Vec<u8>) {
    let mut dc = BitWriter::with_capacity(256);
    let mut ac = BitWriter::with_capacity(pl.strides[0] * 8);
    for p in 0..nplanes {
        let stride = pl.strides[p];
        let rows = &pl.data[p][s * SLICE_HEIGHT * stride..(s + 1) * SLICE_HEIGHT * stride];
        encode_plane::<T, Native>(rows, stride, level_shift(p, T::DEPTH), matrix, dc_shift, &mut dc, &mut ac);
    }
    (dc.into_bytes(), ac.into_bytes())
}

fn encode_slices<T: Sample>(
    pl: &Planes<T>,
    l: &Layout,
    nplanes: usize,
    matrix: &[u16; 192],
    dc_shift: u32,
    threads: usize,
) -> Streams {
    let n = l.slices;
    let mut results: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(n);
    if threads <= 1 || n < 2 {
        for s in 0..n {
            results.push(encode_one_slice(pl, s, nplanes, matrix, dc_shift));
        }
    } else {
        let per = n.div_ceil(threads.min(n));
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..n)
                .step_by(per)
                .map(|start| {
                    scope.spawn(move || {
                        (start..(start + per).min(n))
                            .map(|s| encode_one_slice(pl, s, nplanes, matrix, dc_shift))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            for h in handles {
                results.extend(h.join().expect("encoder worker panicked"));
            }
        });
    }
    results.into_iter().unzip()
}
