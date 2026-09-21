# OMT wire protocol, as libomtnet implements it

Stage 1 of [`PROTOCOL_PLAN.md`](PROTOCOL_PLAN.md). This describes what upstream
**code** does, not what upstream documentation says. Where the two disagree, the
code is quoted and the disagreement is flagged.

- Source: `libomtnet` at `029ef4e` (v1.0.0.19), in `reference/libomtnet`.
  Unless a path says otherwise, paths are relative to `reference/libomtnet/src/`.
- Every normative statement cites `file:line`. Anything without a citation is
  labelled **inference** or **unclear**.
- Upstream's own `reference/libomtnet/PROTOCOL.md` is cited as `PROTOCOL.md:line`
  and treated as secondary.

**Nothing in this document has been confirmed against a running implementation.**
The last column of every table, "Live", starts empty. It gets a date and a capture
file only when a `tshark` capture of real `libomtnet`, vMix, OBS or a Pi shows the
behaviour.

---

## 1. Transport

| # | Statement | Source | Live |
|---|---|---|---|
| T1 | Plain TCP. A sender listens on a dual-stack IPv6 socket (`IPv6Only = false`), backlog 5. | `OMTSend.cs:97,107-109` | |
| T2 | The sender takes the first free port from 6400 to 6600 inclusive. The range can be overridden with `NetworkPortStart`/`NetworkPortEnd` in `settings.xml`. | `OMTSend.cs:99-119`, `OMTConstants.cs:64-65` | |
| T3 | Both ends set `TCP_NODELAY`, and try to enable SO_KEEPALIVE with idle time 5 (the option is set with raw number 3 and the value 5; the unit is platform-translated by .NET and not stated). | `OMTChannel.cs:75,88-89` | |
| T4 | Socket buffers: send 64 KiB; receive 8 MiB on video/audio connections, 64 KiB on metadata-only ones. | `OMTChannel.cs:76-83`, `OMTConstants.cs:38-40` | |
| T5 | **A receiver opens two TCP connections to the same port**: one for video + metadata and one for audio. Only the connections for the requested frame types are opened. A metadata-only receiver opens one. | `OMTReceive.cs:363-377`, and the sender's own comment at `OMTSend.cs:340` | |
| T6 | The sender does not know in advance what a connection is for. Every accepted connection is created as a metadata channel. What it carries is decided by the subscribe commands it later receives (§4.1). | `OMTSend.cs:364`, `OMTChannel.cs:195-198,327-339` | |

`PROTOCOL.md` does not mention T5. A receiver that opens one connection and
subscribes to video and audio on it should still work, because the sender filters
per connection on subscriptions only (`OMTChannel.cs:195`). This is **inference**
and needs a test.

## 2. Byte order and scalar encoding

| # | Statement | Source | Live |
|---|---|---|---|
| B1 | Every multi-byte field is little-endian, written byte by byte. | `OMTBinary.cs:47-137` | |
| B2 | `float` is IEEE-754 binary32, the bit pattern written as a little-endian `u32`. | `OMTBinary.cs:77-82,129-137` | |

## 3. Frames

### 3.1 Frame header (16 bytes)

| Offset | Size | Field | Type | Source |
|---:|---:|---|---|---|
| 0 | 1 | `Version` | u8, always 1 | `OMTFrame.cs:30-33,129,189` |
| 1 | 1 | `FrameType` | u8: 1 metadata, 2 video, 4 audio | `OMTFrame.cs:190`, `OMTPublicTypes.cs:34-40` |
| 2 | 8 | `Timestamp` | i64, units of 100 ns (10,000,000 per second) | `OMTFrame.cs:191`, `OMTPublicTypes.cs:286-297` |
| 10 | 2 | `MetadataLength` | u16, bytes of per-frame metadata at the end of the payload | `OMTFrame.cs:79,192` |
| 12 | 4 | `DataLength` | i32, bytes that follow this header: extended header + data + per-frame metadata | `OMTFrame.cs:80,193-200,295` |

Header size 16: `OMTFrame.cs:106`. The struct also has two commented-out reserved
bytes (`OMTFrame.cs:77-78`) that are **not** on the wire; the write order above is
the truth.

### 3.2 Payload layout

```
[16-byte header][extended header: 32 video, 24 audio, 0 metadata][data][per-frame metadata]
                 \_________________________ DataLength bytes ____________________________/
```

