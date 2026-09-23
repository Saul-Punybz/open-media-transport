# Prints every OMT metadata frame in a pcap's TCP payloads, per stream and direction.
# Reassembles each direction of each stream before splitting frames (16-byte header,
# DataLength at offset 12, docs/PROTOCOL.md §3.1).
import subprocess, sys
rows = subprocess.run(["tshark","-r",sys.argv[1],"-Y","tcp.len>0","-T","fields",
    "-e","frame.time_relative","-e","tcp.stream","-e","tcp.srcport","-e","tcp.dstport","-e","tcp.payload"],
    capture_output=True, text=True).stdout.splitlines()
buf = {}
for r in rows:
    t, st, sp, dp, pl = r.split("\t")
    k = (st, sp, dp)
    b = buf.get(k, b"") + bytes.fromhex(pl.replace(":", ""))
    while len(b) >= 16:
        n = int.from_bytes(b[12:16], "little")
        if len(b) < 16 + n: break
        hdr, data = b[:16], b[16:16+n]
        b = b[16+n:]
        who = "server->client" if sp == "6399" else "client->server"
        print(f"{float(t):8.3f} stream{st} {sp}->{dp} {who} ver={hdr[0]} type={hdr[1]} ts={int.from_bytes(hdr[2:10],'little')} len={n}")
        print("    " + repr(data)[2:-1])
    buf[k] = b
