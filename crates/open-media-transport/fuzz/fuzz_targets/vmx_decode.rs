//! Arbitrary bytes as a VMX1 bitstream, at arbitrary dimensions, through
//! `vmx_codec::Decoder`: header parsing, full decode (8- and 10-bit, with and
//! without alpha) and the 1/8 preview. A receiver hands the decoder whatever
//! the network sent, with the size from the frame header, so nothing here may
//! panic. Errors are fine.
//!
//! Input: `w w h h sel` then the bitstream. Width and height are taken modulo
//! 1025, so invalid sizes (odd, under 16) are tried too without letting one
//! input allocate gigabytes. `sel` picks the output format and thread count.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vmx_codec::{Decoder, PixelFormat};

const FORMATS: [PixelFormat; 7] = [
    PixelFormat::Uyvy,
    PixelFormat::Yuy2,
    PixelFormat::Uyva,
    PixelFormat::Yuv422p,
    PixelFormat::Yuva422p,
    PixelFormat::P216,
    PixelFormat::Pa16,
];

fuzz_target!(|data: &[u8]| {
    if data.len() < 5 {
        return;
    }
    let width = u16::from_le_bytes([data[0], data[1]]) as usize % 1025;
    let height = u16::from_le_bytes([data[2], data[3]]) as usize % 1025;
    let sel = data[4];
    let stream = &data[5..];
    let Ok(mut dec) = Decoder::new(width, height) else {
        return;
    };
    dec.set_threads(1 + (sel as usize >> 6));
    let _ = dec.info(stream);
    // The preview needs only the DC prefix (`VMX_GetEncodedPreviewLength`):
    // whatever it decodes from the whole frame it must decode from the prefix.
    if let Ok(n) = dec.preview_len(stream) {
        assert!(n <= stream.len(), "preview_len {n} > {}", stream.len());
        let alpha = sel & 0x20 != 0;
        let full = dec.decode_preview(stream, alpha);
        let prefix = dec.decode_preview(&stream[..n], alpha);
        assert_eq!(full.is_ok(), prefix.is_ok());
        if let (Ok(a), Ok(b)) = (full, prefix) {
            assert_eq!(a.planes, b.planes);
        }
    }
    let format = FORMATS[sel as usize % FORMATS.len()];
    let _ = dec.decode(stream, format);
});
