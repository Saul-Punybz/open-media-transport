//! `media` against the libvmx C++ reference: the layouts `vmx-codec` does not
//! produce itself (BGRA/BGRX, preview UYVA/BGRA) must match libvmx byte for
//! byte, since that is what libomtnet's receiver hands to applications
//! (`OMTReceive.cs:797-887`). Skipped when `reference/libvmx` is missing.

use libvmx_ref::{profile, RefCodec, RefPreview, AVAILABLE};
use open_media_transport::frame::{
    ExtendedHeader, FrameHeader, FrameType, VideoFlags, VideoHeader, CODEC_VMX1,
};
use open_media_transport::media::{MediaDecoder, PreferredVideoFormat, VideoFormat, VideoFrame};
use open_media_transport::OwnedFrame;
use vmx_codec::{Encoder, EncoderConfig, Frame, PixelFormat, Profile};

/// Noise with a slow gradient, so both smooth areas and clipping occur.
fn picture(w: usize, h: usize, format: PixelFormat, interlaced: bool) -> Frame {
    let mut f = Frame::new(w, h, format);
    f.interlaced = interlaced;
    let mut seed = 0x2545_f491u32;
    for plane in &mut f.planes {
        for (i, b) in plane.data.iter_mut().enumerate() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *b = ((i / 7 % 256) as u32 / 2 + (seed >> 25)) as u8;
        }
    }
    f
}

fn packet(f: &Frame) -> Vec<u8> {
    let mut cfg = EncoderConfig::new(f.width, f.height);
    cfg.profile = Profile::OmtHq;
    Encoder::new(cfg).unwrap().encode(f).unwrap()
}

fn decode(
    pref: PreferredVideoFormat,
    data: &[u8],
    w: usize,
    h: usize,
    flags: u32,
    color_space: i32,
) -> VideoFrame {
    let f = OwnedFrame {
        header: FrameHeader {
            frame_type: FrameType::Video,
            timestamp: 0,
            metadata_length: 0,
            data_length: 0,
        },
        ext: ExtendedHeader::Video(VideoHeader {
            codec: CODEC_VMX1,
            width: w as i32,
            height: h as i32,
            frame_rate_n: 30,
            frame_rate_d: 1,
            aspect_ratio: 1.0,
            flags: VideoFlags(flags),
            color_space,
        }),
        data: data.to_vec(),
        metadata: Vec::new(),
    };
    let mut out = VideoFrame::default();
    MediaDecoder::new(pref).decode_video(&f, &mut out).unwrap();
    out
}

fn reference(w: usize, h: usize, color_space: i32) -> RefCodec {
    RefCodec::with_color_space(w, h, profile::OMT_HQ, 1, false, color_space).unwrap()
}

#[test]
fn bgra_matches_libvmx() {
    if !AVAILABLE {
        eprintln!("libvmx reference not built; skipping");
        return;
    }
    // (size, source, interlaced, colour space): 709 by default at 720 lines,
    // 601 by default below, explicit 709 below 720, interlaced 601.
    let cases = [
        (1280, 720, PixelFormat::Uyva, false, 0),
        (640, 360, PixelFormat::Uyva, false, 0),
        (640, 360, PixelFormat::Uyvy, false, 709),
        (720, 480, PixelFormat::Uyvy, true, 601),
        (1920, 1080, PixelFormat::Uyva, true, 0),
    ];
    for (w, h, fmt, interlaced, cs) in cases {
        let data = packet(&picture(w, h, fmt, interlaced));
        let mut flags = if interlaced {
            VideoFlags::INTERLACED
        } else {
            0
        };
        let alpha = fmt.has_alpha();
        if alpha {
            flags |= VideoFlags::ALPHA;
        }
        let ours = decode(PreferredVideoFormat::Bgra, &data, w, h, flags, cs);
        assert_eq!((ours.format, ours.stride), (VideoFormat::Bgra, 4 * w));
        let theirs = reference(w, h, cs).decode_bgra(&data, alpha).unwrap();
        assert!(ours.data == theirs, "BGRA {w}x{h} cs={cs} alpha={alpha}");
        if alpha {
            // The same stream for a receiver that ignores alpha: BGRX.
            let ours = decode(
                PreferredVideoFormat::Bgra,
                &data,
                w,
                h,
                flags & !VideoFlags::ALPHA,
                cs,
            );
            let theirs = reference(w, h, cs).decode_bgra(&data, false).unwrap();
            assert!(ours.data == theirs, "BGRX {w}x{h} cs={cs}");
        }
    }
}

#[test]
fn previews_match_libvmx() {
    if !AVAILABLE {
        eprintln!("libvmx reference not built; skipping");
        return;
    }
    for (w, h, cs) in [(1280, 720, 0), (640, 360, 0), (632, 360, 709)] {
        let data = packet(&picture(w, h, PixelFormat::Uyva, false));
        let flags = VideoFlags::PREVIEW | VideoFlags::ALPHA;
        for (pref, reff, flags) in [
            (PreferredVideoFormat::UyvyOrUyva, RefPreview::Uyva, flags),
            (PreferredVideoFormat::Bgra, RefPreview::Bgra, flags),
            (
                PreferredVideoFormat::Bgra,
                RefPreview::Bgrx,
                flags & !VideoFlags::ALPHA,
            ),
        ] {
            let ours = decode(pref, &data, w, h, flags, cs);
            let (theirs, pw, ph) = reference(w, h, cs).decode_preview_as(&data, reff).unwrap();
            assert_eq!((ours.width, ours.height), (pw, ph));
            assert!(ours.data == theirs, "{reff:?} preview {w}x{h} cs={cs}");
        }
    }
}
