#!/bin/sh
# Scenario 1: libomtnet sender A redirects to libomtnet sender B; our receiver follows and returns.
OUT=$SCRATCH/addressing-out
R=$SCRATCH/addressing-runs/s1
M=SAULS-MACBOOK-PRO.LOCAL
rm -rf $R; mkdir -p $R; cd $OUT/harness
tshark -i lo0 -f "tcp portrange 6400-6600" -w $R/all.pcapng -a duration:26 >/dev/null 2>$R/tshark.err &
sleep 2
./libomtnet-harness send addressing-B 24 > $R/libomtnet-send-B.txt 2>$R/libomtnet-send-B.log &
./libomtnet-harness send addressing-A 24 "5=$M (addressing-B)" 13= > $R/libomtnet-send-A.txt 2>$R/libomtnet-send-A.log &
sleep 2
$OUT/omt-recv "$M (addressing-A)" 19 > $R/omt-recv.txt 2>&1
wait
