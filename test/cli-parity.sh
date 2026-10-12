#!/usr/bin/env bash
# Runs the same command lines through the Go and the Rust binaries and prints
# every difference in stdout, stderr or exit status.
#
#   GO_RVW=/path/to/go/rvw RVW=/path/to/rust/rvw bash test/cli-parity.sh
#
# Each binary gets its own scratch database, HOME and XDG_DATA_HOME, and the
# same git repository as its workspace. Cases run in order, so both databases
# see the same history and assign the same comment ids. `--db` comes first,
# before the case's words.
set -u

here=$(cd "$(dirname "$0")" && pwd)
GO_RVW=${GO_RVW:?set GO_RVW to the Go rvw binary}
RVW=${RVW:-$here/../target/debug/rvw}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

mk_env() { # $1: side name
	mkdir -p "$tmp/$1/home" "$tmp/$1/xdg" "$tmp/$1/repo"
	git -C "$tmp/$1/repo" init -q
	printf 'alpha\nbeta\ngamma\n' >"$tmp/$1/repo/a.txt"
}
mk_env go
mk_env rust

differences=0
cases=0

# case NAME ARGS...: runs both binaries and compares what they print.
case_run() {
	local name=$1
	shift
	cases=$((cases + 1))
	local side bin
	for side in go rust; do
		if [ "$side" = go ]; then bin=$GO_RVW; else bin=$RVW; fi
		local dir=$tmp/$side
		local full=(--db "$dir/rvw.db" "$@")
		(
			cd "$dir/repo" || exit 99
			export HOME=$dir/home XDG_DATA_HOME=$dir/xdg USER=tester
			"$bin" "${full[@]}" >"$dir/$name.out" 2>"$dir/$name.err" </dev/null
			echo $? >"$dir/$name.code"
		)
		# The two repositories live in different directories; say so once.
		sed -i "s#$tmp/rust#$tmp/go#g" "$dir/$name.out" "$dir/$name.err"
	done
	local what
	for what in out err code; do
		if ! diff -u "$tmp/go/$name.$what" "$tmp/rust/$name.$what" >"$tmp/d"; then
			differences=$((differences + 1))
			echo "--- $name: rvw $* ($what)"
			sed -n '3,$p' "$tmp/d" | head -n 20
		fi
	done
}

# Lines are read from the cases table below; each line is one invocation.
run_cases() {
	local line
	while IFS= read -r line; do
		[ -z "$line" ] && continue
		case "$line" in \#*) continue ;; esac
		# shellcheck disable=SC2086
		eval "set -- $line"
		case_run "c$cases" "$@"
	done
}

run_cases <<'CASES'
# unknown flags, commands and arguments
--nope
-x
x
version
add
add --file a.txt
show
show a b
show -r3
show -- -r3
list -x
list --limit 3
pull --limit abc
pull --limit
pull --limit 0
pull --format bad
list --status bad
mcp bogus
mcp serve --bogus
tui x
path x
count a
workspaces extra
add --file a.txt --lines 1 --comment hi extra
--db
--workspace /nonexistent list
# hyphen-leading values
add --file a.txt --lines 1 --comment "-x is wrong" --format ids
add --file a.txt --lines 2 --comment -- --format ids
add --file a.txt --lines 3 --comment "--" --author -bob --format ids
add --file a.txt --lines 2-3 --comment-file - --format ids
edit r1 --comment "-y" --format json
resolve r1 --note -
resolve r2 --note "-z"
reject r3 --note "--"
show -- r2
list --file -weird.txt
pull --id -r9 --peek
# argument count, order of checks, and value forms
show --format bad
show a b --format bad
show r1 r2 r3
reject --note x
reject a b
reject r1
resolve --note -
resolve r1 --note
list --status bad --reviews
submit --decision bad --summary x --no-comments
submit --summary x
submit --no-comments --id r1 --decision comment --summary x
add --file a.txt --lines 9-1 --comment x
add --file a.txt --lines=1 --comment=-x --format=ids
add --file a.txt --lines 1
add --file missing.txt --lines 1 --comment x
add --file a.txt --lines 1 --code-file nope --comment x
list extra
pull --format json --limit 0
mcp config --scope
mcp config --format bad
tui --leader ab
tui --leader -
help add
add --help
help
mcp
--help
show -h
help nope
CASES

echo "$cases cases, $differences difference(s)"
[ "$differences" -eq 0 ]
