# open-media-transport

A pure-Rust implementation of the
[Open Media Transport](https://www.openmediatransport.org/) (OMT) protocol:
live video, audio and metadata over a local network, the job NDI does, under
the MIT license. Video uses the [`vmx-codec`](../vmx-codec) crate.

**Early work.** What exists today is the wire format only:

- `frame` — the 16-byte frame header and the video/audio extended headers
- `command` — the fixed protocol commands, byte for byte, and a classifier for
  incoming metadata
- `Deframer` — splits a TCP byte stream into frames, with explicit size limits;
  a malformed peer produces an error instead of a stalled connection

No networking, no discovery, and **no testing against another implementation
yet**. Nothing here claims to interoperate with vMix, OBS or libomtnet until
`docs/INTEROP.md` says so.

The protocol is implemented from [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md),
which describes what the reference implementation,
[libomtnet](https://github.com/openmediatransport/libomtnet), actually does,
with a source citation for every statement.

Licensed under either of Apache License 2.0 or MIT, at your option.
Protocol details derive from libomtnet (MIT, Copyright (c) 2025 Open Media
Transport Contributors); see `NOTICE`.
