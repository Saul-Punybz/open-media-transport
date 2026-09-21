//! Parses one direction of a captured OMT TCP stream and prints every frame.
//!
//! ```sh
//! # one direction of a TCP stream, as raw bytes:
//! tshark -r capture.pcapng -q -z follow,tcp,raw,0 ...   # then hex -> binary
//! cargo run -p open-media-transport --example omt-dump -- stream.bin
//! ```
//!
//! Metadata payloads are printed with non-printable bytes escaped, so a
//! terminating NUL shows up as `\x00`.

use open_media_transport::command::{classify, Message};
use open_media_transport::frame::ExtendedHeader;
use open_media_transport::{Deframer, Limits};

fn main() {
    let path = std::env::args().nth(1).expect("usage: omt-dump STREAM.bin");
    let bytes = std::fs::read(&path).expect("read stream file");
    let mut d = Deframer::new(Limits::VIDEO);
    d.push(&bytes);
    let mut n = 0;
    loop {
        match d.next_frame() {
            Ok(Some(f)) => {
                let h = f.header;
                let what = match f.ext {
                    ExtendedHeader::None => {
                        let kind = match classify(&f.data) {
                            Message::Command(c) => format!("command {c:?}"),
                            Message::QualityOther(_) => "quality (other form)".into(),
                            Message::SenderInfo(_) => "sender-info".into(),
                            Message::Redirect(_) => "redirect".into(),
                            Message::Application(_) => "application".into(),
                        };
                        format!("{kind} \"{}\"", escape(&f.data))
                    }
                    ExtendedHeader::Video(v) => format!(
                        "codec={} {}x{} rate={}/{} aspect={:.4} flags={} cs={} data={}",
                        fourcc(v.codec),
                        v.width,
                        v.height,
                        v.frame_rate_n,
                        v.frame_rate_d,
                        v.aspect_ratio,
                        v.flags.0,
                        v.color_space,
                        f.data.len()
                    ),
                    ExtendedHeader::Audio(a) => format!(
                        "codec={} rate={} spc={} ch={} active={:#x} reserved={} data={}",
                        fourcc(a.codec),
                        a.sample_rate,
                        a.samples_per_channel,
                        a.channels,
                        a.active_channels,
                        a.reserved,
                        f.data.len()
                    ),
                };
                let meta = if f.metadata.is_empty() {
                    String::new()
                } else {
                    format!(" meta=\"{}\"", escape(&f.metadata))
                };
                println!(
                    "{n:5} {:?} ts={} mlen={} dlen={} {what}{meta}",
                    h.frame_type, h.timestamp, h.metadata_length, h.data_length
                );
                n += 1;
            }
            Ok(None) => break,
            Err(e) => {
                println!("error after {n} frames: {e}");
                std::process::exit(1);
            }
        }
    }
    println!("{n} frames, {} trailing bytes", d.buffered());
}

fn fourcc(c: u32) -> String {
    escape(&c.to_le_bytes())
}

fn escape(b: &[u8]) -> String {
    b.iter()
        .map(|&x| {
            if (32..127).contains(&x) && x != b'"' {
                (x as char).to_string()
            } else {
                format!("\\x{x:02X}")
            }
        })
        .collect()
}
