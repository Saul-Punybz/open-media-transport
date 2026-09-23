#!/bin/sh
# omt send --10bit watched by libomtnet's receiver (preferring
# UYVYorUYVAorP216orPA16) and by omt recv --snapshot .png; then libomtnet's
# 10-bit and 8-bit senders snapshotted by omt recv. Usage: snapshots.sh BIN_DIR
set -u
BIN=$1; H=$BIN/harness/libomtnet-harness
"$BIN/omt" send --name decode-10bit --size 640x360 --seconds 12 --10bit > omt-send-10bit.txt 2>&1 &
SP=$!
sleep 2
PORT=$(grep -o 'on port [0-9]*' omt-send-10bit.txt | grep -o '[0-9]*$')
"$H" recv "omt://127.0.0.1:$PORT" 5 - UYVYorUYVAorP216orPA16 > omt-send-10bit-libomtnet-recv.txt 2>&1 &
A=$!
"$BIN/omt" recv "127.0.0.1:$PORT" --seconds 5 --snapshot snap-omt-send-10bit.png > omt-send-10bit-omt-recv.txt 2>&1
wait $A
"$BIN/omt" recv "127.0.0.1:$PORT" --seconds 3 --snapshot snap-omt-send-10bit.bmp > omt-send-10bit-omt-recv-bmp.txt 2>&1
wait $SP
for src in p216 uyvy; do
  "$H" send "decode-snap-$src" 8 "$src" > "send-snap-$src.txt" 2>&1 &
  SP=$!
  sleep 3
  PORT=$(grep -o 'port=[0-9]*' "send-snap-$src.txt" | head -1 | cut -d= -f2)
  ext=png; [ $src = uyvy ] && ext=bmp
  "$BIN/omt" recv "127.0.0.1:$PORT" --seconds 3 --snapshot "snap-libomtnet-$src.$ext" > "snap-libomtnet-$src-omt-recv.txt" 2>&1
  wait $SP
done
