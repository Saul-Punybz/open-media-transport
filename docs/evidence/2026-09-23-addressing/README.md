# Addressing by name, re-resolving after a move, and redirect (§8, §9)

Date: 23 Sep 2026. Host: macOS 26.5.1, Apple Silicon, one Mac. libomtnet `029ef4e`
(v1.0.0.19) through `interop/libomtnet-harness` (now with a redirect schedule on `send`
and a `RedirectAddress` printout on `recv`). Ours: `omt-recv` and `omt-send` examples of
`open-media-transport` at the commit that adds this directory. Tools outside Claude:
`tshark` on `lo0` (ports 6400-6600), `dns-sd -L`, and libomtnet's own log lines.
The scripts that ran are `s1.sh`, `s23.sh`, `s4.sh`; `extract.py` pulls the redirect
frames out of a capture. `control-packets.pcapng` keeps only packets under 200 bytes
(handshakes and control messages, no video); the full captures were not kept.

Source names begin with `addressing-` (other agents were announcing at the same time).
`M` below is `SAULS-MACBOOK-PRO.LOCAL`.

## s1 — libomtnet sender redirects, our receiver follows (X1, X2)

libomtnet `send addressing-A` redirects to `M (addressing-B)` (another libomtnet sender)
at 5 s and clears at 13 s. Our `omt-recv "M (addressing-A)"` found A by DNS-SD.

| Seen | Where |
|---|---|
| libomtnet's exact bytes: `<OMTRedirect NewAddress="M (addressing-B)" />`, 67 bytes, timestamp 0, no NUL; cancel is `<OMTRedirect NewAddress="" />` (29 bytes) | `redirect-frames.txt` |
| Our receiver: redirect event, both connections to B's port 6401, then back to A's 6400 on the cancel | `omt-recv.txt` |
| Our side connection to A while on B: one metadata-only connection (`<OMTSubscribe Metadata="true" />` only) that received the cancel | `redirect-frames.txt` (port 52099), `conv.txt` |
| libomtnet's sender itself opens a metadata-only connection to the target (127.0.0.1 → 6401), its X3 watcher | `conv.txt` |
| After the cancel, libomtnet sends `NewAddress=""` to **every new connection** (`OMTRedirect.cs:59-63`) | `redirect-frames.txt` (52103, 52104) |
| A's tally went to program while we were on A, off while on B, on again after | `libomtnet-send-A.txt` |

## s2 — our sender redirects, libomtnet receivers follow (X1, X2)

Our `omt-send addressing-C` redirects to `M (addressing-D)` (libomtnet) at 5 s, clears at
11 s, redirects again at 18 s. libomtnet receiver 1 watches C from the start; receiver 2
joins at 14 s, after the clear.

- Our redirect bytes are byte-identical to libomtnet's in s1 (67 and 29 bytes, timestamp
  0, no NUL): `redirect-frames.txt`.
- Receiver 1 logs "First redirect … to M (addressing-D)", connects video and audio to D,
  returns to C on the cancel, and follows the second redirect (`libomtnet-recv-1*.txt`).
- Receiver 2 follows the second redirect (`libomtnet-recv-2*.txt`): our sender sends
  nothing to a new connection while no redirect is active.

## s3 — the same with a libomtnet sender: a libomtnet bug

Identical to s2 but C is libomtnet. Receiver 1 behaves the same. Receiver 2, joining after
the clear, is sent `NewAddress=""` on connect, logs "First redirect of … to " (empty) and
"Redirect stopped", and when C redirects again at 18 s logs **"Skipping redirect to
M (addressing-D) due to existing side channel"** and stays on C
(`libomtnet-recv-2-log.txt`). Cause: `OMTReceive.cs:579-594` treats the empty address as a
first redirect and creates its redirect state; `OMTRedirect.cs:84-90` then creates no side
connection, and later redirects on the main connection are skipped. Our sender avoids
triggering it (s2); our receiver ignores an empty first redirect.

## s4 — connect by name, sender restarts on another port (N2, N5)

libomtnet `send addressing-M` on port 6400 ran 7 s and exited; a plain listener then held
6400 (`blocker.txt`), and a new `send addressing-M` came up on **6401**. `dns-sd -L` shows
the SRV port change 6400 → 6401 (`dns-sd-L.txt`). Our `omt-recv "M (addressing-M)"`:
connected to 6400, lost it, made one attempt that landed on the placeholder listener at
6400 (the entry was still the old one), then re-resolved and connected video and audio to
6401, with PSNR 58.0-58.9 dB on the second sender's frames (`omt-recv.txt`).

## Not shown

- Redirect chains (X3) across implementations: only between our own senders
  (`redirect::tests::end_to_end::chains_are_forwarded_by_the_first_sender`), and the
  libomtnet sender's watcher connection in s1.
- Redirect to an `omt://` URL, escaping of special characters in `NewAddress`, and any of
  this on Windows, Linux, vMix, OBS or a Pi.
- Re-resolving after an address change (only a port change was shown).
