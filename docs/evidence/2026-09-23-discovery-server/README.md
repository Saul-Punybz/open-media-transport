# Discovery server (§10): ours against libomtnet, both ways

Date: 23 Sep 2026. Host: macOS, Apple Silicon, .NET SDK 10.0.401, one Mac.
Tools outside Claude: `tshark`, libomtnet 1.0.0.19 (`029ef4e`) through
`interop/libomtnet-harness`, and upstream's `OMTDiscoveryServer` program built unchanged
through `interop/upstream-discovery-server`.

## What ran

`run.sh MODE DIR 192.0.0.2` (in this directory) twice:

- `upstream/` — server: upstream `OMTDiscoveryServer` (port 6399).
- `ours/` — server: `omt discovery-server` (port 6399).

Everything else is the same in both runs. Clients reach the server at `omt://192.0.0.2:6399`,
the Mac's en0 address, so the server sees `192.0.0.2` as their source address while they
themselves send loopback; that makes S4 visible. (Traffic to one's own address goes over
`lo0` on macOS, where `tshark` captured it.) libomtnet reads the server from
`settings.xml` in `$OMT_STORAGE_PATH` (`OMTSettings.cs:35-37,62`, `mac/MacPlatform.cs:75-78`),
so no file in the user's home was written. The file used is `*/settings.xml`.

Sequence (`*/steps.log`):

1. server starts;
2. our sender `omt send --name discovery-server-ours --discovery-server omt://192.0.0.2:6399` registers;
3. libomtnet `list` (connects after 2) — must see our sender;
4. libomtnet `recv "SAULS-MACBOOK-PRO.LOCAL (discovery-server-ours)"` — must find it by name through the server and receive;
5. libomtnet `send 'discovery-server-lib&<x>'` registers (a name with XML special characters);
6. our `omt list --discovery-server … --no-mdns`, then our `omt recv "SAULS-MACBOOK-PRO.LOCAL (discovery-server-lib&<x>)" --discovery-server … --no-mdns`;
7. the server is killed and restarted while both senders run; libomtnet `list` and our `list` again;
8. the libomtnet sender exits; our `list` again; our sender exits; server stops.

`*/server-6399.pcapng` is the whole TCP port-6399 capture; `*/server-6399.frames.txt` is every
OMT frame in it, per stream and direction, made with `decode.py`. `*/mdns-en0.pcapng` is
all mDNS on en0 during the run. `*.stderr.txt` are libomtnet's own log lines.

## Results (identical in both runs unless noted)

| What | Seen |
|---|---|
| (a) our client → upstream server → libomtnet | libomtnet `list` shows `SAULS-MACBOOK-PRO.LOCAL (discovery-server-ours)`; libomtnet `recv` by that name received 60 video + 59 audio frames (`upstream/libomtnet-*.log`) |
| (b) libomtnet clients → our server | the same with our server (`ours/libomtnet-*.log`); libomtnet logs `NewFromServer`, `NewIP … ::ffff:192.0.0.2` |
| our client finds libomtnet's sender | `omt list --no-mdns` shows `… (discovery-server-lib&<x>) 192.0.0.2:6401`; `omt recv` by that name gets 30 fps video and audio, against either server |
| S2 | Every client connection sends exactly `<OMTSubscribe Metadata="true" />` and then `<OMTAddress>` frames; the server sends `<OMTTally Preview="false" Program=="false" />` on accept, then `<OMTAddress>` frames. All metadata frames, timestamp 0, no NUL |
| S3 | Exact bytes, from libomtnet's client and from ours alike: `<OMTAddress>\n  <Name>…</Name>\n  <Port>6400</Port>\n  <Addresses>\n    <IPAddress>::ffff:127.0.0.1</IPAddress>\n  </Addresses>\n</OMTAddress>`; a removal adds `  <Removed>True</Removed>\n` after `Port`. Text escaping is `&amp;`, `&lt;`, `&gt;` (`discovery-server-lib&amp;&lt;x&gt;`). The set of distinct client→server payloads is byte-identical between the two runs |
| S4 | Clients send `::ffff:127.0.0.1`; both servers send `::ffff:192.0.0.2`, the TCP source address |
| S5 | Each add and remove goes to every connection including the sender's own (e.g. `upstream` stream 3 gets its own add at 8.2 s); new connections get the table; the libomtnet sender's exit produced a `Removed` to the others. Distinct server→client payloads are byte-identical between the two servers, except that ours also delivered the echo of our sender's final removal, which upstream's timing missed because that client had already closed |
| S6 | After the server restart both clients reconnected and re-registered: both `list`s after restart show both sources; libomtnet logs `Disconnected from server`, `Connected to server`, then `NewFromServer` again (`ours/libomtnet-send.stderr.txt`). libomtnet's re-registration carries two addresses, `::ffff:127.0.0.1` and `::ffff:192.0.0.2`: the echo of its own source was merged into its registered entry (`OMTDiscovery.cs:218-243`); the server discards both anyway (S4) |
| S1 | Neither sender appears in `mdns-en0.pcapng` (8 mDNS packets captured, none naming `discovery-server`): with a server configured, neither libomtnet nor our sender announces over DNS-SD |
| Table timing | Upstream sent the table 100–190 ms after a new client's subscription (`upstream/server-6399.frames.txt`); nothing in its code orders the two (module docs of `discovery_server.rs`). Ours sends it on the subscription, 0 ms |

## Not shown here

Anything across two machines, on Windows or Linux, with IPv6 clients, or with vMix/OBS
using a discovery server. Only one Mac, loopback routing.