- Extended header sizes: `OMTFrame.cs:103-109,157-171`.
- `DataLength = data.Length + ExtendedHeaderLength`, where `data` already contains
  the per-frame metadata: `OMTFrame.cs:293-296`, filled by `OMTSend.cs:743-749` (video)
  and `OMTSend.cs:813-818` (audio).
- The receiver takes the per-frame metadata as the **last `MetadataLength` bytes**
  of the data: `OMTReceive.cs:1068`. The compressed video is everything before it:
  `OMTReceive.cs:788`.

### 3.3 Video extended header (32 bytes)

| Offset | Field | Type | Values | Source |
|---:|---|---|---|---|
| 0 | `Codec` | i32 FourCC | on the wire only `VMX1` = `0x31584D56` is produced by the sender | `OMTFrame.cs:215`, `OMTSend.cs:751,778`, `OMTPublicTypes.cs:96` |
| 4 | `Width` | i32 | pixels (full frame, also in preview mode) | `OMTFrame.cs:216`, `OMTSend.cs:751` |
| 8 | `Height` | i32 | pixels | `OMTFrame.cs:217` |
| 12 | `FrameRateN` | i32 | | `OMTFrame.cs:218` |
| 16 | `FrameRateD` | i32 | | `OMTFrame.cs:219` |
| 20 | `AspectRatio` | f32 | display aspect, width/height | `OMTFrame.cs:220`, `OMTPublicTypes.cs:333-335` |
| 24 | `Flags` | i32 bitfield | 1 interlaced, 2 alpha, 4 premultiplied, 8 preview, 16 high bit depth | `OMTFrame.cs:221-228`, `OMTPublicTypes.cs:60-68` |
| 28 | `ColorSpace` | i32 | 0 undefined, 601, 709 | `OMTFrame.cs:229`, `OMTPublicTypes.cs:123-128` |

- The sender sets flag 16 (high bit depth) itself when the source was P216 or PA16:
  `OMTSend.cs:725,736`.
- The sender sets flag 8 (preview) on the wire per connection when that receiver
  asked for preview: `OMTFrame.cs:221-224`, `OMTChannel.cs:199`.
- Undefined colour space means BT.601 below 720 lines, BT.709 otherwise:
  `OMTPublicTypes.cs:118-121`. (This is a codec convention; the wire carries 0.)

### 3.4 Audio extended header (24 bytes)

| Offset | Field | Type | Source |
|---:|---|---|---|
| 0 | `Codec` | i32 FourCC, always `FPA1` = `0x31415046` | `OMTFrame.cs:233,383`, `OMTPublicTypes.cs:97` |
| 4 | `SampleRate` | i32 | `OMTFrame.cs:234` |
| 8 | `SamplesPerChannel` | i32 | `OMTFrame.cs:235` |
| 12 | `Channels` | i32, 1..=32 | `OMTFrame.cs:236`, `OMTSend.cs:800` |
| 16 | `ActiveChannels` | u32 bitfield, bit *i* set when channel *i* is present in the data | `OMTFrame.cs:237`, `codecs/OMTFPA1Codec.cs:68-86` |
| 20 | `Reserved1` | i32, written as 0 (never assigned) | `OMTFrame.cs:238`, `OMTFrame.cs:100` |

### 3.5 Metadata frames

| # | Statement | Source | Live |
|---|---|---|---|
| M1 | `FrameType` 1, no extended header, `MetadataLength` 0, data = UTF-8 XML. | `OMTChannel.cs:161-167`, `OMTBuffer.cs:109-113` | |
| M2 | **Protocol commands are sent without a terminating NUL.** They are built from string constants and encoded with `UTF8.GetBytes`, which adds nothing. | `OMTMetadata.cs:38-58`, `OMTBuffer.cs:109-113`, e.g. `OMTReceive.cs:420` | |
| M3 | **Commands are recognised by exact string equality** on the whole payload decoded as UTF-8. A command that arrives with a trailing NUL, extra whitespace or different attribute order is not a command; it is passed to the application as ordinary metadata. | `OMTBuffer.cs:115-118`, `OMTChannel.cs:326-363` | |
| M4 | Application metadata keeps whatever length the application gave. Applications are told to include the NUL (`OMTPublicTypes.cs:357,367`), and `IntPtrToXML` decodes all `length` bytes, so that NUL travels on the wire. | `OMTUtils.cs:148-158`, `OMTMetadata.cs:126-134` | |
| M5 | Metadata timestamps: protocol commands use 0. | `OMTReceive.cs:420-448`, `OMTMetadata.cs:106-124` | |
| M6 | Metadata frames are sent on a connection regardless of subscriptions. Video and audio frames are sent only if that connection subscribed to them. | `OMTChannel.cs:195-198` | |

