//! Test-only FFI bindings to the upstream libvmx C++ codec (MIT, Open Media
//! Transport Contributors), used by `vmx-codec`'s conformance tests and
//! benchmark. Not published; `vmx-codec` itself never links C.
//!
//! The upstream sources are expected in `reference/libvmx` (gitignored). When
//! they are missing [`AVAILABLE`] is `false` and nothing is linked.

#![allow(clippy::missing_safety_doc)]

/// Whether the C++ reference was compiled into this build.
pub const AVAILABLE: bool = !cfg!(libvmx_missing);

/// Profiles, identical to `VMX_PROFILE` in `vmxcodec.h`.
pub mod profile {
    pub const DEFAULT: i32 = 0;
    pub const LQ: i32 = 33;
    pub const SQ: i32 = 66;
    pub const HQ: i32 = 99;
    pub const OMT_LQ: i32 = 133;
    pub const OMT_SQ: i32 = 166;
    pub const OMT_HQ: i32 = 199;
}

#[cfg(not(libvmx_missing))]
mod ffi {
    use std::os::raw::c_int;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct VmxSize {
        pub width: c_int,
        pub height: c_int,
    }

    #[repr(C)]
    pub struct VmxInstance {
        _private: [u8; 0],
    }

    extern "C" {
        pub fn VMX_Create(dimensions: VmxSize, profile: c_int, color_space: c_int) -> *mut VmxInstance;
        pub fn VMX_SetQuality(instance: *mut VmxInstance, q: c_int);
        pub fn VMX_GetQuality(instance: *mut VmxInstance) -> c_int;
        pub fn VMX_GetEncodingParameters(
            instance: *mut VmxInstance,
            frame_min: *mut c_int,
            frame_max: *mut c_int,
            min_quality: *mut c_int,
            dc_shift: *mut c_int,
        );
        pub fn VMX_SetEncodingParameters(
            instance: *mut VmxInstance,
            frame_min: c_int,
            frame_max: c_int,
            min_quality: c_int,
            dc_shift: c_int,
        );
        pub fn VMX_EncodeUYVY(i: *mut VmxInstance, src: *mut u8, stride: c_int, interlaced: c_int) -> c_int;
        pub fn VMX_EncodeUYVA(i: *mut VmxInstance, src: *mut u8, stride: c_int, interlaced: c_int) -> c_int;
        pub fn VMX_EncodeYUY2(i: *mut VmxInstance, src: *mut u8, stride: c_int, interlaced: c_int) -> c_int;
        pub fn VMX_EncodeP216(i: *mut VmxInstance, src: *mut u8, stride: c_int, interlaced: c_int) -> c_int;
        pub fn VMX_EncodePA16(i: *mut VmxInstance, src: *mut u8, stride: c_int, interlaced: c_int) -> c_int;
        pub fn VMX_EncodeNV12(
            i: *mut VmxInstance,
            y: *mut u8,
            ys: c_int,
            uv: *mut u8,
            uvs: c_int,
            interlaced: c_int,
        ) -> c_int;
        pub fn VMX_EncodeYV12(
            i: *mut VmxInstance,
            y: *mut u8,
            ys: c_int,
            u: *mut u8,
            us: c_int,
            v: *mut u8,
            vs: c_int,
            interlaced: c_int,
        ) -> c_int;
        pub fn VMX_SaveTo(i: *mut VmxInstance, dst: *mut u8, max_len: c_int) -> c_int;
        pub fn VMX_LoadFrom(i: *mut VmxInstance, data: *mut u8, len: c_int) -> c_int;
        pub fn VMX_DecodeUYVY(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn VMX_DecodeUYVA(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn VMX_DecodeYUY2(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn VMX_DecodeP216(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn VMX_DecodePA16(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn VMX_DecodePreviewUYVY(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn VMX_DecodeBGRA(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn VMX_DecodeBGRX(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn VMX_DecodePreviewUYVA(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn VMX_DecodePreviewBGRA(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn VMX_DecodePreviewBGRX(i: *mut VmxInstance, dst: *mut u8, stride: c_int) -> c_int;
        pub fn vmxref_set_avx2(i: *mut VmxInstance, enabled: c_int);
        pub fn vmxref_set_threads(i: *mut VmxInstance, n: c_int);
        pub fn vmxref_destroy(i: *mut VmxInstance);
    }
}

/// Packed / planar layouts understood by the reference wrapper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefFormat {
    Uyvy,
    Uyva,
    Yuy2,
    P216,
    Pa16,
}

/// Safe owner of one `VMX_INSTANCE`.
#[cfg_attr(libvmx_missing, allow(dead_code))]
pub struct RefCodec {
    #[cfg(not(libvmx_missing))]
    inst: *mut ffi::VmxInstance,
    width: usize,
    height: usize,
}

// The instance is only ever used from one thread at a time through &mut self.
unsafe impl Send for RefCodec {}

#[cfg(not(libvmx_missing))]
impl RefCodec {
    /// Creates an instance. `simd256` enables the AVX2 path where available;
    /// with `false` the 128-bit (SSE / NEON-via-sse2neon) path is used.
    pub fn new(width: usize, height: usize, profile: i32, threads: i32, simd256: bool) -> Option<Self> {
        Self::with_color_space(width, height, profile, threads, simd256, 0)
    }

    /// As [`RefCodec::new`], with a `VMX_COLORSPACE` (0, 601 or 709), which
    /// picks the YUV/RGB matrix of the BGRA functions.
    pub fn with_color_space(
        width: usize,
        height: usize,
        profile: i32,
        threads: i32,
        simd256: bool,
        color_space: i32,
    ) -> Option<Self> {
        let inst = unsafe {
            ffi::VMX_Create(
                ffi::VmxSize { width: width as i32, height: height as i32 },
                profile,
                color_space,
            )
        };
        if inst.is_null() {
            return None;
        }
        unsafe {
            // VMX_SetThreads can deadlock tearing down the old pool; see csrc/shim.cpp.
            ffi::vmxref_set_threads(inst, threads);
            if !simd256 {
                ffi::vmxref_set_avx2(inst, 0);
            }
        }
        Some(Self { inst, width, height })
    }

    pub fn set_quality(&mut self, q: i32) {
        unsafe { ffi::VMX_SetQuality(self.inst, q) }
    }

    pub fn quality(&mut self) -> i32 {
        unsafe { ffi::VMX_GetQuality(self.inst) }
    }

    /// (frame_min, frame_max, min_quality, dc_shift)
    pub fn encoding_parameters(&mut self) -> (i32, i32, i32, i32) {
        let (mut a, mut b, mut c, mut d) = (0, 0, 0, 0);
        unsafe { ffi::VMX_GetEncodingParameters(self.inst, &mut a, &mut b, &mut c, &mut d) };
        (a, b, c, d)
    }

    pub fn set_encoding_parameters(&mut self, frame_min: i32, frame_max: i32, min_q: i32, dc_shift: i32) {
        unsafe { ffi::VMX_SetEncodingParameters(self.inst, frame_min, frame_max, min_q, dc_shift) }
    }

    /// Bytes per row of `fmt` for this instance (tight packing).
    pub fn stride(&self, _fmt: RefFormat) -> usize {
        self.width * 2
    }

    /// Total buffer size for `fmt` with tight packing.
    pub fn frame_len(&self, fmt: RefFormat) -> usize {
        let s = self.stride(fmt) * self.height;
        match fmt {
            RefFormat::Uyvy | RefFormat::Yuy2 => s,
            RefFormat::Uyva => s + self.width * self.height,
            RefFormat::P216 => 2 * s,
            RefFormat::Pa16 => 3 * s,
        }
    }

    /// Encodes one frame and returns the compressed bytes.
    pub fn encode(&mut self, fmt: RefFormat, src: &[u8], interlaced: bool) -> Vec<u8> {
        assert!(src.len() >= self.frame_len(fmt));
        let mut copy = src.to_vec();
        let stride = self.stride(fmt) as i32;
        let il = interlaced as i32;
        let p = copy.as_mut_ptr();
        let err = unsafe {
            match fmt {
                RefFormat::Uyvy => ffi::VMX_EncodeUYVY(self.inst, p, stride, il),
                RefFormat::Uyva => ffi::VMX_EncodeUYVA(self.inst, p, stride, il),
                RefFormat::Yuy2 => ffi::VMX_EncodeYUY2(self.inst, p, stride, il),
                RefFormat::P216 => ffi::VMX_EncodeP216(self.inst, p, stride, il),
                RefFormat::Pa16 => ffi::VMX_EncodePA16(self.inst, p, stride, il),
            }
        };
        assert_eq!(err, 0, "VMX_Encode failed");
        self.save()
    }

    /// Encodes 4:2:0 NV12 (Y plane then interleaved UV plane, tight strides).
    pub fn encode_nv12(&mut self, y: &[u8], uv: &[u8]) -> Vec<u8> {
        let mut y = y.to_vec();
        let mut uv = uv.to_vec();
        let w = self.width as i32;
        let err = unsafe { ffi::VMX_EncodeNV12(self.inst, y.as_mut_ptr(), w, uv.as_mut_ptr(), w, 0) };
        assert_eq!(err, 0);
        self.save()
    }

    /// Encodes 4:2:0 planar (I420/YV12: separate U and V planes, tight strides).
    pub fn encode_yv12(&mut self, y: &[u8], u: &[u8], v: &[u8]) -> Vec<u8> {
        let (mut y, mut u, mut v) = (y.to_vec(), u.to_vec(), v.to_vec());
        let w = self.width as i32;
        let err = unsafe {
            ffi::VMX_EncodeYV12(self.inst, y.as_mut_ptr(), w, u.as_mut_ptr(), w / 2, v.as_mut_ptr(), w / 2, 0)
        };
        assert_eq!(err, 0);
        self.save()
    }

    fn save(&mut self) -> Vec<u8> {
        let cap = self.width * self.height * 8 + 4096;
        let mut out = vec![0u8; cap];
        let n = unsafe { ffi::VMX_SaveTo(self.inst, out.as_mut_ptr(), cap as i32) };
        assert!(n > 0, "VMX_SaveTo failed");
        out.truncate(n as usize);
        out
    }

    /// Loads a compressed frame and decodes it into `fmt`.
    pub fn decode(&mut self, data: &[u8], fmt: RefFormat) -> Result<Vec<u8>, i32> {
        let mut copy = data.to_vec();
        let err = unsafe { ffi::VMX_LoadFrom(self.inst, copy.as_mut_ptr(), copy.len() as i32) };
        if err != 0 {
            return Err(err);
        }
        let mut out = vec![0u8; self.frame_len(fmt)];
        let stride = self.stride(fmt) as i32;
        let p = out.as_mut_ptr();
        let err = unsafe {
            match fmt {
                RefFormat::Uyvy => ffi::VMX_DecodeUYVY(self.inst, p, stride),
                RefFormat::Uyva => ffi::VMX_DecodeUYVA(self.inst, p, stride),
                RefFormat::Yuy2 => ffi::VMX_DecodeYUY2(self.inst, p, stride),
                RefFormat::P216 => ffi::VMX_DecodeP216(self.inst, p, stride),
                RefFormat::Pa16 => ffi::VMX_DecodePA16(self.inst, p, stride),
            }
        };
        if err != 0 {
            return Err(err);
        }
        Ok(out)
    }

    /// Decodes the 1/8-scale DC-only preview as UYVY.
    pub fn decode_preview_uyvy(&mut self, data: &[u8]) -> Result<(Vec<u8>, usize, usize), i32> {
        let mut copy = data.to_vec();
        let err = unsafe { ffi::VMX_LoadFrom(self.inst, copy.as_mut_ptr(), copy.len() as i32) };
        if err != 0 {
            return Err(err);
        }
        let mut pw = self.width >> 3;
        if pw % 2 != 0 {
            pw += 1;
        }
        let ph = self.height >> 3;
        let mut out = vec![0u8; pw * 2 * ph];
        let err = unsafe { ffi::VMX_DecodePreviewUYVY(self.inst, out.as_mut_ptr(), (pw * 2) as i32) };
        if err != 0 {
            return Err(err);
        }
        Ok((out, pw, ph))
    }

    fn load(&mut self, data: &[u8]) -> Result<(), i32> {
        let mut copy = data.to_vec();
        match unsafe { ffi::VMX_LoadFrom(self.inst, copy.as_mut_ptr(), copy.len() as i32) } {
            0 => Ok(()),
            e => Err(e),
        }
    }

    /// Decodes to packed BGRA (`alpha`) or BGRX (alpha 255), stride `4w`.
    pub fn decode_bgra(&mut self, data: &[u8], alpha: bool) -> Result<Vec<u8>, i32> {
        self.load(data)?;
        let mut out = vec![0u8; self.width * 4 * self.height];
        let (p, stride) = (out.as_mut_ptr(), (self.width * 4) as i32);
        let err = unsafe {
            if alpha {
                ffi::VMX_DecodeBGRA(self.inst, p, stride)
            } else {
                ffi::VMX_DecodeBGRX(self.inst, p, stride)
            }
        };
        if err != 0 {
            return Err(err);
        }
        Ok(out)
    }

    /// Decodes the progressive preview as UYVA (UYVY then alpha) or as BGRA
    /// / BGRX, like libomtnet's receiver (`OMTReceive.cs:797-836`).
    pub fn decode_preview_as(&mut self, data: &[u8], fmt: RefPreview) -> Result<(Vec<u8>, usize, usize), i32> {
        self.load(data)?;
        let pw = (self.width >> 3) + ((self.width >> 3) & 1);
        let ph = self.height >> 3;
        let (bytes, stride) = match fmt {
            RefPreview::Uyva => (pw * 3 * ph, pw * 2),
            RefPreview::Bgra | RefPreview::Bgrx => (pw * 4 * ph, pw * 4),
        };
        let mut out = vec![0u8; bytes];
        let (p, s) = (out.as_mut_ptr(), stride as i32);
        let err = unsafe {
            match fmt {
                RefPreview::Uyva => ffi::VMX_DecodePreviewUYVA(self.inst, p, s),
                RefPreview::Bgra => ffi::VMX_DecodePreviewBGRA(self.inst, p, s),
                RefPreview::Bgrx => ffi::VMX_DecodePreviewBGRX(self.inst, p, s),
            }
        };
        if err != 0 {
            return Err(err);
        }
        Ok((out, pw, ph))
    }
}

/// Preview layouts for [`RefCodec::decode_preview_as`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefPreview {
    Uyva,
    Bgra,
    Bgrx,
}

#[cfg(not(libvmx_missing))]
impl Drop for RefCodec {
    fn drop(&mut self) {
        unsafe { ffi::vmxref_destroy(self.inst) }
    }
}

// Without the upstream sources `new` returns `None`, so the methods below are
// never reached; they exist so the tests and benchmark that use them still
// compile (and then skip themselves), e.g. in CI.
#[cfg(libvmx_missing)]
#[allow(unused_variables, dead_code)]
impl RefCodec {
    pub fn new(_w: usize, _h: usize, _p: i32, _t: i32, _s: bool) -> Option<Self> {
        None
    }
    pub fn set_quality(&mut self, q: i32) {
        unreachable!("libvmx reference not built")
    }
    pub fn quality(&mut self) -> i32 {
        unreachable!("libvmx reference not built")
    }
    pub fn encoding_parameters(&mut self) -> (i32, i32, i32, i32) {
        unreachable!("libvmx reference not built")
    }
    pub fn set_encoding_parameters(&mut self, frame_min: i32, frame_max: i32, min_q: i32, dc_shift: i32) {
        unreachable!("libvmx reference not built")
    }
    pub fn stride(&self, fmt: RefFormat) -> usize {
        unreachable!("libvmx reference not built")
    }
    pub fn frame_len(&self, fmt: RefFormat) -> usize {
        unreachable!("libvmx reference not built")
    }
    pub fn encode(&mut self, fmt: RefFormat, src: &[u8], interlaced: bool) -> Vec<u8> {
        unreachable!("libvmx reference not built")
    }
    pub fn encode_nv12(&mut self, y: &[u8], uv: &[u8]) -> Vec<u8> {
        unreachable!("libvmx reference not built")
    }
    pub fn encode_yv12(&mut self, y: &[u8], u: &[u8], v: &[u8]) -> Vec<u8> {
        unreachable!("libvmx reference not built")
    }
    pub fn decode(&mut self, data: &[u8], fmt: RefFormat) -> Result<Vec<u8>, i32> {
        unreachable!("libvmx reference not built")
    }
    pub fn decode_preview_uyvy(&mut self, data: &[u8]) -> Result<(Vec<u8>, usize, usize), i32> {
        unreachable!("libvmx reference not built")
    }
    pub fn with_color_space(_w: usize, _h: usize, _p: i32, _t: i32, _s: bool, _c: i32) -> Option<Self> {
        None
    }
    pub fn decode_bgra(&mut self, data: &[u8], alpha: bool) -> Result<Vec<u8>, i32> {
        unreachable!("libvmx reference not built")
    }
    pub fn decode_preview_as(&mut self, data: &[u8], fmt: RefPreview) -> Result<(Vec<u8>, usize, usize), i32> {
        unreachable!("libvmx reference not built")
    }
}
