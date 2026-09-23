# STATUS — Open Media Transport in Rust

**Last updated:** 23 Sep 2026 (evening) — pre-testing-week batch in flight (see "In flight" below). Earlier today: addressing (connect by name/URL, re-resolve, redirect), decoding receive API + 10-bit snapshots, and the discovery server are merged and verified against libomtnet; SIMD for vmx-codec and the Caudal-M12 prerequisites merged too.

## RESUME HERE

**What this repo is.** A pure-Rust implementation of [Open Media Transport](https://www.openmediatransport.org/),
the vMix team's MIT-licensed alternative to NDI for moving live video over a LAN.
Public repo `Saul-Punybz/open-media-transport`, licensed MIT OR Apache-2.0.
**All repo text is English.**

**Where we are.**

| Crate | What it is | State |
|---|---|---|
| `vmx-codec` | The OMT video codec, a safe-Rust port of `libvmx` | **Done.** Byte-identical to the C++ reference both ways, at any thread count. Conformance tests build upstream at `544bcfb`. |
| `omt-cli` | The `omt` tool: `list`, `send` (test pattern), `recv` (stats + BMP snapshot) | Works against libomtnet here; for testers (`TESTING.md`). |
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

**Readiness (21 Sep 2026):** ready for *technical* testers who can build from source or run a
downloaded binary from a terminal (`TESTING.md`). Not ready for end users: no GUI, unsigned
binaries, only libomtnet-on-one-Mac verified. CI (`.github/workflows/ci.yml`) builds and
tests on macOS, Linux and Windows — green since `a6a23b9` (21 Sep 2026). That proves the
code builds and its unit/loopback tests pass there, not that it interoperates there.

**v0.1.0 tagged (21 Sep 2026)** with the maintainer's go-ahead. `release.yml` built `omt` for
aarch64/x86_64 macOS, x86_64 Linux and x86_64 Windows into a **draft** release. The macOS
arm64 archive, downloaded from that draft, was run against libomtnet both ways and works;
the other three archives have not been run. **Published 22 Sep 2026 as a pre-release**
(maintainer's request): https://github.com/Saul-Punybz/open-media-transport/releases/tag/v0.1.0 —
confirmed downloadable without authentication; archive checksums in the release notes and in
the local `dist/v0.1.0/SHA256SUMS`. `release.yml` builds `omt` for four targets into a
**draft** GitHub release when a `v*` tag is pushed.


**23 Sep 2026 batch (merged to main, each verified against libomtnet on this Mac; evidence in `docs/evidence/2026-09-23-*`):**
- **Discovery server (§10)** — `discovery_server` module (client + server), `Discovery::with_server`, `omt discovery-server`, `--discovery-server`/`--no-mdns`. Our client with upstream's OMTDiscoveryServer and our server with libomtnet clients, both ways; client bytes identical to libomtnet's. S1–S6 Live.
- **Addressing (§8, §9)** — `address` module (`Address`, `Directory`), `Receiver::connect_to` by full name or `omt://` URL, re-resolve on every reconnect (shown: libomtnet sender restarted on another port, receiver came back); redirect on both sides (`Sender::set_redirect`, `Event::Redirect`), redirect bytes identical to libomtnet's. Found a libomtnet bug: after a cleared redirect, a late-joining libomtnet receiver never follows again. Chains (X3) and X4 only tested between our own senders.
- **Decoding receive API** — `media` module (`MediaDecoder`, libomtnet's preferred-format rules): UYVY, UYVA, BGRA/BGRX, P216, PA16, previews, f32 planar audio — byte-identical to libomtnet's own decoder in 23 live cases. `omt send --10bit` (P216) and 16-bit PNG snapshots (`omt recv --snapshot x.png`).
- **vmx-codec SIMD** — NEON (aarch64) and SSE2 (x86_64) kernels in `src/simd.rs`, the only module allowed `unsafe` (crate went `forbid` → `deny`; safe routes compiled to scalar and were slower, see BENCH.md). 1080p one thread on the M4: encode 111 → 341 fps at OMT q80 (libvmx 327–339), 45 → 137 at q98 (libvmx 142); decode 454 → ~1120 (libvmx 1252), 63 → 101 (libvmx 135). Still byte-identical to libvmx (conformance on aarch64, and x86_64 under Rosetta). No AVX2. First real x86 run is CI.
- **M12 prerequisites** — `Sender::send_encoded_video` (pre-encoded VMX1; a libomtnet receiver decoded it to identical pixels), `SenderConfig::encoder_threads`, `SendError` instead of panics (**breaking:** `send_video`/`send_audio` return `Result<usize, SendError>`), sender/peer/receiver stats, one shared `Discovery` with interface selection (`DiscoveryConfig`), bounded `Drop` (2 s), and a `vmx_decode` fuzz target. It found `preview_len` miscounting an extended header with DC shift 0 — fixed, regression test in `decoder.rs`.
Still only libomtnet on one Mac: **no vMix, OBS or Pi.**

**Goal for the week of 23 Sep 2026 (maintainer's request):** everything that does not need real
equipment is finished, so the maintainer's vMix / OBS / Raspberry Pi testing week is the only thing
left. Windows CI fixed and merged 23 Sep (`3f84881`, `src/net.rs`: on Windows `shutdown()` does not
wake a blocked read, so readers poll a stop flag every 100 ms; evidence `docs/evidence/2026-09-23-windows-ci`).

**In flight on 23 Sep (branches; worktrees under the session scratchpad — if lost, re-create from the
pushed branches):**
- `fix/library` — bug-hunt + security fixes and small features. Bugs (each with a failing test in
  the bug-hunt report): shared `Discovery` second browse deafens the first (mdns-sd overwrites the
  listener); sender `start_peer` race leaks peers/tally on fast close; duplicate name on a shared
  Discovery withdraws the other sender; discovery server writes under the table lock (one slow client
  stalls all); unbounded receiver queue; discovery client writes without timeout; `redirect::parse`
  and unparseable-redirect behaviour differ from libomtnet; redirect watchers start their own mDNS.
  Security (PoCs on localhost): receiver queue 15 MB → 3 GB in 1.2 s; redirect to any host incl. DNS
  (→ `RedirectPolicy`, default SameHost); discovery server hijack/flood/stall (→ only-remove-own,
  caps, per-peer outbox); sender 500 idle sockets → 1,002 threads (→ connection/per-IP caps,
  subscribe timeout, `bind`). Features: `settings.xml`, lenient tally parsing, D6 on Windows,
  `log` facade, metadata helpers (OMTWeb/OMTPTZ/AncillaryData/OMTGroup), more fuzz targets,
  `SECURITY.md` ("OMT is a trusted-LAN protocol").
- `feat/tester-kit` — `omt send` every format/size/fps/alpha/audio, `omt recv` with live tally/
  quality/preview, **`omt check`** (health of any OMT source: frames, fps, decode, freeze/black/
  silence, A/V offset; OK/WARN/FAIL + exit codes, `--watch`, `--json`), `omt report` bundle,
  `--verbose`, TESTING.md rewritten per product (step 0: `omt check --all`).
- `feat/release-ci` — Linux aarch64 (Pi) + static musl binaries, SHA256SUMS in the workflow, CI on
  `ubuntu-24.04-arm`, libvmx conformance actually running in CI (at `544bcfb` and upstream's new
  `a1828cb`), fuzz smoke, libomtnet harness interop in CI on Linux and Windows.
- `feat/interop-coverage` — harness sends/receives what OBS and the Pi tools use (NV12, YUY2, BGRA
  premultiplied, interlaced, BT.601, 1080p59.94, mono/8/16/32 ch, 44.1/96 kHz), fills PROTOCOL.md
  Live rows, X3/X4 against libomtnet.

**After those merge:** opt-in encrypted transport `omts://` (TLS 1.3 via rustls, separate port,
pinned certs, advertised in the unused mDNS TXT; plain OMT stays the default); version 0.2.0 +
CHANGELOG (breaking: `send_video`/`send_audio` return `Result<usize, SendError>`); refresh stale docs
(README says vmx-codec is safe-Rust; crate README module list; lib.rs Status; UPSTREAM "later";
INTEROP rows for 23 Sep); new release for the testers.

**Decided not to do (would break interop — upstream rejected them, libomtnet PRs #36-#40):** QUIC,
AV1/Opus, PTP, multicast. **After testing week, candidates:** C ABI compatible with `libomt.h`
(FFmpeg/Python/Unity), GStreamer `omtsrc`/`omtsink`, AVX2, `omt route` (router on redirect), a
.NET-free Pi encoder/player, frame sync in Caudal.

**Top risks for the testing week:** our SRV target `<host>-omt.local.` (libomtnet uses the OS host
name) resolving from Windows/vMix and Linux+Avahi; Windows browse freshness (libomtnet re-queries
PTR every 8 s); vMix quality/multichannel audio behaviour.

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
