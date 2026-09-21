//! Protocol commands: fixed metadata strings that control a connection
//! (`docs/PROTOCOL.md` §3.5 and §4).
//!
//! libomtnet sends these without a terminating NUL (M2) and recognises them
//! by exact byte equality of the whole payload (M3). The tally strings contain
//! `Program==`, two equals signs, which is not well-formed XML and must be
//! reproduced exactly (`OMTMetadata.cs:44-48`).

const SUBSCRIBE_VIDEO: &str = r#"<OMTSubscribe Video="true" />"#;
const SUBSCRIBE_AUDIO: &str = r#"<OMTSubscribe Audio="true" />"#;
const SUBSCRIBE_METADATA: &str = r#"<OMTSubscribe Metadata="true" />"#;
const PREVIEW_ON: &str = r#"<OMTSettings Preview="true" />"#;
const PREVIEW_OFF: &str = r#"<OMTSettings Preview="false" />"#;
const TALLY_NONE: &str = r#"<OMTTally Preview="false" Program=="false" />"#;
const TALLY_PREVIEW: &str = r#"<OMTTally Preview="true" Program=="false" />"#;
const TALLY_PROGRAM: &str = r#"<OMTTally Preview="false" Program=="true" />"#;
const TALLY_BOTH: &str = r#"<OMTTally Preview="true" Program=="true" />"#;
const QUALITY_DEFAULT: &str = r#"<OMTSettings Quality="Default" />"#;
const QUALITY_LOW: &str = r#"<OMTSettings Quality="Low" />"#;
const QUALITY_MEDIUM: &str = r#"<OMTSettings Quality="Medium" />"#;
const QUALITY_HIGH: &str = r#"<OMTSettings Quality="High" />"#;

const QUALITY_PREFIX: &[u8] = b"<OMTSettings Quality=";
const SENDER_INFO_PREFIX: &[u8] = b"<OMTInfo";
const REDIRECT_PREFIX: &[u8] = b"<OMTRedirect";

/// Tally state: whether a source is on preview and/or program.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Tally {
    /// On preview.
    pub preview: bool,
    /// On program (on air).
    pub program: bool,
}

impl Tally {
    /// Combines tallies from several receivers, as a sender does (OR).
    pub fn union(self, other: Tally) -> Tally {
        Tally {
            preview: self.preview || other.preview,
            program: self.program || other.program,
        }
    }
}

/// Encoder quality a receiver suggests (§4.1). The sender uses the highest
/// suggestion among its video connections when its own quality is `Default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Quality {
    /// Defer to other receivers.
    #[default]
    Default,
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
}

/// A command with a fixed wire form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// Receiver asks for video on this connection.
    SubscribeVideo,
    /// Receiver asks for audio on this connection.
    SubscribeAudio,
    /// Receiver asks for metadata broadcasts on this connection.
    SubscribeMetadata,
    /// Receiver switches this connection to or from 1/8 preview video.
    Preview(bool),
    /// Receiver reports its tally; sender reports the combined tally.
    Tally(Tally),
    /// Receiver suggests an encoder quality.
    Quality(Quality),
}

impl Command {
    /// The exact bytes to send, without a NUL.
    pub fn as_bytes(self) -> &'static [u8] {
        let s = match self {
            Command::SubscribeVideo => SUBSCRIBE_VIDEO,
            Command::SubscribeAudio => SUBSCRIBE_AUDIO,
            Command::SubscribeMetadata => SUBSCRIBE_METADATA,
            Command::Preview(true) => PREVIEW_ON,
            Command::Preview(false) => PREVIEW_OFF,
            Command::Tally(Tally {
                preview: false,
                program: false,
            }) => TALLY_NONE,
            Command::Tally(Tally {
                preview: true,
                program: false,
            }) => TALLY_PREVIEW,
            Command::Tally(Tally {
                preview: false,
                program: true,
            }) => TALLY_PROGRAM,
            Command::Tally(Tally {
                preview: true,
                program: true,
            }) => TALLY_BOTH,
            Command::Quality(Quality::Default) => QUALITY_DEFAULT,
            Command::Quality(Quality::Low) => QUALITY_LOW,
            Command::Quality(Quality::Medium) => QUALITY_MEDIUM,
            Command::Quality(Quality::High) => QUALITY_HIGH,
        };
        s.as_bytes()
    }

    /// Recognises a metadata payload that is exactly one of the fixed
    /// strings. A trailing NUL or any other difference means it is not (M3).
    pub fn recognize(payload: &[u8]) -> Option<Command> {
        ALL.iter().copied().find(|c| c.as_bytes() == payload)
    }
}

const ALL: [Command; 13] = [
    Command::SubscribeVideo,
    Command::SubscribeAudio,
    Command::SubscribeMetadata,
    Command::Preview(true),
    Command::Preview(false),
    Command::Tally(Tally {
        preview: false,
        program: false,
    }),
    Command::Tally(Tally {
        preview: true,
        program: false,
    }),
    Command::Tally(Tally {
        preview: false,
        program: true,
    }),
    Command::Tally(Tally {
        preview: true,
        program: true,
    }),
    Command::Quality(Quality::Default),
    Command::Quality(Quality::Low),
    Command::Quality(Quality::Medium),
    Command::Quality(Quality::High),
];

