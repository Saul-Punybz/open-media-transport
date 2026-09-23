//! Sends frames that were VMX1-compressed beforehand, with
//! [`Sender::send_encoded_video`], the way a relay forwards what it received.
//!
//! ```sh
//! cargo run --release -p open-media-transport --example omt-send-encoded -- "Encoded" 10
//! ```
//!
//! 640x360 UYVY at 30 fps, encoded with `vmx_codec` at `OMT_LQ` (a profile
//! the sender would not pick itself, V3). Every 30th frame carries
//! `<HarnessFrame N="n" />\0` and the program prints the length of the
//! bitstream it sent and the FNV-1a 64 hash of that bitstream decoded to UYVY
//! by `vmx_codec`, so a receiver's decoded pixels can be compared.

use std::time::{Duration, Instant};

use open_media_transport::clock::Clock;
use open_media_transport::frame::VideoFlags;
use open_media_transport::sender::{EncodedVideo, Sender, SenderConfig, VideoParams};
use vmx_codec::{Decoder, Encoder, EncoderConfig, Frame, PixelFormat, Profile};

const W: usize = 640;
const H: usize = 360;
const FPS: i64 = 30;

fn fnv1a64(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "Encoded".into());
    let seconds: u64 = args.next().map_or(10, |s| s.parse().unwrap());
    let tx = Sender::new(SenderConfig::new(name)).expect("start sender");
    println!(
        "send name=\"{}\" port={}",
        tx.full_name().unwrap_or("-"),
        tx.port()
    );
    let params = VideoParams {
        frame_rate_n: FPS as i32,
        frame_rate_d: 1,
        aspect_ratio: W as f32 / H as f32,
        color_space: 709,
        premultiplied: false,
    };
    let mut cfg = EncoderConfig::new(W, H);
    cfg.profile = Profile::OmtLq;
    let mut encoder = Encoder::new(cfg).expect("encoder");
    let mut decoder = Decoder::new(W, H).expect("decoder");
    let mut frame = Frame::new(W, H, PixelFormat::Uyvy);
    let mut clock = Clock::new();
    let start = Instant::now();
    for n in 0usize.. {
        if start.elapsed() >= Duration::from_secs(seconds) {
            break;
        }
        for (i, b) in frame.planes[0].data.iter_mut().enumerate() {
            let (x, y) = (i % (W * 2), i / (W * 2));
            *b = ((x + y * 3 + n * 4) % 220 + 16) as u8;
        }
        let bits = encoder.encode(&frame).expect("encode");
        let meta = if n % 30 == 0 {
            let decoded = decoder.decode(&bits, PixelFormat::Uyvy).expect("decode");
            println!(
                "sent N={n} vmx_len={} decoded_uyvy_fnv1a64={:016x}",
                bits.len(),
                fnv1a64(&decoded.planes[0].data)
            );
            format!("<HarnessFrame N=\"{n}\" />\0").into_bytes()
        } else {
            Vec::new()
        };
        let encoded = EncodedVideo {
            data: &bits,
            width: W,
            height: H,
            flags: VideoFlags::default(),
        };
        let ts = clock.video(FPS as i32, 1);
        tx.send_encoded_video(encoded, params, ts, &meta)
            .expect("send");
    }
    let s = tx.stats();
    println!(
        "send done video_sent={} video_dropped={} bytes={}",
        s.video.sent, s.video.dropped, s.bytes_sent
    );
}
