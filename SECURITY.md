# Security

## OMT is a trusted-LAN protocol

Open Media Transport, like the reference implementation (libomtnet) and the
products that speak it (vMix, OBS with the OMT plugin, SIENNA), has **no
encryption and no authentication**. Anyone who can reach a sender's TCP port can
subscribe to its video and audio; anyone on the multicast domain sees every
mDNS announcement; a discovery server trusts what its clients tell it. This is by
design: OMT moves uncompressed-quality live video over a local network, and its
security model is the network's. Run it only on a network you trust, the way you
would NDI. Do not expose OMT ports to the internet.

This crate keeps that model — it interoperates with libomtnet by default — and
does **not** add authentication or change the wire format. What it adds is
resistance to a peer on the same LAN misbehaving, accidentally or on purpose, so
one bad actor cannot take the host down or steer it. Each mitigation is on by
default and can be relaxed.

## Threat model

In scope: a host or program on the trusted LAN that sends malformed or abusive
traffic — to exhaust memory, threads or CPU, to make a receiver connect
somewhere it should not, or to disrupt discovery for everyone else.

Out of scope: eavesdropping, tampering and impersonation on the wire (there is no
transport security), and anything off the trusted LAN. An attacker who is already
on your video LAN can send you video; that is what the LAN boundary is for.

## Mitigations in this crate

- **Bounded receiver queue.** A receiver caps the bytes of decoded frames waiting
  for the caller (`ReceiverConfig::max_queued_bytes`, default 64 MiB) and drops
  frames past it rather than growing without limit. A fast sender against a slow
  reader had reached 3 GB in 1.2 s.

- **Redirect policy.** A receiver follows a redirect only to a loopback or
  private / link-local address by default (`ReceiverConfig::redirect_policy` =
  `LocalOnly`), so a sender cannot steer receivers at an arbitrary public host or
  DNS name. `RedirectPolicy::Any` restores libomtnet's follow-anything behaviour.

- **Sender connection caps.** A sender refuses connections past
  `SenderConfig::max_connections` (default 64) and
  `max_connections_per_ip` (default 8), before spawning their threads, so a flood
  of connections cannot exhaust the host (500 idle sockets had made 1,002
  threads).

- **Discovery-server limits.** The server accepts at most
  `MAX_ENTRIES_PER_PEER` (64) sources per connection and `MAX_ENTRIES_TOTAL`
  (4096) in all, and lets a connection withdraw only the sources it registered,
  so one client cannot flood the table or remove another's sources.

- **Framing limits and fuzzing.** The deframer enforces per-channel maximum
  frame sizes, and the deframer, command matching and the VMX decoder are fuzzed.

## Known gaps (tracked, not yet done)

- No timeout yet to evict a sender connection that connects but never subscribes;
  the connection caps bound the damage in the meantime.
- The discovery server still writes to peers while holding the table lock, so a
  slow client can stall others for up to the write timeout; a per-peer outbox is
  the planned fix.

## Reporting

This is early software with no security guarantees. If you find a problem, open an
issue on the repository (there is no private channel yet). Do not include a working
exploit for anything beyond a trusted-LAN denial of service.
