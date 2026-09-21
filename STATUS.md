# STATUS — Open Media Transport in Rust

**Last updated:** 21 Sep 2026 — receiver, sender and discovery all work against libomtnet (one Mac).

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
| `open-media-transport` | The protocol: discovery, sending, receiving, implemented from `docs/PROTOCOL.md` | **Receiver, sender, discovery.** Against libomtnet 1.0.0.19 on one Mac: we receive what it sends and it receives what we send, with identical decoded pixels both ways, each side finding the other by name (`docs/INTEROP.md`). Receiver reconnects. Deframer and command matching fuzzed (25 M runs, no failure). `Clock` generates and paces timestamps like libomtnet. No redirect or discovery server yet. 40 tests. |

**What is NOT true yet, and must not be claimed:** nothing has talked to vMix, OBS or
the Raspberry Pi devices. Our receiver, sender and discovery have talked to libomtnet itself,
on one Mac (`docs/INTEROP.md`) — that is the whole of our interop evidence. Matching
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

**Verification tools now work here** (21 Sep 2026): .NET SDK 10.0.401 builds libomtnet
through `interop/libomtnet-harness`; `tshark` can capture (wireshark-chmodbpf installed);
`dns-sd` shows mDNS. First evidence: `docs/evidence/2026-09-21-libomtnet-loopback/` —
libomtnet talking to itself confirmed 21 rows of `PROTOCOL.md` (marked [L]), including
commands without NUL, two TCP connections per receiver, and the exact `OMTInfo` bytes.
This is libomtnet against itself: **still nothing about vMix, OBS or a Pi.**

Rebuild recipe (outputs to a scratch dir, never into the repo):
`g++ -O3 -std=c++17 -fdeclspec -fPIC -Wno-c++11-narrowing -dynamiclib reference/libvmx/src/vmxcodec_arm.cpp reference/libvmx/src/vmxcodec.cpp -o $OUT/libvmx.dylib`
then `dotnet build interop/libomtnet-harness -c Release -p:LibVmx=$OUT/libvmx.dylib -o $OUT/harness`.
Use `tshark ... -a duration:N` — it ignores SIGALRM. After any `dotnet build`, run
`dotnet build-server shutdown`: the compiler server otherwise stays resident.

**Discovery decision (21 Sep 2026):** `mdns-sd` with the SRV target `<host>-omt.local.`,
not the OS host name — naming the OS host made `mdns-sd` conflict with macOS's responder
and rename itself (`docs/evidence/2026-09-21-our-discovery`). Loopback interfaces are
excluded by kind and by name (`lo0`, `lo`).

**Next step:** evidence beyond libomtnet-on-one-Mac, which is now the limiting factor:
1. A second machine on the LAN (another Mac, or a Linux box with Avahi), both directions.
2. A real product: OBS with the OMT plugin, SIENNA's macOS OMT tools, or vMix's free tools
   on a Windows PC. Needs the user to install or provide them.
Code gaps meanwhile: re-resolving a source by name after it moves port; redirect (§9);
the discovery server (§10); a public receive API that decodes (today the examples decode).

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
