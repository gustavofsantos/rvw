#!/usr/bin/env bash
# Diffs `rvw <command> --help` against the captured Go output in
# test/reference/help/. File names map spaces to underscores: rvw_mcp_serve.txt
# is `rvw mcp serve --help`, and rvw_.txt is the root. Exits non-zero and lists
# every command whose help differs.
set -u

here=$(cd "$(dirname "$0")" && pwd)
RVW=${RVW:-$here/../target/debug/rvw}
ref=$here/reference/help
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

failed=()
for file in "$ref"/rvw_*.txt; do
	name=$(basename "$file" .txt)
	name=${name#rvw_}
	args=()
	if [ -n "$name" ]; then
		IFS=_ read -r -a args <<<"$name"
	fi
	if ! "$RVW" "${args[@]}" --help >"$tmp/out" 2>&1; then
		: # help exits 0; a failure shows up in the diff below
	fi
	if ! diff -u "$file" "$tmp/out" >"$tmp/diff"; then
		failed+=("rvw ${args[*]:-}")
		echo "--- differs: rvw ${args[*]:-} (reference: $(basename "$file"))"
		head -n 40 "$tmp/diff"
	fi
done

if [ ${#failed[@]} -gt 0 ]; then
	echo "help differs for ${#failed[@]} command(s): ${failed[*]}" >&2
	exit 1
fi
echo "help matches the reference for every command"
