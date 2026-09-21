# Upstream inventory

Stage 0 of [`PROTOCOL_PLAN.md`](PROTOCOL_PLAN.md): what upstream publishes, which
parts of it pin the wire protocol, and which files of `libomtnet` the port is cut from.

Compiled 21 Sep 2026. This is a map, not a spec: it says where things live, not how
they behave on the wire. Descriptions come from reading the source outlines; none of
it has been checked against a running implementation.

## Pinned sources

All clones live under `reference/` (gitignored).

| Clone | Upstream | Commit | Date | What it is |
|---|---|---|---|---|
| `reference/libomtnet` | [openmediatransport/libomtnet](https://github.com/openmediatransport/libomtnet) | `029ef4e925c68973f3289eb0c7ecfd40a161166f` | 2026-09-07 | **The protocol.** C#, v1.0.0.19 (`libomtnet.csproj:6-8`). Full clone, tags `v1.0.0.3`…`v1.0.0.19`. |
| `reference/libvmx` | [openmediatransport/libvmx](https://github.com/openmediatransport/libvmx) | `544bcfb` | — | The codec, already ported as `vmx-codec`. |
| `reference/upstream-libomt` | [openmediatransport/libomt](https://github.com/openmediatransport/libomt) | `bb4b67b` | 2026-08-24 | C ABI wrapper (`libomt.h`, 610 lines) over libomtnet, compiled with NativeAOT. |
| `reference/upstream-omtplugin` | [openmediatransport/omtplugin](https://github.com/openmediatransport/omtplugin) | `b6356ee` | 2026-09-03 | OBS source/output plugin. |
| `reference/upstream-OMTDiscoveryServer` | [openmediatransport/OMTDiscoveryServer](https://github.com/openmediatransport/OMTDiscoveryServer) | `379f758` | 2025-08-12 | 70-line console host for `libomtnet`'s `OMTDiscoveryServer`. |
| `reference/upstream-omtcapture` | [openmediatransport/omtcapture](https://github.com/openmediatransport/omtcapture) | `ada4fe7` | 2026-04-29 | Raspberry Pi 5 encoder (V4L2 capture → OMT send). |
| `reference/upstream-omtplayer` | [openmediatransport/omtplayer](https://github.com/openmediatransport/omtplayer) | `c47397a` | 2026-05-25 | Raspberry Pi 5 decoder (OMT receive → DRM/ALSA). |
| `reference/upstream-Examples` | [openmediatransport/Examples](https://github.com/openmediatransport/Examples) | `8963efe` | 2025-09-25 | C++ send/receive tests over `libomt`, one C# example. |
| `reference/upstream-Metadata` | [openmediatransport/Metadata](https://github.com/openmediatransport/Metadata) | `f4f55b7` | 2025-09-02 | README only: recommended application-level XML (`OMTWeb`, `OMTPTZ`, `AncillaryData`, `OMTGroup`). |
| `reference/upstream-dotgithub` | [openmediatransport/.github](https://github.com/openmediatransport/.github) | `2d66417` | 2026-09-07 | Org profile and `DOWNLOADS.md` (list of third-party tools). |
| `reference/community-openmediatransport-rs` | [MikanseiLaboratory/openmediatransport-rs](https://github.com/MikanseiLaboratory/openmediatransport-rs) | `7711da4` | 2026-09-12 | Not upstream — see [Existing Rust work](#existing-rust-work). |

Not cloned: `Logos` (branding, off limits), `omtcase` (3D-printed case).

## What exists and what does not

**There is exactly one implementation of the wire protocol upstream, and it is
`libomtnet`.** Every other upstream repo that talks OMT — `libomt`, `omtplugin`,
`OMTDiscoveryServer`, `omtcapture`, `omtplayer`, `Examples` — has a
`<Reference Include="libomtnet">` in its project file (or links `libomt`, which wraps it)
and contains no framing or discovery code of its own. Consequences:

- Interop with the OBS plugin or the Raspberry Pi devices is interop with
  `libomtnet`. It is still worth doing (different versions, different platforms'
  discovery back ends), but it is not an independent second implementation.
- vMix is closed source. Whether it embeds `libomtnet`/`libomt` or has its own
  implementation is **unknown**; it is the one adopter likely to be independent evidence.
- Other adopters on openmediatransport.org (Central Control, CNDLive, DataVideo, Miri,
  SIENNA, Softvelum, SRT Mini Server) are closed; their implementations are unknown.

**Written protocol documentation exists, but it is short.** `libomtnet/PROTOCOL.md`
(188 lines, "Protocol 1.0") covers: little-endian byte order, the 16-byte frame header,
the 32-byte video and 24-byte audio extended headers, the fixed-string metadata
commands (subscribe, preview, tally with its deliberate `Program==` typo, quality,
sender info), DNS-SD service type and instance naming, and the discovery-server XML.
openmediatransport.org itself has no spec; it links to GitHub.

`PROTOCOL.md` is a secondary source. Stage 1 cites **source** `file:line`, and uses
`PROTOCOL.md` only to find what to look for. Things spotted in the source that
`PROTOCOL.md` does not describe, to be read properly in stage 1:

- **Redirect.** `OMTRedirect.cs` and the `<OMTRedirect` prefix handled in
  `OMTChannel.cs:394-397` — a sender can point its receivers at another address.
- **TXT records.** None. *Corrected 21 Sep 2026:* an earlier version of this file
  said `mac/OMTDiscoveryDnsSd.cs:248` builds a TXT record on registration. It defines
  `CreateTXTRecord` but never calls it; all three platforms register with no TXT data
  (see `PROTOCOL.md` D5).
- **Port range.** Senders listen on 6400–6600 by default (`OMTConstants.cs:64-65`),
  overridable in `settings.xml` (`OMTSettings.cs:41-45`). Discovery server default
  port 6399 (`OMTConstants.cs:34`).
- **Buffer and size limits** that shape what a receiver must accept:
  `VIDEO_MAX_SIZE` 10 MiB, `AUDIO_MAX_SIZE` 1 MiB, `METADATA_FRAME_SIZE` 64 KiB
  (`OMTConstants.cs:57-70`).
- **Preview video** (`OMTSettings Preview="true"`) depends on libvmx's preview
  encode/decode path (`codecs/OMTVMX1Codec.cs:180-266`). Whether `vmx-codec`
  already covers that path must be checked before stage 3.

## libomtnet file inventory

10,580 lines of C# in 44 files under `src/`. Jobs: **D** discovery, **S** send,
**R** receive; **all** means shared by every path; **—** means not needed for the port.

Port verdict:
- **port** — logic that defines or directly produces wire behaviour.
- **reference** — read to learn the behaviour, then replace with a Rust crate or
  idiom (mDNS, sockets, pools, logging).
- **skip** — platform glue, .NET plumbing, or a feature out of scope for now.

### Wire format and connection (the core)

| File | Lines | Jobs | Verdict | Responsibility |
|---|---:|---|---|---|
| `OMTFrame.cs` | 397 | S R | **port** | Frame header, video and audio extended header structs; read/write of headers and data; `OMTFrameLength` constants; `OMTActiveAudioChannels` bitfield. The byte layout lives here. |
| `OMTBinary.cs` | 140 | S R | **port** | Little-endian reader/writer for byte, u16, i32, u32, i64, f32. |
| `OMTChannel.cs` | 594 | S R | **port** | One TCP connection: async receive loop that reassembles frames, matches incoming metadata against the fixed command strings (`:327-397`), holds subscription/tally/preview/quality state, sends frames, reports statistics. The protocol state machine. |
| `OMTMetadata.cs` | 142 | S R | **port** | The exact command strings (`OMTMetadataConstants`, `:36-48`), prefixes for quality/info/redirect (`:50-59`), metadata frame type, tally → XML. |
| `OMTSend.cs` | 829 | S | **port** | Sender: picks a port, listens, accepts channels, registers with discovery, merges tally across receivers, honours subscriptions and quality, encodes video (VMX1) and audio (FPA1), sends sender info and connection metadata. |
| `OMTReceive.cs` | 1123 | R | **port** | Receiver: resolves a name via discovery, connects, subscribes, reconnects, follows redirects, decodes VMX1 and FPA1 into the requested pixel format, sends tally, flags and suggested quality. Largest file. |
| `OMTSendReceiveBase.cs` | 177 | S R | **port** | Shared tally wait/update, channel events, codec timing statistics, metadata → `OMTMediaFrame`. |
| `OMTClock.cs` | 103 | S | **port** | Sender-side timestamp synthesis (100 ns units) and pacing when the caller passes timestamp −1. |
| `OMTRedirect.cs` | 204 | S R | **port** (later) | Redirect XML and the logic that moves receivers to another sender. Not in `PROTOCOL.md`. |
| `OMTPublicTypes.cs` | 452 | S R | **port** (API shape) | Public enums and structs: frame type, video flags, codec FourCCs, colour space, preferred format, receive flags, quality, statistics, `OMTSenderInfo` (XML), `OMTMediaFrame`, tally. Several values go on the wire. |
| `OMTConstants.cs` | 74 | all | **port** (values) | Ports, buffer sizes, pool counts, size limits, `omt://` prefix. |

### Codecs

| File | Lines | Jobs | Verdict | Responsibility |
|---|---:|---|---|---|
| `codecs/OMTVMX1Codec.cs` | 286 | S R | **port** | Wraps libvmx for OMT: profile, colour space, image types, quality, encode, decode, preview decode and preview size. Maps onto `vmx-codec`. |
| `codecs/OMTFPA1Codec.cs` | 94 | S R | **port** | FPA1 audio: 32-bit float planar, skips silent channels via the active-channel mask. Small and self-contained. |
| `codecs/IVMXCodec.cs` | 70 | S R | skip | Interface over the two P/Invoke back ends. |
| `codecs/VMXCodec.cs` | 188 | S R | skip | P/Invoke forwarding to `libvmx`. Useful only as a list of which libvmx entry points OMT uses. |
| `codecs/VMXCodecIOS.cs` | 189 | S R | skip | Same, for iOS static linking. |
| `codecs/VMXUnmanaged.cs` | 99 | S R | skip | `DllImport` declarations. |
| `codecs/VMXUnmanagedIOS.cs` | 99 | S R | skip | Same, iOS. |

### Discovery

| File | Lines | Jobs | Verdict | Responsibility |
|---|---:|---|---|---|
| `OMTDiscovery.cs` | 656 | D | **port** | Platform-neutral discovery: picks the platform back end (`:78-104`), keeps the table of discovered and registered entries, expiry, lookup by full name or URL, switches to the discovery server when `settings.xml` names one (`:50-66`). |
| `OMTAddress.cs` | 338 | D S R | **port** | Source naming (`MACHINE (Name)`), escaping/sanitising, validity, `omt://` URLs, address lists, `OMTAddress` XML for the discovery server. |
| `mac/OMTDiscoveryDnsSd.cs` | 437 | D | reference | macOS/iOS: DNS-SD browse, resolve, register. Registers with no TXT data (`:270`); `CreateTXTRecord` (`:248`) is unused. |
| `win32/OMTDiscoveryWin32.cs` | 525 | D | reference | Windows: DnsApi browse/register; runs `MDNSClient` alongside. Second opinion on record content. |
| `linux/OMTDiscoveryAvahi.cs` | 292 | D | reference | Linux: Avahi browse, resolve, register. Third opinion. |
| `mdns/MDNSClient.cs` | 232 | D | reference | Hand-built mDNS PTR query for `_omt._tcp.local` every 8 s on 224.0.0.251 and ff02::fb, to work around Windows DNS-SD going quiet. Tells us which query real receivers send. |
| `mac/DnsSd.cs` | 433 | D | skip | P/Invoke bindings for `dns_sd.h`. |
| `win32/DnsApi.cs` | 285 | D | skip | P/Invoke bindings for Windows DnsApi. |
| `linux/AvahiClient.cs` | 106 | D | skip | P/Invoke bindings for Avahi. |
| `mac/DispatchQueue.cs` | 69 | D | skip | libdispatch queue wrapper. |
| `mac/OMTDiscoveryMac.cs` | 32 | D | skip | Empty subclass of `OMTDiscoveryDnsSd`. |
| `server/OMTDiscoveryClient.cs` | 194 | D | **port** (later) | Client for the TCP discovery server: sends register/deregister, applies what the server repeats. Uses the ordinary OMT framing. |
| `server/OMTDiscoveryServer.cs` | 243 | D | **port** (later) | The discovery server: tracks registrations per client, substitutes the client's real IP, rebroadcasts. |

### Infrastructure

| File | Lines | Jobs | Verdict | Responsibility |
|---|---:|---|---|---|
| `OMTSettings.cs` | 154 | all | reference | `settings.xml` in `~/.OMT` or `C:\ProgramData\OMT` (override `OMT_STORAGE_PATH`): `DiscoveryServer`, `NetworkPortStart`, `NetworkPortEnd`. Port only if compatibility with that file is wanted. |
| `OMTBuffer.cs` | 145 | S R | reference | Growable/pinned byte buffers; metadata ↔ UTF-8. |
| `OMTFramePool.cs` | 80 | S R | reference | Frame pool (4 video, 10 audio). The receive-never-blocks advice in `PROTOCOL.md` is implemented partly here. |
| `OMTSocketAsyncPool.cs` | 148 | S R | reference | Pool of `SocketAsyncEventArgs` for sends. |
| `OMTUtils.cs` | 227 | all | reference | Hostname resolution, UTF-8 marshalling, interleaved → planar audio, frame-rate conversions, IPv4 test. |
| `OMTPlatform.cs` | 110 | all | reference | Platform detection, machine name, storage path, native library loading. Machine name matters: it is the first half of every source name. |
| `mac/MacPlatform.cs` | 121 | all | reference | macOS machine name and storage path. |
| `linux/LinuxPlatform.cs` | 85 | all | reference | Linux machine name and storage path. |
| `win32/Win32Platform.cs` | 76 | all | reference | Windows machine name and storage path. |
| `OMTInternalTypes.cs` | 56 | S R | skip | Event-args classes. |
| `OMTLogging.cs` | 213 | — | skip | File/callback logger; replaced by `tracing` or `log`. |
| `OMTBase.cs` | 63 | — | skip | `IDisposable` base. |

### What matters, in reading order

For stage 1 a receiver needs, in this order: `OMTFrame.cs`, `OMTBinary.cs`,
`OMTMetadata.cs`, `OMTChannel.cs`, `OMTReceive.cs`, `codecs/OMTVMX1Codec.cs`,
`codecs/OMTFPA1Codec.cs`, `OMTPublicTypes.cs`, `OMTConstants.cs`; then for discovery
`OMTAddress.cs`, `OMTDiscovery.cs`, `mac/OMTDiscoveryDnsSd.cs`, `mdns/MDNSClient.cs`,
and the Windows and Linux back ends to compare record content. That is about 5,100
lines. Sending adds `OMTSend.cs`, `OMTClock.cs` and `OMTSendReceiveBase.cs`.
Redirect and the discovery server can wait until basic send and receive work.

## Existing Rust work

The reuse rule in `STATUS.md` says to check before writing. Two projects already exist.
**Neither was known when `PROTOCOL_PLAN.md` was written, and the first one bears on
whether this crate should exist at all. That is a decision for the maintainer, not
for this document.**

**[MikanseiLaboratory/openmediatransport-rs](https://github.com/MikanseiLaboratory/openmediatransport-rs)**
— crate `openmediatransport` 0.1.0, MIT, started 2026-08-09, last push 2026-09-12,
1 GitHub star, **not published on crates.io**. Listed in upstream's own `DOWNLOADS.md`
as "OMT Community Rust SDK". About 9,000 lines of Rust in `src/`, covering the same
ground as the plan: framing, metadata commands, discovery via `mdns-sd` 0.20, the
discovery server and client, redirect, settings, FPA1, sync and optional `tokio`
APIs, optional `wgpu`. It has its own `PROTOCOL.md` citing libomtnet at an older
commit (`2846a96`). Its video codec is a separate crate,
[`vmx-rs`](https://github.com/MikanseiLaboratory/vmx-rs) (MIT, with SIMD), pulled
by git rev. Edition 2024, MSRV 1.97.

What was checked: file list, `Cargo.toml`, README. What was **not** checked: whether
it builds, whether its tests pass, whether it matches libomtnet, whether it has ever
talked to vMix, OBS or a Pi. Its README makes no interop claims either way. Nothing
here says it is correct or incorrect.

**[raycaster-io/libomt-rs](https://github.com/raycaster-io/libomt-rs)** — crate
`libomt` 0.2.0 on crates.io, bindings to the native `libomt` (i.e. .NET code compiled
with NativeAOT). Not pure Rust, so not a substitute for this crate, but potentially
useful as a test oracle: it drives the real upstream implementation from Rust.

Other finds worth knowing: `DOWNLOADS.md` lists free tools that make interop
testing possible without owning vMix — vMix's OMT Viewer and Desktop Capture (Windows),
SIENNA's OMT tools including a signal generator and monitor (macOS), Central Control's
OMT Signal Generator (Windows), and MikanseiLaboratory's community tools.

## Open questions for stage 1

Answered in [`PROTOCOL.md`](PROTOCOL.md) and [`COMPARISON.md`](COMPARISON.md):

1. TXT records: none on any platform (D5).
2. `vmx-codec` already has preview length and preview decode (§6.2).
3. Frame layout history across tags: not yet examined.
4. The community crate: we build our own and use it as a cross-check (`COMPARISON.md`).
