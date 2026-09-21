//! Bytes captured from upstream libomtnet (v1.0.0.19) talking to itself, see
//! `docs/evidence/2026-09-21-libomtnet-loopback`. These are the only tests in
//! the crate whose expected values did not come from reading the spec.

use open_media_transport::command::{classify, Command, Message, Quality, Tally};
use open_media_transport::{frame, Deframer, Limits, OwnedFrame};

const RECV_VIDEO_CONN: &[u8] = include_bytes!("data/libomtnet-recv-video-conn.bin");
const RECV_AUDIO_CONN: &[u8] = include_bytes!("data/libomtnet-recv-audio-conn.bin");
const SEND_ACCEPT: &[u8] = include_bytes!("data/libomtnet-send-accept.bin");

fn frames(bytes: &[u8]) -> Vec<OwnedFrame> {
    let mut d = Deframer::new(Limits::VIDEO);
    d.push(bytes);
    let mut out = Vec::new();
    while let Some(f) = d.next_frame().unwrap() {
        out.push(f);
    }
    assert_eq!(d.buffered(), 0, "trailing bytes");
    out
}

fn commands(cmds: &[Command]) -> Vec<u8> {
    let mut out = Vec::new();
    for c in cmds {
        frame::write_metadata(0, c.as_bytes(), &mut out);
    }
    out
}

#[test]
fn receiver_video_connection_is_byte_identical() {
    // Receiver wanting video, audio and metadata, tally program, quality High.
    let ours = commands(&[
        Command::SubscribeMetadata,
        Command::SubscribeVideo,
        Command::Quality(Quality::High),
        Command::Tally(Tally {
            preview: false,
            program: true,
        }),
    ]);
    assert_eq!(ours, RECV_VIDEO_CONN);
}

#[test]
fn receiver_audio_connection_is_byte_identical() {
    assert_eq!(commands(&[Command::SubscribeAudio]), RECV_AUDIO_CONN);
}

#[test]
fn sender_accept_sequence_parses() {
    let f = frames(SEND_ACCEPT);
    assert_eq!(f.len(), 4);
    assert!(f
        .iter()
        .all(|f| f.header.timestamp == 0 && f.metadata.is_empty()));
    assert_eq!(
        f[0].data,
        br#"<OMTInfo ProductName="omt-harness" Manufacturer="open-media-transport" Version="0.1" />"#
    );
    assert!(matches!(classify(&f[0].data), Message::SenderInfo(_)));
    assert_eq!(
        classify(&f[1].data),
        Message::Application(br#"<HarnessHello Value="1" />"#)
    );
    assert_eq!(
        classify(&f[2].data),
        Message::Command(Command::Tally(Tally::default()))
    );
    assert_eq!(
        classify(&f[3].data),
        Message::Command(Command::Tally(Tally {
            preview: false,
            program: true
        }))
    );
}

#[test]
fn no_captured_command_carries_a_nul() {
    for f in frames(RECV_VIDEO_CONN)
        .into_iter()
        .chain(frames(RECV_AUDIO_CONN))
        .chain(frames(SEND_ACCEPT))
    {
        assert!(!f.data.contains(&0));
    }
}
