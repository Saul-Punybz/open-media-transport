# Security

## OMT is a trusted-LAN protocol

Open Media Transport has no encryption and no authentication, like NDI on a
LAN. libomtnet, the reference implementation, has none either: no passwords,
no TLS and no access lists. This crate speaks the same protocol, so it has the
same limits. Anyone who can reach the network can:

- watch any stream, because video, audio and metadata travel in clear text over TCP;
- announce a source under any name, over mDNS or to a discovery server;
- send a receiver a redirect to another source;
- set a sender's tally (the on-air light) and its suggested quality.

So:

- **Keep OMT on a production network** that you control: its own VLAN, or behind a
  firewall.
- **Never expose OMT ports to the internet**: TCP 6400–6600 for senders, and TCP 6399
  for a discovery server. Only UDP 5353 (mDNS) is needed besides.
- **Bind senders to the production interface** when the machine has others:
  `SenderConfig::bind`, and `DiscoveryConfig::interfaces` for mDNS.
- **Across sites**, carry OMT over a VPN such as WireGuard, or send the picture another
  way (SRT with a passphrase, WebRTC, HLS).

## What this crate limits

These limits do not change what goes over the wire, so libomtnet, vMix and OBS see the
same protocol. Each one is tested.

| Where | Limit | Default | libomtnet |
|---|---|---|---|
| Receiver | Frames waiting for the application: video, audio, metadata. A frame that arrives when its kind is full is dropped and counted (`ChannelStats::dropped`). | 4 / 10 / 60 | same pool sizes |
| Receiver | Frame size: video, and audio or metadata | 10 MiB / 1 MiB | same |
| Receiver | Which redirects to follow (`ReceiverConfig::redirects`): `Any`, `SameHost` or `Never`. With `SameHost`, the target must be on the machine the original sender was reached at. A refused redirect leaves the receiver on the original sender. | `SameHost` | follows any |
| Sender | Connections (`max_connections`), and connections from one IP (`max_connections_per_ip`). Extra ones are closed at once. | 256 / 64 | none |
| Sender | A connection that has not subscribed to anything is closed after `SUBSCRIBE_TIMEOUT` | 5 s | never |
| Sender | Frames queued per connection. Beyond this a slow receiver loses frames, and the sender never blocks. | 4 media, 64 metadata | same |
| Discovery server | Only the connection that registered a source can remove it | — | anyone can |
| Discovery server | Connections, sources per connection, sources in all (`ServerConfig`), and full-name length | 512 / 256 / 4096 / 1024 bytes | none |
| Discovery server | Each client has its own writer and queue; a client that stops reading is dropped, and the others are not delayed | — | one slow client delays all |
| Discovery client | Writes time out | 2 s | — |
| Decoder | Frame dimensions | 7680×4320 | — |

The command-line tool `omt recv` follows **any** redirect by default, as libomtnet does,
because it is a test tool. Pass `--redirects same-host` or `--redirects never` to change
that.

## What this crate does not do

- **No encryption or authentication.** An opt-in encrypted transport between this
  crate's own peers is planned. It will use a separate port and leave plain OMT as the
  default.
- **Tally and quality commands** are accepted from any connection, as in libomtnet. One
  receiver can therefore light the tally, or raise the encoder quality for everyone.
- **mDNS names are not verified.** A source announced with someone else's name is found
  like the real one.

## Reporting a vulnerability

Report it privately through GitHub: on the repository, **Security → Report a
vulnerability**. Do not open a public issue. If that is not possible, write to the
maintainer at the address in the commit history.
