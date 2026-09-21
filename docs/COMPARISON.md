# Comparison with existing implementations

Our crate is being built from scratch against [`PROTOCOL.md`](PROTOCOL.md). The other
implementations are **cross-checks**, never sources: nothing is copied from them, and
where they disagree with libomtnet's code, libomtnet's code wins until a capture says
otherwise.

21 Sep 2026.

## The implementations

| | libomtnet (upstream) | openmediatransport-rs (community) | libomt-rs | this repo |
|---|---|---|---|---|
| What | reference C# | pure Rust | Rust bindings to native `libomt` (= libomtnet, NativeAOT) | pure Rust |
| Version checked | `029ef4e`, v1.0.0.19 | `7711da4`, 0.1.0 | crates.io 0.2.0 (not built) | — |
| License | MIT | MIT | see its repo | MIT OR Apache-2.0 |
| Codec | libvmx (C++) | `vmx-rs` (git dep, SIMD) | libvmx via libomt | `vmx-codec`, byte-identical to libvmx both ways, tested |
| On crates.io | n/a | no | yes | not yet |
| Role here | the spec's source | cross-check | possible oracle (drives real upstream code from Rust) | the product |

## The community crate, checked on this machine

Built from a copy at `7711da4`, `CARGO_BUILD_JOBS=2`, `nice -n 10`, rustc 1.98.1:

- `cargo build --all-targets --locked`: **builds, no warnings**, 24 s.
- `cargo test` on 9 of its 11 test targets (lib + `discovery_server`, `fpa1`,
  `loopback`, `metadata`, `protocol_roundtrip`, `session_ops`, `settings`,
  `vmx_simd`): **100 tests, all pass**. `soak_loopback` and `av_stress` were
  skipped to keep the laptop cool.
- Every test pits the crate against itself. None runs against libomtnet or any
  other implementation, so passing says nothing about interop.
- Its `fuzz/` directory is a placeholder README; there are no fuzz targets.
- It cannot be published to crates.io as is: `vmx` is a git dependency.

## Where it differs from libomtnet's code

Each row cites our spec. "Verified" means observed with a tool outside Claude.

| Topic | libomtnet code | community crate | Effect | Verified |
|---|---|---|---|---|
| Machine name in source names (D3) | `gethostname()` upper-cased | env `COMPUTERNAME` / `HOSTNAME`, else `localhost` | On macOS it announces `LOCALHOST (Name)` with SRV host `LOCALHOST.local`, and answers A/AAAA for that name with loopback addresses too. Two Macs would collide. | **yes** — [`evidence/2026-09-21-community-crate-mdns`](evidence/2026-09-21-community-crate-mdns/README.md) |
| Local address | all interfaces via platform DNS-SD | one IP found by "connecting" a UDP socket to 8.8.8.8, plus 127.0.0.1 | multi-homed hosts advertise one interface | no (read in code) |
| Unknown header version (R1) | stalls | returns a protocol error | theirs is more robust; not a compatibility problem | no |
| Preview + per-frame metadata (U2) | metadata bytes replaced by VMX bytes (suspected bug) | appends the real metadata after the preview prefix | theirs is arguably correct; bytes differ from upstream | no |
| Opaque BGRA (V4) | `VMX_EncodeBGRA` | `encode_bgrx` | different bitstream; should decode the same | no |
| Preview off | sends nothing unless preview is wanted | always sends `<OMTSettings Preview="false" />` | harmless if upstream behaves as read | no |
| Commands without NUL (M2) | no NUL | no NUL | agree | no — both are readings of the same code |
| Two connections per receiver (T5) | yes | yes (AV stream + separate audio stream) | agree | no |

## What would make this crate worth having next to theirs

Ranked by how much each strengthens trust in the result, which is the one thing
nobody has yet: neither crate has ever been shown to talk to libomtnet, vMix or OBS.

1. **Interop evidence, kept in the repo.** `tshark` captures and a written matrix
   (`INTEROP.md`, stage 5) against libomtnet itself, OBS with the OMT plugin, vMix's
   free OMT tools, SIENNA's macOS tools, and a Pi. This is the single largest gap in
   the community crate and the thing the README can point to.
2. **A conformance harness against real upstream code.** The same idea that made
   `vmx-codec` trustworthy: build libomtnet (needs the .NET SDK) or drive `libomt`
   through `libomt-rs`, send and receive through it, and diff bytes against ours in
   CI. Stage 1's open questions (M2/M3, T5, exact XML bytes, U2) become tests.
3. **A spec with citations**, which exists now: [`PROTOCOL.md`](PROTOCOL.md). It
   also records where upstream's own `PROTOCOL.md` is wrong (NUL, `IPAddress`,
   TXT, two connections) — worth offering upstream as a documentation fix.
4. **Correct identity on the network.** Real host names from the OS, all interfaces,
   and no loopback addresses advertised to the LAN — the defect shown above.
5. **Fuzzed parsers.** Real `cargo fuzz` targets for the frame parser, the command
   matcher and discovery-server XML, since every byte arrives from the network.
6. **Publishable.** Codec and protocol as crates.io packages with no git
   dependencies, dual MIT/Apache licence, `#![forbid(unsafe_code)]` in the protocol
   crate as in `vmx-codec`.
7. **Deliberate choices on upstream bugs.** For each item in `PROTOCOL.md` §12,
   decide "match upstream bytes" or "fix", make it a documented option where it
   matters, and test both.
8. **Bounded memory.** Explicit caps (10 MiB video, 1 MiB audio, 64 KiB metadata,
   queue depths) enforced by the parser instead of by buffer size, so a hostile or
   broken peer is dropped rather than stalling the connection (R1, R3).

Not a differentiator: SIMD speed (theirs has it; ours is enough for 1080p60 on one
core, `crates/vmx-codec/BENCH.md`), GPU paths, async wrappers. Those can come later
and are not what decides whether someone trusts the crate on a live show.

## Could the community crate be useful to us?

As a second opinion, yes: when our reading of libomtnet and theirs agree, that is
mild comfort; when they disagree, it points at a line worth a capture. As a test peer,
also yes, once both exist: our sender to their receiver and back is cheap, even if it
is not proof of anything upstream does. Bugs found in it (like the host name) are
worth reporting to its maintainer.
