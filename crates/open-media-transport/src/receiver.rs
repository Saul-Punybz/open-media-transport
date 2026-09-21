//! A minimal blocking receiver: connects to a sender's TCP port the way
//! libomtnet does and delivers raw frames (`docs/PROTOCOL.md` §1, §4.3).
//!
//! Like libomtnet, it opens one connection for video + metadata and a second
//! one for audio (T5), and sends the same commands in the same order on each
//! (§4.3). Frames are returned as they arrive, protocol commands included;
//! decoding is left to the caller. There is no discovery and no reconnect yet.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::command::{Command, Quality, Tally};
use crate::{frame, Deframer, Error, Limits, OwnedFrame};

/// What to ask the sender for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceiverConfig {
    /// Subscribe to video.
    pub video: bool,
    /// Subscribe to audio (on a second connection).
    pub audio: bool,
    /// Ask for 1/8 preview video instead of full frames (§6.2).
    pub preview: bool,
    /// Suggested encoder quality, sent with the video subscription.
    pub quality: Quality,
    /// Initial tally.
    pub tally: Tally,
}

impl Default for ReceiverConfig {
    fn default() -> Self {
        ReceiverConfig {
            video: true,
            audio: true,
            preview: false,
            quality: Quality::Default,
            tally: Tally::default(),
        }
    }
}

/// Which of the receiver's connections an event came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    /// The video + metadata connection (or the only one, if metadata-only).
    Video,
    /// The audio connection.
    Audio,
}

/// Something that happened on a connection.
#[derive(Debug)]
pub enum Event {
    /// A complete frame.
    Frame(Channel, OwnedFrame),
    /// The connection closed: `None` for a clean close by the peer, otherwise
    /// the I/O or protocol error that ended it.
    Closed(Channel, Option<ReceiveError>),
}

/// Why a connection ended.
#[derive(Debug)]
pub enum ReceiveError {
    /// The socket failed.
    Io(io::Error),
    /// The peer sent something that is not a valid OMT stream.
    Protocol(Error),
}

struct Connection {
    stream: TcpStream,
    reader: Option<JoinHandle<()>>,
}

/// A connection to one sender.
pub struct Receiver {
    video: Option<Connection>,
    audio: Option<Connection>,
    // Commands go on the video connection, or the audio one if there is none
    // (`OMTReceive.cs:715-731`).
    control: Mutex<TcpStream>,
    events: mpsc::Receiver<Event>,
}

impl Receiver {
    /// Connects to a sender at `addr` and subscribes as `config` says.
    pub fn connect(addr: SocketAddr, config: ReceiverConfig) -> io::Result<Receiver> {
        let (tx, events) = mpsc::channel();
        let mut video = None;
        let mut audio = None;

        // §4.3, video connection (also used alone for metadata-only).
        if config.video || !config.audio {
            let mut cmds = vec![Command::SubscribeMetadata];
            if config.video {
                if config.preview {
                    cmds.push(Command::Preview(true));
                }
                cmds.push(Command::SubscribeVideo);
                cmds.push(Command::Quality(config.quality));
            }
            cmds.push(Command::Tally(config.tally));
            video = Some(open(
                addr,
                &cmds,
                Channel::Video,
                Limits::VIDEO,
                tx.clone(),
            )?);
        }
        // §4.3, audio connection.
        if config.audio {
            let mut cmds = Vec::new();
            if !config.video {
                cmds.push(Command::SubscribeMetadata);
            }
            cmds.push(Command::SubscribeAudio);
            audio = Some(open(
                addr,
                &cmds,
                Channel::Audio,
                Limits::AUDIO_OR_METADATA,
                tx,
            )?);
        }

        let control = video
            .as_ref()
            .or(audio.as_ref())
            .expect("at least one connection");
        let control = Mutex::new(control.stream.try_clone()?);
        Ok(Receiver {
            video,
            audio,
            control,
            events,
        })
    }

    /// Waits up to `timeout` for the next event.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Event> {
        self.events.recv_timeout(timeout).ok()
    }

    /// Sends a command to the sender, e.g. a tally or quality change.
    pub fn send(&self, command: Command) -> io::Result<()> {
        self.send_metadata(command.as_bytes())
    }

    /// Sends application metadata. Include a trailing NUL if the far end
    /// expects one; libomtnet passes on whatever it gets (M4).
    pub fn send_metadata(&self, xml: &[u8]) -> io::Result<()> {
        let mut out = Vec::new();
        frame::write_metadata(0, xml, &mut out);
        self.control.lock().unwrap().write_all(&out)
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        for c in [self.video.take(), self.audio.take()].into_iter().flatten() {
            let _ = c.stream.shutdown(Shutdown::Both);
            if let Some(h) = c.reader {
                let _ = h.join();
            }
        }
    }
}

