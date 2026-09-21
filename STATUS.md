# STATUS — Open Media Transport in Rust

**Last updated:** 21 Sep 2026 — spec written; crate started with the wire format.

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
| `open-media-transport` | The protocol: discovery, sending, receiving, implemented from `docs/PROTOCOL.md` | **Wire format only**: frame headers, commands, a size-limited deframer; 18 tests, all self-consistency. No networking yet. |

**What is NOT true yet, and must not be claimed:** nothing has ever talked to a real
OMT device or application (not vMix, not OBS, not the Raspberry Pi encoder). Matching
the reference implementation's bytes proves the codec; it is not a live handshake.
`vmx-codec` has no SIMD, so it encodes 2.2x-3.2x slower than the C reference
(`crates/vmx-codec/BENCH.md`) — one core still does 1080p60 at OMT's default quality.
Caudal does not speak OMT; that integration is Caudal's M12 and comes after this.

**Stages 0 and 1 are written.**
- `docs/UPSTREAM.md` — inventory of libomtnet at `029ef4e` (v1.0.0.19) and every other upstream repo.
- `docs/PROTOCOL.md` — the wire protocol as libomtnet's code implements it, every claim
  cited to `file:line`. Its "Live" column is still empty: nothing is confirmed by capture.
  It records where upstream's own `PROTOCOL.md` disagrees with the code (commands carry
  no NUL; receivers open two TCP connections; `IPAddress` not `Address`; no TXT data).
- `docs/COMPARISON.md` — decision (21 Sep 2026): **we build our own crate** and use the
  community crate (MikanseiLaboratory/openmediatransport-rs) and `libomt-rs` only as
  cross-checks. Their crate builds and its 100 self-tests pass, but it has no interop
  evidence, and on macOS it announces itself as `LOCALHOST` (shown with `dns-sd`,
  `docs/evidence/2026-09-21-community-crate-mdns/`).

**Blocked on a tool:** confirming the spec needs libomtnet running here, which needs the
.NET SDK (not installed). Alternatives: vMix/SIENNA free OMT tools, OBS + plugin, a Pi.

**Next step:** discovery (stage 2): announce and browse `_omt._tcp` with a maintained
mDNS crate (check `mdns-sd` first), using the real OS host name (`PROTOCOL.md` D3), with
`dns-sd`/`tshark` output kept under `docs/evidence/`. The wire format in
`crates/open-media-transport` still needs confirming against a real sender.

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
