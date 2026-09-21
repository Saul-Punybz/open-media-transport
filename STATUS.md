# STATUS — Open Media Transport in Rust

**Last updated:** 21 Sep 2026 — stage 0 done (`docs/UPSTREAM.md`).

## RESUME HERE

**What this repo is.** A pure-Rust implementation of [Open Media Transport](https://www.openmediatransport.org/),
the vMix team's MIT-licensed alternative to NDI for moving live video over a LAN.
Public repo `Saul-Punybz/open-media-transport`, licensed MIT OR Apache-2.0.
**All repo text is English.**

**Where we are.**

| Crate | What it is | State |
|---|---|---|
| `vmx-codec` | The OMT video codec, a safe-Rust port of `libvmx` | **Done.** Byte-identical to the C++ reference both ways, at any thread count. Conformance tests build upstream at `544bcfb`. |
| `libvmx-ref` | Builds the upstream C++ reference | Test-only. |
| `open-media-transport` | The protocol: discovery, sending, receiving. A port of `libomtnet` (C#, ~10.6K lines) | **Not started — this is the work.** |

**What is NOT true yet, and must not be claimed:** nothing has ever talked to a real
OMT device or application (not vMix, not OBS, not the Raspberry Pi encoder). Matching
the reference implementation's bytes proves the codec; it is not a live handshake.
`vmx-codec` has no SIMD, so it encodes 2.2x-3.2x slower than the C reference
(`crates/vmx-codec/BENCH.md`) — one core still does 1080p60 at OMT's default quality.
Caudal does not speak OMT; that integration is Caudal's M12 and comes after this.

**Stage 0 is done:** `docs/UPSTREAM.md` inventories `libomtnet` at `029ef4e` (v1.0.0.19),
cloned in `reference/libomtnet`, plus every other upstream repo. Two findings change the picture:

1. Upstream has exactly one protocol implementation, `libomtnet`. The OBS plugin and both
   Raspberry Pi devices are built on it, so they are not independent interop evidence;
   vMix might be, but it is closed and its implementation is unknown.
2. **A pure-Rust OMT crate already exists**: MikanseiLaboratory/openmediatransport-rs
   (MIT, ~9K lines, listed in upstream's `DOWNLOADS.md`, not on crates.io, unverified).
   Whether to build on it, contribute to it, use it as a cross-check or ignore it is
   an open decision for the maintainer — settle it before stage 1.

**Next step:** decide on the community crate, then `docs/PROTOCOL_PLAN.md` stage 1
(`docs/PROTOCOL.md`, every claim citing upstream source `file:line`). Still no protocol code.

## House rules

- **Never invent protocol details.** Every statement about the wire format cites
  upstream `file:line`. If upstream is ambiguous, say so in the spec and test it
  against a real implementation rather than guessing.
- **Verify with tools outside Claude**: `tshark` captures, real OMT applications,
  the upstream C# implementation itself. Anything not externally verified is
  reported as not verified.
- **Reuse before writing**: check crates.io and GitHub first (mDNS, framing,
  async IO). Only write what is genuinely missing, and verify what you take.
- **Licensing**: MIT OR Apache-2.0. Ported code keeps the MIT notices of
  `libomtnet` and `libvmx` and credits their authors — see `NOTICE`.
  Never use OMT's or vMix's logos as branding; the crates may say what they implement.
- **This laptop overheats.** One heavy job at a time, `CARGO_BUILD_JOBS=2`,
  `nice -n 10`, never leave a process running. No endless loops, no full-workspace
  builds while something else compiles.
- **Commits**: clean messages, no `Co-Authored-By` and no `Claude-Session` lines.
- **Save often**: update this file and push at the end of each batch of work,
  not at the end of the project.
