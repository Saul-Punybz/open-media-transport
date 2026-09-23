//! Our sender to our receiver through `media`: the decoded pixels, samples and
//! XML must be what went in (pixels: what a fresh OMT_SQ encoder + decoder
//! make of the source, since the sender's first frame is exactly that).

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use open_media_transport::media::{Media, MediaDecoder, PreferredVideoFormat, VideoFormat};
use open_media_transport::receiver::{Event, Receiver, ReceiverConfig};
use open_media_transport::sender::{Sender, SenderConfig, VideoParams};
use vmx_codec::{Decoder, Encoder, EncoderConfig, Frame, PixelFormat, Profile};

fn sender(name: &str) -> Sender {
    Sender::new(SenderConfig {
        announce: false,
        ports: 16700..=16900,
        ..SenderConfig::new(name)
    })
    .unwrap()
}

fn wait_for(mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn source(w: usize, h: usize, format: PixelFormat) -> Frame {
    let mut f = Frame::new(w, h, format);
    for (p, plane) in f.planes.iter_mut().enumerate() {
        for (i, b) in plane.data.iter_mut().enumerate() {
            *b = ((i * (5 + p) / 3) % 220) as u8 + 16;
        }
    }
    f
}

fn expected(f: &Frame, out: PixelFormat) -> Vec<u8> {
    let mut cfg = EncoderConfig::new(f.width, f.height);
    cfg.profile = Profile::OmtSq; // Quality Default (V3)
    let packet = Encoder::new(cfg).unwrap().encode(f).unwrap();
    let d = Decoder::new(f.width, f.height)
        .unwrap()
        .decode(&packet, out)
        .unwrap();
    d.planes
        .iter()
        .flat_map(|p| p.data.iter().copied())
        .collect()
}

const PARAMS: VideoParams = VideoParams {
    frame_rate_n: 50,
    frame_rate_d: 1,
    aspect_ratio: 2.0,
    color_space: 709,
    premultiplied: false,
};

/// Sends `frame` once and returns the first decoded video frame.
fn round_trip(
    name: &str,
    frame: &Frame,
    pref: PreferredVideoFormat,
) -> open_media_transport::media::VideoFrame {
    let tx = sender(name);
    let addr = SocketAddr::from(([127, 0, 0, 1], tx.port()));
    let cfg = ReceiverConfig {
        audio: false,
        ..ReceiverConfig::default()
    };
    let rx = Receiver::connect(addr, cfg).unwrap();
    wait_for(|| tx.video_receivers() == 1);
    assert_eq!(tx.send_video(frame, PARAMS, 777, b"<F />\0"), Ok(1));
    let mut dec = MediaDecoder::new(pref);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(Event::Frame(_, f)) = rx.recv_timeout(Duration::from_millis(100)) {
            if let Some(Media::Video(v)) = dec.decode(&f).unwrap() {
                return v.clone();
            }
        }
    }
    panic!("no video");
}

#[test]
fn uyvy_round_trip() {
    let src = source(256, 64, PixelFormat::Uyvy);
    let v = round_trip(
        "media-uyvy",
        &src,
        PreferredVideoFormat::UyvyOrUyvaOrP216OrPa16,
    );
    assert_eq!(
        (v.format, v.width, v.height, v.stride),
        (VideoFormat::Uyvy, 256, 64, 512)
    );
    assert_eq!(
        (v.timestamp, v.frame_rate(), v.aspect_ratio),
        (777, 50.0, 2.0)
    );
    assert_eq!(
        (v.color_space, v.metadata.as_slice()),
        (709, &b"<F />\0"[..])
    );
    assert!(!v.is_high_bit_depth() && !v.has_alpha());
    assert!(v.data == expected(&src, PixelFormat::Uyvy));
}

