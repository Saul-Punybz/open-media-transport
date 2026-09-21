//! Metadata classification: never panics, and anything recognised as a
//! command is exactly that command's bytes (M3).
#![no_main]

use libfuzzer_sys::fuzz_target;
use open_media_transport::command::{classify, Command, Message};

fuzz_target!(|data: &[u8]| {
    if let Some(c) = Command::recognize(data) {
        assert_eq!(c.as_bytes(), data);
        assert_eq!(classify(data), Message::Command(c));
    } else {
        assert!(!matches!(classify(data), Message::Command(_)));
    }
});
