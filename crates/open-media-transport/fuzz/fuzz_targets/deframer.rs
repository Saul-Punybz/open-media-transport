//! Arbitrary bytes, split into arbitrary chunks, through the deframer.
//! The first byte picks the chunk size, so splits are explored too.
#![no_main]

use libfuzzer_sys::fuzz_target;
use open_media_transport::frame::{ExtendedHeader, FrameType, HEADER_LEN};
use open_media_transport::{Deframer, Limits};

fuzz_target!(|data: &[u8]| {
    let Some((&chunk, stream)) = data.split_first() else { return };
    let chunk = chunk as usize % 64 + 1;
    let mut d = Deframer::new(Limits { max_frame_len: 1 << 16 });
    let mut consumed = 0;
    for piece in stream.chunks(chunk) {
        d.push(piece);
        loop {
            match d.next_frame() {
                Ok(Some(f)) => {
                    let ext_len = f.header.frame_type.extended_header_len();
                    let total = HEADER_LEN + ext_len + f.data.len() + f.metadata.len();
                    assert_eq!(total, f.header.frame_len().unwrap());
                    assert_eq!(f.metadata.len(), f.header.metadata_length as usize);
                    assert_eq!(
                        matches!(f.ext, ExtendedHeader::None),
                        f.header.frame_type == FrameType::Metadata
                    );
                    consumed += total;
                }
                Ok(None) => break,
                Err(_) => return, // stream rejected; nothing more to check
            }
        }
    }
    assert_eq!(consumed + d.buffered(), stream.len());
});