**Conflict with upstream docs.** `PROTOCOL.md:82-84` says metadata is
"null terminated" and "DataLength should always include the null character". The
code does not do that for commands (M2), and a receiver that adds the NUL would
break command matching (M3). Follow the code. **This is the first thing to confirm
with a capture.**

### 3.6 What a receiver accepts

| # | Statement | Source | Live |
|---|---|---|---|
| R1 | `Version` other than 1 makes the header "not readable". The receive loop then treats the frame as incomplete and waits for more bytes. It never closes the connection. **Inference**: an unknown version stalls the connection. | `OMTFrame.cs:245-254`, `OMTChannel.cs:439,495` | |
| R2 | `FrameType` other than 1, 2 or 4 closes the connection ("ProtocolFailure"). | `OMTChannel.cs:441-445,415-419` | |
| R3 | A whole frame (header + `DataLength`) must fit in the receive buffer: 10 MiB on a video connection, 1 MiB on audio and metadata connections. Nothing rejects a larger frame. **Inference**: it stalls the connection like R1. | `OMTChannel.cs:102-115,497`, `OMTConstants.cs:57,62` | |
| R4 | The sender drops, rather than sends, any frame longer than 10 MiB. | `OMTChannel.cs:200-206` | |
| R5 | When the receiver's frame pool is exhausted, the frame is dropped and counted; the connection continues. | `OMTChannel.cs:470-488` | |
| R6 | At most 60 unread metadata frames are queued per connection; further ones are dropped. | `OMTChannel.cs:400-410`, `OMTConstants.cs:68` | |

## 4. Protocol commands

All strings exactly as in `OMTMetadata.cs`. Quotes are ASCII `"`; a single space
precedes `/>`.

### 4.1 Receiver → sender

| Command | Bytes | Effect on the sender | Source | Live |
|---|---|---|---|---|
| Subscribe video | `<OMTSubscribe Video="true" />` | this connection receives video | `OMTMetadata.cs:38`, `OMTChannel.cs:327-331` | |
| Subscribe audio | `<OMTSubscribe Audio="true" />` | this connection receives audio | `OMTMetadata.cs:39`, `OMTChannel.cs:332-335` | |
| Subscribe metadata | `<OMTSubscribe Metadata="true" />` | this connection receives application metadata broadcasts | `OMTMetadata.cs:40`, `OMTChannel.cs:336-339`, `OMTSend.cs:649` | |
| Preview on / off | `<OMTSettings Preview="true" />` / `<OMTSettings Preview="false" />` | video on this connection switches to preview (§6.2) | `OMTMetadata.cs:41-42`, `OMTChannel.cs:356-363` | |
| Tally | `<OMTTally Preview="…" Program=="…" />`, four fixed strings | sets this connection's tally | `OMTMetadata.cs:45-48`, `OMTChannel.cs:340-355` | |
| Suggested quality | `<OMTSettings Quality="X" />`, X ∈ `Default`, `Low`, `Medium`, `High` | sets this connection's suggestion | `OMTMetadata.cs:52-53`, `OMTReceive.cs:1047-1057`, `OMTChannel.cs:364-389` | |

- There is no unsubscribe. Subscriptions only ever add bits: `OMTChannel.cs:329,334,338`.
- Tally strings contain `Program==` with **two** equals signs. They are not
  well-formed XML and must be matched as bytes. `OMTMetadata.cs:44` explains it is
  kept for compatibility; `PROTOCOL.md:124-126` agrees.
- Quality is the one command matched by **prefix** (`<OMTSettings Quality=`) and then
  parsed as XML. Unknown values leave the previous suggestion in place:
  `OMTChannel.cs:364-389`. The names map to `OMTQuality` 0, 1, 50, 100:
  `OMTPublicTypes.cs:185-191`.

### 4.2 Sender → receiver

