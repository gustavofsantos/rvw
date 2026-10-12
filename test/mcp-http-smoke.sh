#!/usr/bin/env bash
# Smoke test for `rvw mcp serve --http`: starts the server on an ephemeral
# loopback port, drives initialize, notifications/initialized and tools/list
# over streamable HTTP with curl, expects the eleven tools, then sends SIGTERM
# and expects the process to exit within two seconds.
#
# Usage: test/mcp-http-smoke.sh   (RVW=path/to/rvw overrides target/debug/rvw)
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
rvw=${RVW:-$root/target/debug/rvw}
work=$(mktemp -d)
pid=""

cleanup() {
  if [ -n "$pid" ]; then kill -9 "$pid" 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT

fail() {
  echo "mcp-http-smoke: FAIL: $*" >&2
  [ -f "$work/err.log" ] && sed 's/^/  stderr: /' "$work/err.log" >&2
  exit 1
}

[ -x "$rvw" ] || fail "$rvw is not built (cargo build -p rvw-cli)"
command -v curl >/dev/null || fail "curl is required"
command -v jq >/dev/null || fail "jq is required"

# A git repo and a scratch database: the run never touches the real queue.
git -C "$work" init -q proj
export XDG_DATA_HOME="$work/data"

# exec makes the backgrounded subshell become rvw, so $! is the server's pid.
(cd "$work/proj" && exec "$rvw" --db "$work/q.db" mcp serve --http 127.0.0.1:0 \
  >"$work/out.log" 2>"$work/err.log") &
pid=$!

# The server prints its bound address once the socket is listening.
url=""
for _ in $(seq 100); do
  url=$(sed -n 's|.*serving MCP at \(http://[^ ]*\).*|\1|p' "$work/err.log" 2>/dev/null | head -n 1)
  [ -n "$url" ] && break
  kill -0 "$pid" 2>/dev/null || fail "server exited before listening"
  sleep 0.1
done
[ -n "$url" ] || fail "no 'serving MCP at' line within 10s"
case "$url" in
  http://127.0.0.1:[0-9]*/mcp) ;;
  *) fail "bound to an unexpected address: $url" ;;
esac

# post BODY [SESSION] prints the HTTP status and leaves the JSON-RPC message
# in $work/json. The server may frame it as one SSE event; unwrap that.
post() {
  local session_args=() status
  if [ -n "${2:-}" ]; then session_args=(-H "Mcp-Session-Id: $2"); fi
  status=$(curl -sS -o "$work/body" -D "$work/headers" -w '%{http_code}' \
    -H 'Accept: application/json, text/event-stream' \
    -H 'Content-Type: application/json' \
    "${session_args[@]}" --data "$1" "$url")
  if grep -q '^data: ' "$work/body"; then
    sed -n 's/^data: //p' "$work/body" | tail -n 1 >"$work/json"
  else
    cp "$work/body" "$work/json"
  fi
  echo "$status"
}

status=$(post '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}')
[ "$status" = 200 ] || fail "initialize returned HTTP $status: $(cat "$work/json")"
session=$(tr -d '\r' <"$work/headers" | sed -n 's/^[Mm]cp-[Ss]ession-[Ii]d: //p' | head -n 1)
jq -e '.result.serverInfo.name == "rvw"' "$work/json" >/dev/null || fail "unexpected initialize result: $(cat "$work/json")"

status=$(post '{"jsonrpc":"2.0","method":"notifications/initialized"}' "$session")
case "$status" in 200|202) ;; *) fail "notifications/initialized returned HTTP $status" ;; esac

status=$(post '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' "$session")
[ "$status" = 200 ] || fail "tools/list returned HTTP $status: $(cat "$work/json")"
names=$(jq -r '.result.tools[].name' "$work/json" | sort | tr '\n' ' ')
count=$(jq -r '.result.tools | length' "$work/json")
[ "$count" = 11 ] || fail "tools/list returned $count tools, want 11: $names"
echo "mcp-http-smoke: 11 tools at $url: $names"

# SIGTERM must stop the server. A zombie counts as stopped.
kill -TERM "$pid"
stopped=""
for _ in $(seq 20); do
  state=$(ps -o stat= -p "$pid" 2>/dev/null || true)
  if [ -z "$state" ] || [ "${state#Z}" != "$state" ]; then stopped=1; break; fi
  sleep 0.1
done
[ -n "$stopped" ] || fail "server still running 2s after SIGTERM"
set +e
wait "$pid"
exit_status=$?
set -e
pid=""
[ "$exit_status" -eq 0 ] || fail "server exited with status $exit_status after SIGTERM"
echo "mcp-http-smoke: PASS"
