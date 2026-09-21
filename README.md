# Open Media Transport for Rust

A pure-Rust implementation of [Open Media Transport](https://www.openmediatransport.org/)
(OMT). OMT is an open, MIT-licensed protocol from the vMix team for sending
live video over a local network with low latency, the same job NDI does.

| Crate | What it is | Status |
|---|---|---|
| [`vmx-codec`](crates/vmx-codec) | VMX, the OMT video codec. A safe-Rust port of [libvmx](https://github.com/openmediatransport/libvmx). | Codec core ported. Output is byte-identical to libvmx. |
| [`open-media-transport`](crates/open-media-transport) | Protocol: discovery, sending and receiving, implemented from [`docs/PROTOCOL.md`](docs/PROTOCOL.md), a cited description of [libomtnet](https://github.com/openmediatransport/libomtnet). | Receiver, sender and discovery, working both ways against libomtnet on one Mac ([`docs/INTEROP.md`](docs/INTEROP.md)). Not yet tested with vMix or OBS. |
| [`omt-cli`](crates/omt-cli) | The `omt` command: list, send a test pattern, receive and report. | For testers — see [`TESTING.md`](TESTING.md). |
| `libvmx-ref` (unpublished) | Builds the upstream C++ libvmx, used only by the conformance tests and the benchmark. | Test-only. |

## Try it

```sh
cargo build --release -p omt-cli
./target/release/omt list                     # OMT sources on your network
./target/release/omt send --name "Rust Test"  # a test pattern other OMT software can receive
./target/release/omt recv "MY-PC (Camera 1)" --snapshot shot.bmp
```

[`TESTING.md`](TESTING.md) explains how to test against vMix, OBS and other OMT products,
and what to report. Only libomtnet on one Mac has been tested so far
([`docs/INTEROP.md`](docs/INTEROP.md)).

## Development

```sh
# optional, for conformance tests and the C benchmark (gitignored):
git clone --depth 1 https://github.com/openmediatransport/libvmx reference/libvmx

cargo test -p vmx-codec
cargo test -p open-media-transport
cargo clippy --all-targets -- -D warnings
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Copyright (c) 2026 Saul González
and Puny.bz Inc. Ported code keeps the MIT notice of its upstream: see
[`NOTICE`](NOTICE).

This project is not affiliated with or endorsed by the Open Media Transport
project or vMix.