| Message | Bytes | When | Source | Live |
|---|---|---|---|---|
| Sender info | `<OMTInfo ProductName="…" Manufacturer="…" Version="…" />` | on connect, if set; and broadcast when set | `OMTPublicTypes.cs:238-250`, `OMTSend.cs:366-369,167-177` | |
| Connection metadata | application-defined strings | on connect | `OMTSend.cs:370,192-204` | |
| Tally | the same four strings as §4.1 | on connect (combined tally), then on every change | `OMTSend.cs:371,464-467` | |
| Redirect | `<OMTRedirect NewAddress="…" />` | on connect if a redirect is active, and when it changes | `OMTRedirect.cs:59-63,50-57,181-194` | |

- The combined tally is the OR of every connection's preview and program bits:
  `OMTSend.cs:583-597`.
- Sender info is matched by prefix `<OMTInfo`, parsed, **and** passed on to the
  application: `OMTChannel.cs:390-393`.
- Redirect is matched by prefix `<OMTRedirect`; the attribute read is `NewAddress`:
  `OMTChannel.cs:394-398`, `OMTRedirect.cs:165-180`.
- **Unclear, exact bytes.** Sender info and redirect are produced by .NET's
  `XmlTextWriter` with `Formatting.Indented` (`OMTPublicTypes.cs:240-249`,
  `OMTRedirect.cs:185-191`). A single element with attributes should serialise on one
  line as `<OMTInfo … />`, but the exact spacing and escaping has to come from a
  capture, not from memory. Receivers parse these as XML, so exact bytes matter only
  if we want byte-identical output.

### 4.3 Connection sequences

**Receiver, video connection** (`OMTReceive.cs:415-431`), in order:
1. `<OMTSubscribe Metadata="true" />`
2. `<OMTSettings Preview="true" />` — only if the receiver wants preview
3. `<OMTSubscribe Video="true" />` — if it wants video
4. `<OMTSettings Quality="…" />` — if it wants video
5. tally (current value; `TALLY_NONE` by default)

**Receiver, audio connection** (`OMTReceive.cs:432-442`):
1. `<OMTSubscribe Metadata="true" />` — only if the receiver did not ask for video
2. `<OMTSubscribe Audio="true" />`

**Receiver, metadata-only connection** (`OMTReceive.cs:443-449`):
1. `<OMTSubscribe Metadata="true" />`

**Sender, on accepting any connection** (`OMTSend.cs:363-377`), before any subscribe
arrives: sender info (if set), each connection-metadata string, tally, redirect
(if active).

- Later tally, preview and quality changes from the receiver go on the video
  connection, or the audio one if there is no video connection: `OMTReceive.cs:715-731`.

## 5. Timestamps and pacing

| # | Statement | Source | Live |
|---|---|---|---|
| C1 | Units are 100 ns. The application is expected to supply capture time. | `OMTPublicTypes.cs:286-297` | |
| C2 | Timestamp −1 asks the sender to generate timestamps. The first frame gets 0. Later frames get previous + interval, where interval is `10^7 / fps` for video or `10^7 × samples / rate` for audio. | `OMTClock.cs:58-70,90-99` | |
| C3 | In that mode the sender also **paces**: it sleeps until wall-clock catches up, and skips timestamps forward if it has fallen more than one interval behind. | `OMTClock.cs:72-83` | |
| C4 | The clock resets when frame rate or sample rate changes. | `OMTClock.cs:51-57` | |

Video and audio clocks are independent (`OMTSend.cs:85-86`); nothing in the code
aligns them. Audio/video sync is therefore up to the timestamps the application
supplies.

## 6. Media payloads

### 6.1 Video: VMX1

| # | Statement | Source | Live |
|---|---|---|---|
| V1 | The payload is one libvmx frame as produced by `VMX_SaveTo`. | `codecs/OMTVMX1Codec.cs:173-177` | |
| V2 | The sender accepts raw UYVY, YUY2, NV12, YV12, BGRA, UYVA, P216, PA16 (min 16×16) and encodes them; or it forwards an already-compressed VMX1 frame untouched. | `OMTSend.cs:671-677,766-786` | |
| V3 | Encoder profile: `Default` means `OMT_SQ` (166). A receiver's suggested quality chooses `OMT_LQ` (133), `OMT_SQ` or `OMT_HQ` (199) when the sender's own quality is `Default`; the highest suggestion among video connections wins. | `codecs/OMTVMX1Codec.cs:105`, `OMTSend.cs:511-519,486-507` | |
| V4 | Opaque BGRA is encoded with `VMX_EncodeBGRA`, not `VMX_EncodeBGRX`, even though the "BGRX" image type is chosen. | `OMTSend.cs:702-710`, `codecs/OMTVMX1Codec.cs:164-169` | |
| V5 | UYVA without the alpha flag is encoded as UYVY; PA16 without alpha as P216. | `OMTSend.cs:712-733` | |
| V6 | The receiver picks the decode format from the flags and its preferred format; alpha frames decode to BGRA/UYVA/PA16 only when asked. | `OMTReceive.cs:773-950` | |

