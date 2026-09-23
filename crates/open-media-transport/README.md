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
- `sender` — listens, answers receivers' commands, encodes VMX1 with `vmx-codec`
- `discovery` — announce and browse `_omt._tcp` over mDNS, with libomtnet's naming
- `discovery_server` — the discovery server and its client, for networks without
  multicast; `Discovery::with_server` uses one instead of mDNS for announcing

Receiver, sender and discovery have each been run against libomtnet, with the
same decoded pixels on both sides; see `docs/INTEROP.md` for exactly what was
tested. Nothing has been tested against vMix or OBS yet.

The protocol is implemented from [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md),
which describes what the reference implementation,
[libomtnet](https://github.com/openmediatransport/libomtnet), actually does,
with a source citation for every statement.

Licensed under either of Apache License 2.0 or MIT, at your option.
Protocol details derive from libomtnet (MIT, Copyright (c) 2025 Open Media
Transport Contributors); see `NOTICE`.

## Fuzzing

Two libFuzzer targets live in `fuzz/` (nightly and `cargo-fuzz` needed):

```sh
cargo +nightly fuzz run --target aarch64-apple-darwin deframer -- -max_total_time=60
cargo +nightly fuzz run --target aarch64-apple-darwin metadata -- -max_total_time=60
```

`deframer` feeds arbitrary bytes in arbitrary chunks and checks every returned
frame is self-consistent; `metadata` checks command recognition is exact. On
21 Sep 2026 they ran 10.0 M and 15.3 M executions without a failure, seeded with
bytes captured from libomtnet. A smaller deterministic version runs in
`cargo test` (`deframer::tests::corrupted_streams_never_panic`).
