#!/bin/sh
# $1 = s2 (our sender as A) or s3 (libomtnet sender as A). B is libomtnet.
# A redirects to B at 5 s, clears at 11 s, redirects again at 18 s.
# R1 (libomtnet) watches A from 2 s; R2 (libomtnet) joins at 14 s, after the clear.
OUT=$SCRATCH/addressing-out
R=$SCRATCH/addressing-runs/$1
M=SAULS-MACBOOK-PRO.LOCAL
rm -rf $R; mkdir -p $R; cd $OUT/harness
tshark -i lo0 -f "tcp portrange 6400-6600" -w $R/all.pcapng -a duration:30 >/dev/null 2>$R/tshark.err &
sleep 2
./libomtnet-harness send addressing-D 27 > $R/libomtnet-send-B.txt 2>$R/libomtnet-send-B.log &
if [ "$1" = s2 ]; then
  $OUT/omt-send addressing-C 27 "5=$M (addressing-D)" 11= "18=$M (addressing-D)" > $R/omt-send-A.txt 2>&1 &
else
  ./libomtnet-harness send addressing-C 27 "5=$M (addressing-D)" 11= "18=$M (addressing-D)" > $R/libomtnet-send-A.txt 2>$R/libomtnet-send-A.log &
fi
sleep 2
./libomtnet-harness recv "$M (addressing-C)" 23 - > $R/libomtnet-recv-1.txt 2>$R/libomtnet-recv-1.log &
sleep 12
./libomtnet-harness recv "$M (addressing-C)" 11 - > $R/libomtnet-recv-2.txt 2>$R/libomtnet-recv-2.log &
wait
