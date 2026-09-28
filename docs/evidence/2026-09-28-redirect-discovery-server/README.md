# 2026-09-28 — redirect and the discovery server, between two machines

Follows `2026-09-28-cross-machine`. Same two machines:

- **A** `Sauls-MacBook-Pro` (172.16.80.64) — macOS 26.5.1, arm64
- **B** `punybz-NUC7i3BNHXF` (172.16.80.73) — Linux Mint 22.3, x86_64

Both `omt` v0.1.0 at commit `d5e74dd`. Wired LAN, same subnet. Both sides are our
own `omt` (not a third-party product). These two features had only ever been run
between our own senders/receivers on one Mac; here they cross the network.

## 1. Redirect across machines

A real source runs on B; a redirecting "portal" runs on A; a receiver connects to
the portal on A and should end up receiving from B.

- B: `omt send --name "Real"` → advertises `PUNYBZ-NUC7I3BNHXF (Real)`.
- A: `omt send --name "Portal" --redirect "PUNYBZ-NUC7I3BNHXF (Real)"`.
- A: `omt recv "SAULS-MACBOOK-PRO.LOCAL (Portal)"`.

The receiver's log:

```
connected (Video channel) to 172.16.80.64:6400        # the portal, on A
sender says: <OMTRedirect NewAddress="PUNYBZ-NUC7I3BNHXF (Real)" />
redirected to PUNYBZ-NUC7I3BNHXF (Real)
disconnected (Video channel): None — retrying
connected (Video channel) to 172.16.80.73:6400        # the real source, on B
   2.0s  video 640x360 30.00 fps | 26.0 fps received | audio 48000 Hz 2 ch | decode errors 0
```

The receiver read the redirect from the portal on A, **re-resolved the target name
over mDNS, and reconnected to the real source on B** (172.16.80.73), then decoded
its video with no errors. **Redirect resolves and is followed across the network.**

## 2. Discovery server across machines, no mDNS

The server runs on B; A's sender registers with it instead of announcing over mDNS;
a lister on A and a receiver on B find the sender through the server alone
(`--no-mdns`).

- B: `omt discovery-server --port 6399`.
- A: `omt send --name "SrvSend" --discovery-server omt://172.16.80.73:6399`.
- A: `omt list --discovery-server omt://172.16.80.73:6399 --no-mdns`.
- B: `omt recv "SAULS-MACBOOK-PRO.LOCAL (SrvSend)" --discovery-server omt://172.16.80.73:6399 --no-mdns`.

Server log (on B):

```
discovery server on port 6399. Ctrl-C to stop.
Connected: [::ffff:172.16.80.64]:61197
[::ffff:172.16.80.64]:61197 ADDED SAULS-MACBOOK-PRO.LOCAL (SrvSend) port 6400 [172.16.80.64]
Connected: [::ffff:172.16.80.73]:33348
```

`omt list --no-mdns` on A returned the one source, and `omt recv --no-mdns` on B
connected to A (172.16.80.64) through the server and received:

```
video 640x360 30.00 fps | ~30 fps received | audio 48000 Hz 2 ch | decode errors 0
```

**A sender registered with the server on one machine is discovered and received by
a client on another machine with mDNS switched off.**

## What this does not cover

vMix, OBS, SIENNA (untested). Windows, Wi-Fi and cross-subnet networks (this was one
wired subnet). Redirect chains longer than one hop across machines were not exercised
here (single-hop only).
