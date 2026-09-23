#!/bin/sh
# Re-resolve: libomtnet sender "addressing-M" runs 7 s and exits; its port is then held
# by a plain listener, and a new "addressing-M" starts on another port. Our receiver,
# given the name, must come back.
OUT=$SCRATCH/addressing-out
R=$SCRATCH/addressing-runs/s4
M=SAULS-MACBOOK-PRO.LOCAL
rm -rf $R; mkdir -p $R; cd $OUT/harness
dns-sd -L "$M (addressing-M)" _omt._tcp local > $R/dns-sd-L.txt 2>&1 &
DNSSD=$!
./libomtnet-harness send addressing-M 7 > $R/libomtnet-send-1.txt 2>$R/libomtnet-send-1.log &
sleep 2
$OUT/omt-recv "$M (addressing-M)" 18 > $R/omt-recv.txt 2>&1 &
sleep 5.5
P1=$(sed -n 's/.*port=\([0-9]*\).*/\1/p' $R/libomtnet-send-1.txt)
python3 -c "
import socket,time
s=socket.socket(socket.AF_INET6); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0); s.bind(('::', $P1)); s.listen(1)
print('blocker holds port', $P1, flush=True); time.sleep(14)" > $R/blocker.txt 2>&1 &
sleep 2
./libomtnet-harness send addressing-M 10 > $R/libomtnet-send-2.txt 2>$R/libomtnet-send-2.log &
sleep 12.5
kill $DNSSD
wait
