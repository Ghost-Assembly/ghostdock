#!/usr/bin/env bash
# GhostDock's own CPU while sampling 40 containers. Budget: 2 % of one core.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PORT=18585
DATA="$(mktemp -d)"
cleanup() {
  ss -lptnH "sport = :$PORT" | grep -o 'pid=[0-9]*' | cut -d= -f2 | xargs -r kill || true
  for i in $(seq 1 40); do docker rm -f "ghostdock-cost-$i" >/dev/null 2>&1 || true; done
  rm -rf "$DATA"
}
trap cleanup EXIT
for i in $(seq 1 40); do
  docker run -d --name "ghostdock-cost-$i" --label com.docker.compose.project=ghostdock-cost --label com.docker.compose.service=sleeper alpine:3.22 sleep 3600 >/dev/null
done
GHOSTDOCK_DATA_DIR="$DATA" GHOSTDOCK_BIND="127.0.0.1:$PORT" RUST_LOG=warn "$ROOT/target/release/ghostdock" >/dev/null 2>&1 &
PID=$!
sleep 90
# A low figure means nothing unless all 40 are being sampled.
JAR="$DATA/cookies"
curl -fsS -c "$JAR" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"correct horse battery staple"}' \
  "http://127.0.0.1:$PORT/api/v1/auth/bootstrap" >/dev/null
SEEN=$(curl -fsS -b "$JAR" "http://127.0.0.1:$PORT/api/v1/hosts/1/metrics/now" | grep -o '"key":"ghostdock-cost-[0-9]*"' | wc -l)
echo "sampling $SEEN of 40 containers"
[ "$SEEN" -ge 40 ]
ticks() { awk '{print $14 + $15}' "/proc/$PID/stat"; }
HZ=$(getconf CLK_TCK)
A=$(ticks); sleep 60; B=$(ticks)
PCT=$(awk -v a="$A" -v b="$B" -v hz="$HZ" 'BEGIN { printf "%.2f", (b - a) / hz / 60 * 100 }')
echo "GhostDock used ${PCT}% of one core sampling 40 containers"
awk -v p="$PCT" 'BEGIN { exit !(p <= 2.0) }'