V4 means upstream's bitstream for opaque BGRA sources carries data from the source's
alpha bytes (libvmx sets `VMX_IMAGE_BGRA`, `reference/libvmx/src/vmxcodec.cpp:1811`)
while the header says "no alpha". Receivers ignore it because the flag is clear.
**Unclear** whether this costs bandwidth; `vmx-codec` can measure it.

### 6.2 Preview mode

| # | Statement | Source | Live |
|---|---|---|---|
| P1 | For a connection in preview mode the sender sends the same header and extended header (with flag 8 added), and a `DataLength` of `32 + EncodedPreviewLength + MetadataLength`. | `OMTFrame.cs:176-178,193-196,302-305`, `OMTSend.cs:750` | |
| P2 | The bytes sent are the **first** `EncodedPreviewLength + MetadataLength` bytes of the VMX frame: the preview is a prefix of the full bitstream. | `OMTChannel.cs:220-223`, `OMTFrame.cs:331-334` | |
| P3 | The receiver decodes it with `VMX_DecodePreview*`. The output is width/8 (rounded up to even) × height/8 (made even when interlaced). | `OMTReceive.cs:797-836`, `codecs/OMTVMX1Codec.cs:245-262` | |
| P4 | For forwarded VMX1 frames the "preview" length is the full length, so preview receivers get the whole frame with flag 8 set. | `OMTSend.cs:772` | |

**Suspected upstream bug.** Because of P2, when a frame has per-frame metadata the
last `MetadataLength` bytes of a preview frame are VMX bytes, not the metadata (the
metadata sits after the *full* bitstream, `OMTSend.cs:743-747`). A receiver following
§3.2 then reads garbage as metadata. To confirm with a capture before deciding
whether to reproduce or fix it.

`vmx-codec` already has `Decoder::preview_len` and `Decoder::decode_preview`
(`crates/vmx-codec/src/decoder.rs:70,122`).

### 6.3 Audio: FPA1

| # | Statement | Source | Live |
|---|---|---|---|
| A1 | 32-bit float samples, planar: all samples of channel 0, then channel 1, … | `OMTPublicTypes.cs:344-355` | |
| A2 | Channels whose samples are all zero bytes are left out of the data and their bit is cleared in `ActiveChannels`. | `codecs/OMTFPA1Codec.cs:68-86` | |
| A3 | The receiver re-inserts silent channels as zeros, so it always outputs `Channels` planes. | `codecs/OMTFPA1Codec.cs:39-59` | |
| A4 | Receivers reject `SamplesPerChannel × Channels × 4 > 1 MiB`. | `OMTReceive.cs:1082-1113` | |

**Upstream bug, harmless on the wire.** `OMTActiveAudioChannels.C32` is
`2147483658` (`OMTFrame.cs:69`), not `2147483648` (2^31). The enum member is never
used by name; masks are built with `1 << i` (`codecs/OMTFPA1Codec.cs:45,77`), so the
wire value for channel 32 is bit 31.

"All zero bytes" in A2 means `-0.0` counts as sound, not silence.

## 7. Discovery (DNS-SD)

