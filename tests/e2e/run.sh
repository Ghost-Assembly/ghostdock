#!/usr/bin/env bash
# Runs the browser tests, each against a freshly started GhostDock.
#
#   tests/e2e/run.sh              every test
#   tests/e2e/run.sh deploy git   just those
#
# Each test assumes a database in a particular state -- most want a brand
# new one, since they exercise first-run setup -- so a server is started
# per test rather than shared. Sharing would make the order matter and a
# failure in one test would corrupt the next.
#
# Needs: a built server (target/release/ghostdock), a built web client
# (crates/web/dist), a reachable Docker daemon, git, and node.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PORT="${GHOSTDOCK_E2E_PORT:-18181}"
URL="http://127.0.0.1:${PORT}"
WORK="$(mktemp -d)"
PASSWORD='correct horse battery staple'
SERVER_PID=""

export GHOSTDOCK_URL="$URL"
export SHOTS="${SHOTS:-$ROOT/target/e2e-shots}"
mkdir -p "$SHOTS"

# Only what these tests create. Other containers on the host are not this
# script's business.
PROJECTS=(demo-app broken blog shellbox livebox ticker chatter ghostdock-e2e-multilogs
  ghostdock-e2e-icons-web ghostdock-e2e-icons-plain)

# Stopped containers the cleanup test creates for itself to remove.
CLEANUP_FIXTURES=(ghostdock-e2e-gone-web-1 ghostdock-e2e-by-hand)

remove_test_containers() {
  for project in "${PROJECTS[@]}"; do
    docker compose -p "$project" down -v >/dev/null 2>&1 || true
  done
  docker rm -f "${CLEANUP_FIXTURES[@]}" >/dev/null 2>&1 || true
}

cleanup() {
  stop_server
  remove_test_containers
  rm -rf "$WORK"
}
trap cleanup EXIT

stop_server() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID"
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  SERVER_PID=""
}

start_fresh() {
  stop_server
  # A fresh host as well as a fresh database. Containers left by one test
  # otherwise appear on the next test's board -- and an unhealthy one sorts
  # first, which is how this was found.
  remove_test_containers
  local data="$WORK/data-$1"
  rm -rf "$data" && mkdir -p "$data"
  GHOSTDOCK_DATA_DIR="$data" \
  GHOSTDOCK_UI_DIR="$ROOT/crates/web/dist" \
  GHOSTDOCK_BIND="127.0.0.1:${PORT}" \
  RUST_LOG=warn \
    "$ROOT/target/release/ghostdock" >"$WORK/server-$1.log" 2>&1 &
  SERVER_PID=$!

  for _ in $(seq 1 80); do
    if curl -fsS "$URL/api/v1/health" >/dev/null 2>&1; then return 0; fi
    sleep 0.25
  done
  echo "GhostDock did not start for $1:" >&2
  cat "$WORK/server-$1.log" >&2
  return 1
}

# An authenticated session for setting up preconditions through the API.
api_session() {
  local jar="$WORK/cookies"
  rm -f "$jar"
  local body="{\"username\":\"admin\",\"password\":\"$PASSWORD\"}"
  # The first call sets the instance up; later ones sign in.
  curl -fsS -c "$jar" -H 'content-type: application/json' -d "$body" \
    "$URL/api/v1/auth/bootstrap" >/dev/null 2>&1 \
    || curl -fsS -c "$jar" -H 'content-type: application/json' -d "$body" \
      "$URL/api/v1/auth/login" >/dev/null
  echo "$jar"
}

run() {
  echo
  echo "=== $1 ==="
  (cd "$ROOT" && node "tests/e2e/$1.mjs")
}

test_smoke()  { start_fresh smoke;  run smoke; }
test_perf()   { start_fresh perf;   run perf; }
test_deploy() { start_fresh deploy; run deploy; }
test_accounts() { start_fresh accounts; run accounts; }
test_tokens() { start_fresh tokens; run tokens; }
test_stall() { start_fresh stall; run stall; }
test_offline() { start_fresh offline; run offline; }

test_git() {
  # A real repository for the Git test.
  REPO="$WORK/origin"
  rm -rf "$REPO" && mkdir -p "$REPO/compose"
  cat >"$REPO/compose/blog.yml" <<'YML'
services:
  web:
    image: alpine:3.22
    command: ["sh", "-c", "echo \"greeting=$GREETING\"; while :; do sleep 3600; done"]
    environment:
      GREETING: ${GREETING:-unset}
    healthcheck:
      test: ["CMD", "true"]
      interval: 1s
      retries: 2
YML
  git -C "$REPO" init -q --initial-branch main
  git -C "$REPO" add -A
  git -C "$REPO" -c user.email=e2e@example.invalid -c user.name=e2e commit -qm "blog stack"
  start_fresh git
  GHOSTDOCK_TEST_REPO="$REPO" run git
}

# Registers an inline stack through the API and deploys it, for tests that
# need something already running. Usage: deploy_fixture NAME YAML
deploy_fixture() {
  local jar status body
  jar="$(api_session)"
  body="$(jq -n --arg name "$1" --arg yaml "$2" '{name: $name, compose_yaml: $yaml}')"
  curl -fsS -b "$jar" -H 'content-type: application/json' -d "$body" \
    "$URL/api/v1/hosts/1/stacks" >/dev/null
  curl -fsS -b "$jar" -X POST "$URL/api/v1/stacks/1/deploy" >/dev/null
  for _ in $(seq 1 90); do
    status="$(curl -fsS -b "$jar" "$URL/api/v1/deployments/1" | jq -r .status)"
    [[ "$status" != "running" ]] && break
    sleep 1
  done
  [[ "$status" == "succeeded" ]] || { echo "could not deploy the $1 fixture: $status" >&2; exit 1; }
}