fn open(
    addr: SocketAddr,
    commands: &[Command],
    channel: Channel,
    limits: Limits,
    tx: mpsc::Sender<Event>,
) -> io::Result<Connection> {
    let mut stream = TcpStream::connect(addr)?;
    // T3. libomtnet also enables TCP keepalive; std has no portable API for it.
    stream.set_nodelay(true)?;
    let mut out = Vec::new();
    for c in commands {
        frame::write_metadata(0, c.as_bytes(), &mut out);
    }
    stream.write_all(&out)?;
    let reader_stream = stream.try_clone()?;
    let reader = std::thread::Builder::new()
        .name(format!("omt-recv-{channel:?}"))
        .spawn(move || read_loop(reader_stream, channel, limits, tx))?;
    Ok(Connection {
        stream,
        reader: Some(reader),
    })
}

fn read_loop(mut stream: TcpStream, channel: Channel, limits: Limits, tx: mpsc::Sender<Event>) {
    let mut deframer = Deframer::new(limits);
    // libomtnet reads at most 128 KiB per call (`OMTConstants.cs:44`).
    let mut buf = vec![0u8; 128 * 1024];
    let reason = loop {
        let n = match stream.read(&mut buf) {
            Ok(0) => break None,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => break Some(ReceiveError::Io(e)),
        };
        deframer.push(&buf[..n]);
        loop {
            match deframer.next_frame() {
                Ok(Some(f)) => {
                    if tx.send(Event::Frame(channel, f)).is_err() {
                        return; // receiver dropped
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = stream.shutdown(Shutdown::Both);
                    let _ = tx.send(Event::Closed(channel, Some(ReceiveError::Protocol(e))));
                    return;
                }
            }
        }
    };
    let _ = tx.send(Event::Closed(channel, reason));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::classify;
    use crate::command::Message;
    use std::net::TcpListener;

    /// Accepts connections on a local port and returns what each one sent
    /// before the peer closed it.
    fn capture_connections(n: usize) -> (SocketAddr, std::thread::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let mut out = Vec::new();
            for _ in 0..n {
                let (mut s, _) = listener.accept().unwrap();
                s.set_read_timeout(Some(Duration::from_millis(300)))
                    .unwrap();
                let mut got = Vec::new();
                let mut buf = [0u8; 4096];
                while let Ok(k) = s.read(&mut buf) {
                    if k == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..k]);
                }
                out.push(got);
            }
            out
        });
        (addr, h)
    }

    fn commands_in(bytes: &[u8]) -> Vec<Command> {
        let mut d = Deframer::new(Limits::VIDEO);
        d.push(bytes);
        let mut out = Vec::new();
        while let Some(f) = d.next_frame().unwrap() {
            match classify(&f.data) {
                Message::Command(c) => out.push(c),
                other => panic!("not a command: {other:?}"),
            }
        }
        out
    }

    #[test]
    fn opens_two_connections_with_libomtnet_sequences() {
        let (addr, h) = capture_connections(2);
        let cfg = ReceiverConfig {
            quality: Quality::High,
            tally: Tally {
                preview: false,
                program: true,
            },
            ..ReceiverConfig::default()
        };
        let r = Receiver::connect(addr, cfg).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        drop(r);
        let got = h.join().unwrap();
        assert_eq!(
            commands_in(&got[0]),
            [
                Command::SubscribeMetadata,
                Command::SubscribeVideo,
                Command::Quality(Quality::High),
                Command::Tally(Tally {
                    preview: false,
                    program: true
                }),
            ]
        );
        assert_eq!(commands_in(&got[1]), [Command::SubscribeAudio]);
    }

    #[test]
    fn audio_only_subscribes_metadata_on_the_audio_connection() {
        let (addr, h) = capture_connections(1);
        let cfg = ReceiverConfig {
            video: false,
            ..ReceiverConfig::default()
        };
        drop(Receiver::connect(addr, cfg).unwrap());
        let got = h.join().unwrap();
        assert_eq!(
            commands_in(&got[0]),
            [Command::SubscribeMetadata, Command::SubscribeAudio]
        );
    }
}