| # | Statement | Source | Live |
|---|---|---|---|
| D1 | Service type `_omt._tcp`, domain `local`. | `mac/OMTDiscoveryDnsSd.cs:190,267`, `win32/OMTDiscoveryWin32.cs:142,201`, `mdns/MDNSClient.cs` via `win32/OMTDiscoveryWin32.cs:122` | |
| D2 | Instance name is the source's full name, `MACHINE (Name)`. | `OMTAddress.cs:196-204`, `mac/OMTDiscoveryDnsSd.cs:262-270`, `linux/OMTDiscoveryAvahi.cs:193-195` | |
| D3 | `MACHINE` is the host name **upper-cased**: `gethostname()` on macOS and Linux, `ComputerNamePhysicalDnsHostname` on Windows. | `mac/MacPlatform.cs:51-73`, `linux/LinuxPlatform.cs:43-65`, `win32/Win32Platform.cs:59-69` | |
| D4 | The full name is cut to 63 characters by shortening `Name`. | `OMTAddress.cs:40,65-75` | |
| D5 | **No TXT data.** macOS passes `txtLen = 0`; Linux passes a null TXT list; Windows sets `dwPropertyCount = 0`. `CreateTXTRecord` exists on macOS but is never called. | `mac/OMTDiscoveryDnsSd.cs:270` (and `:248-253` unused), `linux/OMTDiscoveryAvahi.cs:195`, `win32/OMTDiscoveryWin32.cs:198` | |
| D6 | Windows removes every `.` from the instance name before registering, and advertises host `MACHINE.local`. | `win32/OMTDiscoveryWin32.cs:201-202` | |
| D7 | A browser accepts an instance only if its name contains `(` and `)`. | `OMTAddress.cs:206-219`, `mac/OMTDiscoveryDnsSd.cs:369`, `linux/OMTDiscoveryAvahi.cs:270` | |
| D8 | Windows additionally multicasts its own PTR query for `_omt._tcp.local` every 8 s to 224.0.0.251 and ff02::fb port 5353, on every non-loopback multicast interface, because the Windows API stops querying. It is a QM query (class IN, no unicast-response bit). | `mdns/MDNSClient.cs:37-62,76-124` | |
| D9 | A sender also registers itself locally with the loopback address, so receivers in the same process can find it. | `OMTSend.cs:122-125` | |
| D10 | Discovered IPv4 addresses are stored as IPv4-mapped IPv6; IPv6 link-local addresses are ignored. | `OMTAddress.cs:82-101` | |

Consequences, all **inference**:
- `PROTOCOL.md:153` gives the name form `HOSTNAME (Source Name)._omt._tcp.local`,
  which agrees with D2. It does not mention D3, D4 or D6.
- If `gethostname()` returns `mymac.local` on macOS, the instance is
  `MYMAC.LOCAL (Name)`. The same source name announced from Windows loses its dots
  (D6). A receiver matching by exact full name (§8) sees these as different sources.
- Since there is no TXT data, a browser needs only PTR, SRV and A/AAAA.

## 8. Addressing a sender

| # | Statement | Source | Live |
|---|---|---|---|
| N1 | A receiver is given either a full name `MACHINE (Name)` or a URL `omt://host:port`. | `OMTReceive.cs:142`, `OMTDiscovery.cs:389-400` | |
| N2 | Full names are matched by exact string comparison against discovered entries. | `OMTDiscovery.cs:401-415` | |
| N3 | A URL is parsed with .NET `Uri` and the host resolved with DNS; no discovery is involved. | `OMTDiscovery.cs:362-387` | |
| N4 | A sender's own URL is `omt://MACHINE:port`. | `OMTAddress.cs:60-63` | |
| N5 | Connection attempts are rate-limited to one per second and retried whenever the application calls `Receive` and the receiver is not connected. There is no other reconnect timer. | `OMTReceive.cs:328-331,662-673,675-680` | |

## 9. Redirect

A sender can tell its receivers to use another source instead ("virtual source").

| # | Statement | Source | Live |
|---|---|---|---|
| X1 | The sender sends `<OMTRedirect NewAddress="…" />` to every metadata-subscribed connection, and to each new connection. An empty address cancels the redirect. | `OMTSend.cs:230-234`, `OMTRedirect.cs:50-63,110-127` | |
| X2 | A receiver that gets a redirect reconnects to the new address, and keeps a metadata-only side connection to the original address to hear further changes. | `OMTReceive.cs:562-603`, `OMTRedirect.cs:84-108` | |
| X3 | A sender redirected to another sender that is itself redirected forwards the upstream address ("redirect chain"). | `OMTRedirect.cs:40-49,128-163` | |
| X4 | Redirecting to one's own address is treated as no redirect. | `OMTRedirect.cs:115-118` | |

Not in `PROTOCOL.md`. Implement after basic send and receive.

## 10. Discovery server (optional)

An alternative to DNS-SD for networks without multicast.