#[test]
fn p216_round_trip_keeps_ten_bits() {
    let mut src = Frame::new(128, 32, PixelFormat::P216);
    // A 10-bit luma ramp: levels an 8-bit path cannot hold.
    for (i, px) in src.planes[0].data.chunks_exact_mut(2).enumerate() {
        let v10 = 64 + (i % 128) as u16 * 7;
        px.copy_from_slice(&(v10 << 6).to_le_bytes());
    }
    for px in src.planes[1].data.chunks_exact_mut(2) {
        px.copy_from_slice(&(512u16 << 6).to_le_bytes());
    }
    let v = round_trip(
        "media-p216",
        &src,
        PreferredVideoFormat::UyvyOrUyvaOrP216OrPa16,
    );
    assert_eq!(v.format, VideoFormat::P216);
    assert!(v.is_high_bit_depth());
    assert!(v.data == expected(&src, PixelFormat::P216));
    let lsb2: usize = v.planes()[0]
        .chunks_exact(2)
        .map(|b| ((u16::from_le_bytes([b[0], b[1]]) >> 6) & 3 != 0) as usize)
        .sum();
    assert!(lsb2 > 1000, "10-bit detail kept ({lsb2} samples)");
}

#[test]
fn alpha_round_trips_as_uyva_bgra_and_pa16() {
    let src = source(64, 32, PixelFormat::Uyva);
    let v = round_trip("media-uyva", &src, PreferredVideoFormat::UyvyOrUyva);
    assert_eq!(v.format, VideoFormat::Uyva);
    assert!(v.has_alpha());
    assert!(v.data == expected(&src, PixelFormat::Uyva));
    let v = round_trip("media-bgra", &src, PreferredVideoFormat::UyvyOrBgra);
    assert_eq!((v.format, v.data.len()), (VideoFormat::Bgra, 64 * 4 * 32));
    // Alpha came through: the source's alpha plane, after coding.
    let alpha: Vec<u8> = v.data.chunks_exact(4).map(|p| p[3]).collect();
    assert!(alpha == expected(&src, PixelFormat::Uyva)[64 * 2 * 32..]);

    let src = source(64, 32, PixelFormat::Pa16);
    let v = round_trip(
        "media-pa16",
        &src,
        PreferredVideoFormat::UyvyOrUyvaOrP216OrPa16,
    );
    assert_eq!(v.format, VideoFormat::Pa16);
    assert!(v.has_alpha() && v.is_high_bit_depth());
    assert!(v.data == expected(&src, PixelFormat::Pa16));
}

#[test]
fn audio_and_metadata_round_trip() {
    let tx = sender("media-audio");
    let addr = SocketAddr::from(([127, 0, 0, 1], tx.port()));
    let rx = Receiver::connect(addr, ReceiverConfig::default()).unwrap();
    wait_for(|| tx.connections() == 2);
    // Three channels, the middle one silent (left out on the wire, A2).
    let mut samples = vec![0.0f32; 3 * 480];
    for i in 0..480 {
        samples[i] = (i as f32 / 480.0).sin();
        samples[2 * 480 + i] = -0.5;
    }
    wait_for(|| tx.send_audio(&samples, 3, 48000, 99, b"<A />") == Ok(1));
    assert!(tx.send_metadata(b"<App X=\"1\" />\0", 5) >= 1);

    let mut dec = MediaDecoder::new(PreferredVideoFormat::Uyvy);
    let (mut got_audio, mut got_meta) = (false, false);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !(got_audio && got_meta) && Instant::now() < deadline {
        let Some(Event::Frame(_, f)) = rx.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        match dec.decode(&f).unwrap() {
            Some(Media::Audio(a)) => {
                assert_eq!(
                    (a.channels, a.samples_per_channel, a.sample_rate),
                    (3, 480, 48000)
                );
                assert_eq!((a.timestamp, a.active_channels), (99, 0b101));
                assert_eq!(a.samples, samples);
                assert_eq!(a.metadata, b"<A />");
                got_audio = true;
            }
            Some(Media::Metadata(m)) if !m.sender_info => {
                assert_eq!((m.timestamp, m.text().as_ref()), (5, "<App X=\"1\" />"));
                got_meta = true;
            }
            _ => {}
        }
    }
    assert!(
        got_audio && got_meta,
        "audio {got_audio} metadata {got_meta}"
    );
}
