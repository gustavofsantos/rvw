#!/usr/bin/env bash
# Compares `rvw mcp serve`'s initialize and tools/list with rvw's own recorded
# output (test/reference/mcp-tools-list.jsonl; the Go baseline is retired). The
# reference is the raw answer to initialize and tools/list from a scratch repo,
# so a diff means the MCP surface changed since the capture: re-record it the
# same way (see the pipeline below) and review the diff. Both sides are
# normalized first: server name and title, instructions verbatim, tool names
# in order, descriptions, readOnlyHint, and each input and output schema as
# property names, JSON types, descriptions and required sets, with $ref
# resolved. Slices compare as arrays whether or not "null" is advertised as a
# type (see types). $schema, title, additionalProperties, key order and format
# or bound extras are ignored. Exit 0 on a match; otherwise a diff and exit 1.
# test/reference/mcp-session.jsonl is rvw's replay of a longer session (calls
# and errors); no script reads it.
#
# Usage: test/mcp-compare.sh   (RVW=path/to/rvw overrides target/debug/rvw)
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
rvw=${RVW:-$root/target/debug/rvw}
ref=$root/test/reference/mcp-tools-list.jsonl
work=$(mktemp -d)
[ -n "${MCP_COMPARE_KEEP:-}" ] || trap 'rm -rf "$work"' EXIT

command -v jq >/dev/null || { echo "mcp-compare: jq is required" >&2; exit 2; }
[ -x "$rvw" ] || { echo "mcp-compare: $rvw is not built (cargo build -p rvw-cli)" >&2; exit 2; }

# A git repo and a scratch database, so the run never touches the real queue.
git -C "$work" init -q proj
export XDG_DATA_HOME="$work/data"

# Initialize, then tools/list once the first has been answered. The pauses
# keep the server from seeing tools/list before initialization.
{
  printf '%s\n' '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"mcp-compare","version":"1"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  sleep 0.5
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
  sleep 0.5
} | (cd "$work/proj" && "$rvw" mcp serve) >"$work/out.jsonl" || {
  echo "mcp-compare: $rvw mcp serve failed" >&2
  exit 1
}

# $root is the document holding $defs; references are resolved against it.
# Descriptions from a $ref target are dropped: a struct's doc comment is not a
# property description.
normalize='
# A slice may be advertised as ["array","null"]; the service returns [] for it
# either way, so arrays compare as "array".
def types($s):
  ($s.type // null)
  | if . == null then []
    elif type == "array" then (if any(. == "array") then ["array"] else sort end)
    else [.] end;
def resolve($root):
  if type == "object" and has("$ref")
  then $root["$defs"][(.["$ref"] | split("/") | last)] | resolve($root)
  else . end;
def shape($root):
  if type == "object" and has("anyOf") and (.anyOf | map(.type == "null") | any)
  then . as $w
    | (.anyOf | map(select(.type != "null")) | .[0]) as $alt
    | ($alt | shape($root)) | .type = ((.type + ["null"]) | unique) | .description = ($w.description // null)
  elif type == "object" and has("$ref")
  then resolve($root) | shape($root) | .description = null
  else . as $s
    | { type: types($s), description: ($s.description // null) }
      + (if $s | has("properties")
         then { required: (($s.required // []) | sort),
                properties: (($s.properties // {}) | with_entries(.value |= shape($root))) }
         else {} end)
      + (if $s | has("items") then { items: ($s.items | shape($root)) } else {} end)
  end;
def schema($s): if $s == null then null else $s | shape($s) end;
'

# The comparable view of an initialize result and a tools/list result.
view() {
  jq -S --slurp "$normalize"'
    . as $all
    | ($all[] | select(.id == 0).result) as $init
    | ($all[] | select(.id == 1).result) as $list
    | {
        server: { name: $init.serverInfo.name, title: $init.serverInfo.title },
        instructions: $init.instructions,
        tools: [ $list.tools[] | {
          name,
          description,
          readOnly: (.annotations.readOnlyHint // false),
          input: schema(.inputSchema),
          output: schema(.outputSchema)
        } ]
      }' "$1"
}

view "$ref" >"$work/ref.json"
view "$work/out.jsonl" >"$work/ours.json"

if diff -u "$work/ref.json" "$work/ours.json" >"$work/diff.txt"; then
  echo "mcp-compare: tools/list and initialize match the reference"
  exit 0
fi
echo "mcp-compare: differences from the reference (- reference, + rvw):"
diff -u "$work/ref.json" "$work/ours.json" || true
exit 1
