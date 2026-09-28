# `fix/library` between two machines: media, redirect policy, discovery server

Date: 28 Sep 2026. `omt` built from branch `fix/library` on both machines: `24a79e6`
for §1, `d5c75a5` for §2 and §3 (the two commits in between change only how a
receiver handles a refused redirect).

| | Mac | NUC |
|---|---|---|
| OS | macOS 26.5.1 | Linux Mint 22.3 |
| CPU | Apple Silicon (arm64) | Intel i3-7100U (x86_64) |
| Address | 172.16.80.64 | 172.16.80.73 |
| Link | Wi-Fi (`en0`) | Wi-Fi (`wlp58s0`; the Ethernet port `eno1` had no cable) |

Both machines were on the **same Wi-Fi network**. The machines and link are the same as
in `2026-09-28-cross-machine`. That README says "wired LAN", but on the day `ip route get`
on the NUC and `route get` on the Mac both showed the Wi-Fi interface.

`omt` to `omt` only: nothing here involves libomtnet, vMix, OBS or a Pi.
`M` = `SAULS-MACBOOK-PRO.LOCAL`, `N` = `PUNYBZ-NUC7I3BNHXF`.

## 1. Media both ways, after the discovery and receiver changes

The changes under test: one shared mDNS browse per `Discovery`, the bounded
receiver queue, and the sender's connection limits and subscribe timeout.

- **N → M** (`1a-*`): the Mac found `N (FL NUC)` by name and received 1280x720 at 30 fps.
  It saw 21–34 fps in each one-second window (Wi-Fi) and 0 decode errors, and saved a
  1280x720 snapshot. The sender reported `dropped=0` and 2 connections, 1 of them video,
  while the Mac was receiving.
- **M → N** (`1b-*`): the NUC listed `M (FL Mac)` at `Sauls-MacBook-Pro-omt.local:6400
  [172.16.80.64]` and received it with 0 decode errors. `1b-mac-to-nuc.png` is the
  snapshot: bars, grey ramp and box, decoded on x86_64 from an arm64 VMX1 stream.

## 2. Redirect to another machine, by policy (`ReceiverConfig::redirects`)

`N (FL Virtual)` is `omt send --redirect "M (FL Real)"`: a virtual source on the NUC
that points at a sender on the Mac. The receivers connected by name.

| Run | Receiver | `--redirects` | Expected | Seen |
|---|---|---|---|---|
| `4a.txt` | NUC | `same-host` | target is on another host: refuse, stay on N | redirect heard; stayed connected to 172.16.80.73 at ~30 fps with no gap |
| `4b.txt` | NUC | `any` | follow to M, as libomtnet | switched to 172.16.80.64 within about a second; 0 decode errors |
| `4c.txt` | Mac | `same-host` | the original is on N, the target on M: refuse | stayed on 172.16.80.73 with no gap |

Earlier runs of 4a found two gaps, both fixed on the branch:

- **A refused redirect dropped the connections before checking the policy.** The
  receiver was without video for about a second, then fell back to the original. Fixed
  in `c79e929`.
- **The NUC receiver dropped for about a second when the redirect arrived.** The
  redirect came as soon as the receiver connected, before mDNS had found `M (FL Real)`,
  so the policy check did not know the target yet. Fixed in `d5c75a5`, which waits up to
  2 s for the name while staying connected.

`4-mac-real-send.txt` shows one connection to `M (FL Real)` with no video subscription.
That is the NUC sender's metadata-only watcher on its redirect target (X3).

## 3. Discovery server across the network (§10)

`omt discovery-server` on the NUC (port 6399). The Mac ran
`omt send --discovery-server omt://172.16.80.73:6399`.

- `5-nuc-server.txt`: the Mac's registration was recorded with the address of its
  connection, `[172.16.80.64]`, not the loopback address the client sends (S4). When
  the Mac's sender exited, the entry was removed and the connection closed.
- `5a.txt`: `omt list --discovery-server … --no-mdns` on the NUC showed the Mac's source
  through the server alone.
- `5b.txt`: `omt recv` by name through the server alone connected to 172.16.80.64 and
  received 30 fps with 0 decode errors.
- A plain mDNS `omt list` did not show the source. That is correct: with a server, a
  source is announced to the server only (S1).

## Not covered

- libomtnet, vMix, OBS, a Pi.
- Windows.
- A wired network, or a network that spans more than one subnet.
- The limits here are covered by unit tests only, not across the network: receiver
  queue, sender connection caps, server caps, and removal by the owner only.
