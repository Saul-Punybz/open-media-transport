# Porting the OMT protocol to Rust

The goal: the `open-media-transport` crate — discovery, sending and receiving —
ported from upstream [`libomtnet`](https://github.com/openmediatransport/libomtnet)
(C#, MIT, roughly 10,600 lines). With it, a Rust program can appear as a source in
vMix or OBS and can consume other OMT sources. Without it, `vmx-codec` compresses
frames that have nowhere to go.

This plan is deliberately front-loaded with reading and specification. A wire
protocol ported by guesswork fails in the field, not in the test suite.

## Stage 0 — Get the ground truth

- Clone the upstream reference beside the existing one:
  `git clone --depth 1 https://github.com/openmediatransport/libomtnet reference/libomtnet`
  (`/reference/` is gitignored, exactly like `libvmx`).
- Also fetch anything else upstream publishes that pins behaviour: the protocol
  documentation on openmediatransport.org, the OBS plugin, the Raspberry Pi encoder.
  Note what exists and what does not.
- Produce an inventory in `docs/UPSTREAM.md`: every source file, what it is
  responsible for, its line count, and which of the three jobs it serves
  (discovery / send / receive). This is the map the rest of the work is cut from.

**Done when** someone can read `docs/UPSTREAM.md` and say which files matter.

## Stage 1 — Write the spec before the code

Write `docs/PROTOCOL.md`: the wire format as upstream actually implements it.

- Discovery: service type, instance naming, TXT records, timings.
- Connection setup: transport, handshake, capability exchange, versioning.
- Framing: headers, field widths, endianness, timestamps and clock, how video,
  audio and metadata are distinguished, how quality and format are signalled.
- Teardown, reconnect and error behaviour.

**Every claim cites upstream `file:line`.** Where upstream is ambiguous or the
behaviour is emergent rather than specified, say so in the text — an honest
"unclear, to be confirmed against a real sender" is worth more than a confident
sentence that turns out wrong. Keep a "Confirmed against a live implementation"
column that starts entirely empty.

**Done when** the spec covers enough to implement a receiver, with citations.

## Stage 2 — Discovery

- Announce and browse over mDNS. Check `mdns-sd` and alternatives first; prefer a
  maintained crate over writing mDNS by hand (see the reuse rule in `STATUS.md`).
- Two tests that matter, and neither is a unit test:
  1. A real OMT application sees a name we announce.
  2. We see a name a real OMT application announces.
- Capture both with `tshark` and keep the capture in the repo as evidence.

## Stage 3 — Receive

Connect to a real sender, pull frames, decode them with `vmx-codec`, and prove the
pixels are right. The first milestone worth celebrating is a window showing video
that came out of somebody else's software.

- Start with the simplest path upstream supports; get one format correct before
  adding the rest.
- Audio and metadata after video, not alongside it.

## Stage 4 — Send

The mirror image: announce, accept a connection, encode with `vmx-codec`, and be
consumed by a real receiver. Then the loop that proves the whole thing —
our sender to our receiver, and our sender to theirs.

## Stage 5 — Interop matrix

A table in `docs/INTEROP.md`: our sender and receiver against vMix, OBS with the
OMT plugin, and the Raspberry Pi encoder, with versions, dates and what broke.
This table is the only thing that lets the README say the implementation works.

## Not in scope here

- SIMD for `vmx-codec` (separate work; the codec is already correct and fast
  enough to build on).
- Caudal's OMT ingest and output — that is Caudal's M12, and it starts only once
  this crate sends and receives against real equipment.
- Anything using OMT's or vMix's branding.

## How to judge progress

Not by lines ported. By this question: **what can talk to us today that could not
talk to us yesterday?** Stages 0 and 1 answer "nothing, and that is expected";
from stage 2 on, every stage has to move that answer.