| # | Statement | Source | Live |
|---|---|---|---|
| S1 | Enabled when `settings.xml` sets `DiscoveryServer` to `omt://host:port`; DNS-SD is then not used for registering. Default port 6399. | `OMTDiscovery.cs:50-66,332-360`, `OMTConstants.cs:34`, `OMTSettings.cs:41` | |
| S2 | The client is a metadata-only OMT receiver (§4.3) and the server a metadata-only OMT sender; messages are ordinary metadata frames. | `server/OMTDiscoveryClient.cs:47-53,75-86`, `server/OMTDiscoveryServer.cs:48-52`, `OMTSend.cs:64-76` | |
| S3 | Message: `<OMTAddress>` with child elements `Name`, `Port`, optional `Removed` = `True`, and `Addresses` containing `IPAddress` elements. | `OMTAddress.cs:251-276,278-322` | |
| S4 | The server ignores client-supplied addresses and substitutes the client's TCP source address. | `server/OMTDiscoveryServer.cs:198-203` | |
| S5 | The server rebroadcasts each add and remove to every connection, including the sender's; sends the full table to each new client; and removes a client's entries when it disconnects. | `server/OMTDiscoveryServer.cs:113-176` | |
| S6 | The client sends all its local sources on (re)connect, and forgets server-learned sources on disconnect. | `server/OMTDiscoveryClient.cs:88-131` | |

**Conflict with upstream docs.** `PROTOCOL.md:170-174` shows `<Addresses><Address>`.
The code writes and reads `<Addresses><IPAddress>` (`OMTAddress.cs:268,298`). Both
sides ignore client addresses anyway (S4), so this matters only for server → client
messages, where the code's form is what clients parse. Follow the code.

## 11. Differences from upstream `PROTOCOL.md`, summarised

| Topic | `PROTOCOL.md` | Code | Follow |
|---|---|---|---|
| NUL on metadata | always included (`:82-84`) | absent on commands; commands must match exactly (M2, M3) | code |
| Connections per receiver | not mentioned | two: video+metadata and audio (T5) | code |
| Discovery-server address element | `Address` (`:173`) | `IPAddress` (S3) | code |
| TXT records | not mentioned | none (D5) | code |
| Redirect | not mentioned | implemented (§9) | code |
| Instance name | `HOSTNAME (Source Name)` | same, host upper-cased, ≤ 63 chars, dots removed on Windows (D3, D4, D6) | code |

## 12. Upstream bugs and oddities found while reading

None affects a conforming receiver except the preview one. Listed so nobody
"fixes" our implementation into incompatibility, or copies a bug by accident.

| # | What | Source | Wire impact |
|---|---|---|---|
| U1 | `C32 = 2147483658` instead of 2^31 | `OMTFrame.cs:69` | none (enum member unused) |
| U2 | Preview frames with per-frame metadata carry VMX bytes where the metadata should be | `OMTSend.cs:743-750`, `OMTChannel.cs:220-223` | yes, suspected (§6.2) |
| U3 | Opaque BGRA encoded with `EncodeBGRA` | `codecs/OMTVMX1Codec.cs:167-168` | bitstream differs from `EncodeBGRX`; decodes the same (V4) |
| U4 | Port-range loop stops on the compile-time end port, not the configured one | `OMTSend.cs:114` | only with custom `NetworkPortEnd` |
| U5 | `IsEmpty` adds the buffer offset twice | `codecs/OMTFPA1Codec.cs:62,75` | none while the offset is 0, which it is (`OMTSend.cs:810`) |
| U6 | Unknown header version stalls instead of failing | `OMTFrame.cs:245-254`, `OMTChannel.cs:439` | robustness only (R1) |
| U7 | Application metadata received is re-terminated, so a NUL-terminated payload reaches the application with two NULs | `OMTSendReceiveBase.cs:155-168`, `OMTUtils.cs:138-146` | none on the wire |

## 13. Enough to implement a receiver?

Yes, for the first milestone: browse `_omt._tcp` (§7), connect (§1), send the video
connection sequence (§4.3), parse frames (§3), decode VMX1 (§6.1) with `vmx-codec`.
Open items that a capture must settle before calling it done:

1. M2/M3: commands without NUL.
2. T5: whether one connection for video and audio also works.
3. Exact `<OMTInfo>` and `<OMTRedirect>` bytes.
4. U2: preview + per-frame metadata.
5. D3/D6: the instance names real senders on macOS and Windows actually publish.