/// What an incoming metadata payload is, in the order libomtnet checks
/// (`OMTChannel.cs:322-411`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message<'a> {
    /// A fixed command. Consumed by the connection.
    Command(Command),
    /// Starts with `<OMTSettings Quality=` but is not one of the four exact
    /// forms. libomtnet parses it as XML and keeps the previous suggestion if
    /// the value is unknown; it is consumed either way. Parsing it needs an
    /// XML parser, which this crate does not have yet.
    QualityOther(&'a [u8]),
    /// `<OMTInfo …/>` sender information. libomtnet parses it **and** passes
    /// it on to the application.
    SenderInfo(&'a [u8]),
    /// `<OMTRedirect NewAddress="…"/>` (§9). Consumed by the connection.
    Redirect(&'a [u8]),
    /// Anything else: application metadata.
    Application(&'a [u8]),
}

/// Classifies a metadata frame's payload.
pub fn classify(payload: &[u8]) -> Message<'_> {
    if let Some(c) = Command::recognize(payload) {
        Message::Command(c)
    } else if payload.starts_with(QUALITY_PREFIX) {
        Message::QualityOther(payload)
    } else if payload.starts_with(SENDER_INFO_PREFIX) {
        Message::SenderInfo(payload)
    } else if payload.starts_with(REDIRECT_PREFIX) {
        Message::Redirect(payload)
    } else {
        Message::Application(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_match_libomtnet_exactly() {
        // OMTMetadata.cs:38-48 and :53, with C# "" unescaped to ".
        assert_eq!(
            Command::SubscribeVideo.as_bytes(),
            br#"<OMTSubscribe Video="true" />"#
        );
        assert_eq!(
            Command::SubscribeAudio.as_bytes(),
            br#"<OMTSubscribe Audio="true" />"#
        );
        assert_eq!(
            Command::SubscribeMetadata.as_bytes(),
            br#"<OMTSubscribe Metadata="true" />"#
        );
        assert_eq!(
            Command::Preview(true).as_bytes(),
            br#"<OMTSettings Preview="true" />"#
        );
        assert_eq!(
            Command::Preview(false).as_bytes(),
            br#"<OMTSettings Preview="false" />"#
        );
        assert_eq!(
            Command::Tally(Tally {
                preview: true,
                program: false
            })
            .as_bytes(),
            br#"<OMTTally Preview="true" Program=="false" />"#
        );
        assert_eq!(
            Command::Tally(Tally {
                preview: false,
                program: true
            })
            .as_bytes(),
            br#"<OMTTally Preview="false" Program=="true" />"#
        );
        assert_eq!(
            Command::Tally(Tally {
                preview: true,
                program: true
            })
            .as_bytes(),
            br#"<OMTTally Preview="true" Program=="true" />"#
        );
        assert_eq!(
            Command::Tally(Tally::default()).as_bytes(),
            br#"<OMTTally Preview="false" Program=="false" />"#
        );
        assert_eq!(
            Command::Quality(Quality::Default).as_bytes(),
            br#"<OMTSettings Quality="Default" />"#
        );
        // OMTReceive.cs:1054 builds the others by replacing "Default" with the enum name.
        assert_eq!(
            Command::Quality(Quality::Medium).as_bytes(),
            br#"<OMTSettings Quality="Medium" />"#
        );
    }

    #[test]
    fn no_command_carries_a_nul() {
        for c in ALL {
            assert!(!c.as_bytes().contains(&0), "{c:?}");
        }
    }

    #[test]
    fn every_command_round_trips() {
        for c in ALL {
            assert_eq!(Command::recognize(c.as_bytes()), Some(c));
            assert_eq!(classify(c.as_bytes()), Message::Command(c));
        }
    }

    #[test]
    fn near_misses_are_not_commands() {
        // M3: a trailing NUL, whitespace, or well-formed tally XML is not a command.
        let mut with_nul = Command::SubscribeVideo.as_bytes().to_vec();
        with_nul.push(0);
        assert_eq!(Command::recognize(&with_nul), None);
        assert_eq!(Command::recognize(br#"<OMTSubscribe Video="true"/>"#), None);
        assert_eq!(
            Command::recognize(br#"<OMTTally Preview="true" Program="false" />"#),
            None
        );
        assert_eq!(classify(&with_nul), Message::Application(&with_nul));
    }

    #[test]
    fn prefix_messages() {
        let q = br#"<OMTSettings Quality='High'/>"#;
        assert_eq!(classify(q), Message::QualityOther(q));
        let info = br#"<OMTInfo ProductName="X" Manufacturer="Y" Version="1" />"#;
        assert_eq!(classify(info), Message::SenderInfo(info));
        let r = br#"<OMTRedirect NewAddress="HOST (Cam)" />"#;
        assert_eq!(classify(r), Message::Redirect(r));
        assert_eq!(
            classify(b"<OMTWeb URL=\"http://x/\" />\0"),
            Message::Application(b"<OMTWeb URL=\"http://x/\" />\0")
        );
    }

    #[test]
    fn tally_union() {
        let a = Tally {
            preview: true,
            program: false,
        };
        let b = Tally {
            preview: false,
            program: true,
        };
        assert_eq!(
            a.union(b),
            Tally {
                preview: true,
                program: true
            }
        );
    }
}