SLEEPER='services:
  box:
    image: alpine:3.22
    command: ["sh", "-c", "while :; do sleep 3600; done"]
'

test_shell() {
  # The shell test needs a running stack to open a shell in.
  start_fresh shell
  deploy_fixture Shellbox "$SLEEPER"
  run shell
}

TICKER='services:
  ticker:
    image: alpine:3.22
    command: ["sh", "-c", "i=0; while :; do echo tick $$i; echo warn $$i >&2; i=$$((i+1)); sleep 1; done"]
'

test_logs() {
  start_fresh logs
  deploy_fixture Ticker "$TICKER"
  sleep 3
  run logs
}

test_cleanup() {
  start_fresh cleanup
  # One stopped container from a compose project that is gone, one made by
  # hand. Created, never started: stopped from the first moment.
  docker create --name ghostdock-e2e-gone-web-1 \
    --label com.docker.compose.project=ghostdock-e2e-gone alpine:3.22 >/dev/null
  docker create --name ghostdock-e2e-by-hand alpine:3.22 >/dev/null
  run cleanup
}

test_discover() {
  # A repository holding three stacks in the usual layouts.
  local repo="$WORK/many" jar
  rm -rf "$repo" && mkdir -p "$repo/compose" "$repo/apps/notes"
  printf '%s' "$SLEEPER" >"$repo/compose/alpha.yml"
  printf '%s' "$SLEEPER" >"$repo/compose/beta.yaml"
  printf '%s' "$SLEEPER" >"$repo/apps/notes/compose.yaml"
  git -C "$repo" init -q --initial-branch main
  git -C "$repo" add -A
  git -C "$repo" -c user.email=e2e@example.invalid -c user.name=e2e commit -qm "stacks"
  start_fresh discover
  jar="$(api_session)"
  curl -fsS -b "$jar" -H 'content-type: application/json' \
    -d "{\"url\":\"file://$repo\",\"credential_id\":null}" "$URL/api/v1/repos" >/dev/null
  run discover
}

test_tabs() {
  start_fresh tabs
  deploy_fixture Livebox "$SLEEPER"
  run tabs
}

test_layout() {
  start_fresh layout
  deploy_fixture Livebox "$SLEEPER"
  local jar i
  jar="$(api_session)"
  for i in alpha beta gamma delta epsilon zeta eta theta; do
    curl -fsS -b "$jar" -H 'content-type: application/json' \
      -d "{\"name\":\"$i\",\"compose_yaml\":\"services: {}\\n\"}" "$URL/api/v1/hosts/1/stacks" >/dev/null
  done
  mkdir -p "$SHOTS/desktop" "$SHOTS/phone"
  run layout
}

test_mcp() {
  start_fresh mcp
  deploy_fixture Livebox "$SLEEPER"
  local jar
  jar="$(api_session)"
  GHOSTDOCK_COOKIE="$(awk '$6 == "ghostdock.sid" {print $6"="$7}' "$jar")" run mcp
}

test_roam() {
  start_fresh roam
  deploy_fixture Livebox "$SLEEPER"
  run roam
}

CHATTER='services:
  talk:
    image: alpine:3.22
    command: ["sh", "-c", "i=0; while :; do echo \"lorem ipsum $$i dolor sit amet, consectetur\"; i=$$((i+1)); [ $$((i % 20)) -eq 0 ] && sleep 0.1; done"]
'

test_jank() {
  start_fresh jank
  deploy_fixture Chatter "$CHATTER"
  # A full board: stacks registered but never deployed still get a row.
  local jar i
  jar="$(api_session)"
  for i in $(seq 1 40); do
    curl -fsS -b "$jar" -H 'content-type: application/json' \
      -d "{\"name\":\"Filler $i\",\"compose_yaml\":\"services: {}\\n\"}" \
      "$URL/api/v1/hosts/1/stacks" >/dev/null
  done
  run jank
}

# Two containers in one stack: one chatty on stdout, one slower that also
# writes to stderr.
MULTILOGS='services:
  north:
    image: alpine:3.22
    command: ["sh", "-c", "i=0; while :; do echo \"north line $$i\"; echo \"north warn $$i\" >&2; i=$$((i+1)); sleep 0.5; done"]
  south:
    image: alpine:3.22
    command: ["sh", "-c", "i=0; while :; do echo \"south line $$i lorem ipsum dolor\"; i=$$((i+1)); [ $$((i % 20)) -eq 0 ] && sleep 0.1; done"]
'

test_multilogs() {
  start_fresh multilogs
  deploy_fixture ghostdock-e2e-multilogs "$MULTILOGS"
  run multilogs
}

test_icons() {
  # Registers and deploys its own stacks through the API.
  start_fresh icons
  run icons
}

test_live() {
  # The live test changes a running stack from outside GhostDock.
  start_fresh live
  deploy_fixture Livebox "$SLEEPER"
  run live
}

test_a11y() {
  # Every main screen, with a stack running so each has something to show.
  start_fresh a11y
  deploy_fixture Ticker "$TICKER"
  sleep 3
  run a11y
}

test_host() {
  start_fresh host
  deploy_fixture Chatter "$CHATTER"
  # Two ticks, so there is a rate to show.
  sleep 12
  run host
}

# With no arguments, everything; otherwise just the named tests, in order.
TESTS=("$@")
[[ ${#TESTS[@]} -eq 0 ]] && TESTS=(smoke perf deploy accounts tokens offline stall git discover shell live logs multilogs icons cleanup roam tabs jank layout host a11y mcp)
for t in "${TESTS[@]}"; do
  "test_$t"
done

echo
echo "Browser tests passed: ${TESTS[*]}"
