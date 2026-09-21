# open-media-transport

A pure-Rust implementation of the
[Open Media Transport](https://www.openmediatransport.org/) (OMT) protocol:
live video, audio and metadata over a local network, the job NDI does, under
the MIT license. Video uses the [`vmx-codec`](../vmx-codec) crate.

**Early work.** What exists today:

- `frame` — the 16-byte frame header and the video/audio extended headers
- `command` — the fixed protocol commands, byte for byte, and a classifier for
  incoming metadata
- `Deframer` — splits a TCP byte stream into frames, with explicit size limits;
  a malformed peer produces an error instead of a stalled connection
- `receiver` — a minimal blocking receiver that connects the way libomtnet does
- `discovery` — announce and browse `_omt._tcp` over mDNS, with libomtnet's naming

The receiver has been run against a libomtnet sender and decoded the same pixels
as libomtnet's own receiver; see `docs/INTEROP.md` for exactly what was tested.
Discovery works both ways against libomtnet. No sender yet, and nothing tested
against vMix or OBS.

The protocol is implemented from [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md),
which describes what the reference implementation,
[libomtnet](https://github.com/openmediatransport/libomtnet), actually does,
with a source citation for every statement.

Licensed under either of Apache License 2.0 or MIT, at your option.
Protocol details derive from libomtnet (MIT, Copyright (c) 2025 Open Media
Transport Contributors); see `NOTICE`.
