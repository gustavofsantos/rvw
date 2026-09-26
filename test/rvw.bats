#!/usr/bin/env bats

# Tests for rvw — the per-workspace review comment queue. Ported from the
# `review` script's suite, which this binary replaces; each test pins a
# guarantee that an editor plugin or an agent leans on:
#   * a pulled comment LEAVES the queue — no agent works the same note twice
#   * ids are stable and never reused, so the (stateless) editor can act by id
#   * workspaces are isolated, and a git worktree/subdir resolves to its root;
#     a directory outside git is refused
#   * lane scoping is STRICT — one tree carries several branches at once
#     (GitButler), so a pinned pull must never swallow another lane's comments
#   * every comment ends in a recorded decision, attributed to whoever made it
#   * every failure is one `rvw: ...` line on stderr, exit 1
#
# Isolation strategy:
#   XDG_DATA_HOME → fresh tmpdir (the database lives under it)
#   WORKSPACE     → fresh tmpdir git repo, cwd for every run
#   USER=tester, the default author; every RVW_* / REVIEW_* variable is cleared

setup_file() {
  export RVW="$BATS_FILE_TMPDIR/rvw"
  (cd "$BATS_TEST_DIRNAME/.." && go build -o "$RVW" ./cmd/rvw)
}

setup() {
  # physical path: on macOS mktemp answers /var/..., which git reports as /private/var/...
  TEST_ROOT=$(cd "$(mktemp -d)" && pwd -P)
  for var in WORKSPACE LANE AUTHOR DB HOME; do
    unset "RVW_$var" "REVIEW_$var"
  done
  export XDG_DATA_HOME="$TEST_ROOT/data"
  export USER=tester
  DB="$XDG_DATA_HOME/rvw/rvw.db"
  WORKSPACE="$TEST_ROOT/proj"
  mkdir -p "$WORKSPACE"
  git -C "$WORKSPACE" init -q
  printf 'one\ntwo\nthree\nfour\n' >"$WORKSPACE/app.py"
  printf 'alpha\nbeta\n' >"$WORKSPACE/other.rb"
  cd "$WORKSPACE" || exit 1
}

teardown() {
  cd / || true
  rm -rf "$TEST_ROOT"
}

# Enqueue a comment. Args: file, lines, text
queue() {
  "$RVW" add --file "$1" --lines "$2" --comment "$3" --format ids </dev/null
}

# ── enqueue ───────────────────────────────────────────────────────────────────

@test "add: enqueues a comment and prints its id" {
  run queue app.py 1-2 "rename this"
  [ "$status" -eq 0 ]
  [ "$output" = "r1" ]

  run "$RVW" count
  [ "$output" = "1" ]
}

@test "add: snapshots exactly the requested lines and guesses the fence language" {
  queue app.py 2-3 "look here"
  run "$RVW" list --format json
  [ "$status" -eq 0 ]
  [ "$(jq -r '.reviews[0].code' <<<"$output")" = "$(printf 'two\nthree')" ]
  [ "$(jq -r '.reviews[0].filetype' <<<"$output")" = "python" ]
  [ "$(jq -r '.reviews[0].file' <<<"$output")" = "app.py" ]
}

@test "add: a tracked file keeps the exact reviewed version as a Git blob" {
  git add app.py
  printf 'working\ntree\nversion\n' >app.py

  run "$RVW" add --file app.py --lines 2 --comment "look here" --format json </dev/null
  [ "$status" -eq 0 ]
  file_version=$(jq -r '.file_version' <<<"$output")

  run git cat-file -t "$file_version"
  [ "$status" -eq 0 ]
  [ "$output" = "blob" ]

  run git cat-file blob "$file_version"
  [ "$status" -eq 0 ]
  [ "$output" = "$(printf 'working\ntree\nversion')" ]
}

@test "add: an untracked file has no file version" {
  run "$RVW" add --file app.py --lines 1 --comment "look here" --format json </dev/null
  [ "$status" -eq 0 ]
  [ "$(jq 'has("file_version")' <<<"$output")" = "false" ]
}

@test "add: a Git inspection failure cannot enqueue a versionless tracked review" {
  git add app.py
  mkdir -p "$TEST_ROOT/bin"
  ln -s "$(command -v git)" "$TEST_ROOT/bin/real-git"
  cat >"$TEST_ROOT/bin/git" <<'SH'
#!/bin/sh
if [ "$3" = "ls-files" ]; then
  echo "inspection unavailable" >&2
  exit 128
fi
exec "$(dirname "$0")/real-git" "$@"
SH
  chmod +x "$TEST_ROOT/bin/git"

  run env PATH="$TEST_ROOT/bin:$PATH" \
    "$RVW" add --file app.py --lines 1 --comment "look here" </dev/null
  [ "$status" -eq 1 ]
  [[ "$output" == "rvw: cannot inspect Git metadata for app.py:"* ]]

  run "$RVW" count
  [ "$output" = "0" ]
}

@test "add: a Git storage failure cannot enqueue a versionless tracked review" {
  git add app.py
  mkdir -p "$TEST_ROOT/bin"
  ln -s "$(command -v git)" "$TEST_ROOT/bin/real-git"
  cat >"$TEST_ROOT/bin/git" <<'SH'
#!/bin/sh
if [ "$3" = "hash-object" ]; then
  echo "storage unavailable" >&2
  exit 1
fi
exec "$(dirname "$0")/real-git" "$@"
SH
  chmod +x "$TEST_ROOT/bin/git"

  run env PATH="$TEST_ROOT/bin:$PATH" \
    "$RVW" add --file app.py --lines 1 --comment "look here" </dev/null
  [ "$status" -eq 1 ]
  [[ "$output" == "rvw: cannot store the reviewed version of app.py:"* ]]

  run "$RVW" count
  [ "$output" = "0" ]
}

@test "add: a single line is stored as a one-line range" {
  queue app.py 3 "typo"
  run "$RVW" list
  [[ "$output" == *"app.py:3"* ]]
}

@test "add: --code-file - snapshots the editor buffer, not what is on disk" {
  git add app.py
  printf 'UNSAVED-1\nUNSAVED-2\nUNSAVED-3\n' |
    "$RVW" add --file app.py --lines 2-3 --code-file - --comment "from the buffer"
  run "$RVW" list --format json
  [ "$(jq -r '.reviews[0].code' <<<"$output")" = "$(printf 'UNSAVED-2\nUNSAVED-3')" ]

  file_version=$(jq -r '.reviews[0].file_version' <<<"$output")
  run git cat-file blob "$file_version"
  [ "$status" -eq 0 ]
  [ "$output" = "$(printf 'UNSAVED-1\nUNSAVED-2\nUNSAVED-3')" ]
}

@test "add: reads the comment from stdin when --comment is absent" {
  echo "piped note" | "$RVW" add --file app.py --lines 1
  run "$RVW" list
  [[ "$output" == *"piped note"* ]]
}

@test "add: keeps multi-line comment text intact" {
  queue app.py 1 "$(printf 'first line\nsecond line')"
  run "$RVW" show r1
  [[ "$output" == *"first line"* ]]
  [[ "$output" == *"second line"* ]]
}

@test "add: refuses an empty comment" {
  run "$RVW" add --file app.py --lines 1 --comment "   " </dev/null
  [ "$status" -eq 1 ]
  [[ "$output" == "rvw: a review comment needs text"* ]]
}

@test "add: refuses a range past the end of the file" {
  run "$RVW" add --file app.py --lines 99 --comment x </dev/null
  [ "$status" -eq 1 ]
  [[ "$output" == *"past the end of"* ]]
}

@test "add: refuses a file outside the workspace" {
  printf 'secret\n' >"$TEST_ROOT/outside.py"
  ln -s "$TEST_ROOT/outside.py" linked.py
  for file in "$TEST_ROOT/outside.py" ../outside.py linked.py; do
    run "$RVW" add --file "$file" --lines 1 --comment x </dev/null
    [ "$status" -eq 1 ]
    [ "$output" = "rvw: $TEST_ROOT/outside.py is outside the workspace $WORKSPACE" ]
  done

  run "$RVW" count
  [ "$output" = "0" ]
}

@test "add: refuses a file that is not a regular file, without reading it" {
  command -v timeout >/dev/null || skip "needs timeout(1)"
  mkfifo pipe.py
  run timeout -s KILL 5 "$RVW" add --file pipe.py --lines 1 --comment x </dev/null
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: $WORKSPACE/pipe.py is not a regular file" ]

  mkdir dir.py
  run "$RVW" add --file dir.py --lines 1 --comment x </dev/null
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: $WORKSPACE/dir.py is not a regular file" ]
}

@test "add: refuses a file or a buffer larger than 4 MiB" {
  head -c 4194305 /dev/zero | tr '\0' 'x' >big.txt
  run "$RVW" add --file big.txt --lines 1 --comment x </dev/null
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: $WORKSPACE/big.txt is larger than 4 MiB" ]

  run "$RVW" add --file app.py --lines 1 --comment x --code-file big.txt </dev/null
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: $WORKSPACE/app.py is larger than 4 MiB" ]

  head -c 4194304 /dev/zero | tr '\0' 'x' >edge.txt
  run "$RVW" add --file edge.txt --lines 1 --comment x </dev/null
  [ "$status" -eq 0 ]
}

@test "add: refuses a file that does not exist without a snapshot" {
  run "$RVW" add --file ghost.py --lines 1 --comment x </dev/null
  [ "$status" -eq 1 ]
  [[ "$output" == *"does not exist"* ]]
}

# ── the core guarantee: pulling drains the queue ──────────────────────────────

@test "submit: records one review over the selected comments" {
  queue app.py 1 "first finding"
  queue app.py 2 "second finding"

  run "$RVW" submit --id r1 --id r2 \
    --decision request-changes \
    --summary "Fix both findings before merging." \
    --format json
  [ "$status" -eq 0 ]
  [ "$(jq -r '.id' <<<"$output")" = "rv1" ]
  [ "$(jq -r '.decision' <<<"$output")" = "request-changes" ]
  [ "$(jq -r '.summary' <<<"$output")" = "Fix both findings before merging." ]
  [ "$(jq -r '.comment_ids | join(",")' <<<"$output")" = "r1,r2" ]
}

@test "submitting without ids groups only the reviewer's comments in the exact lane" {
  "$RVW" add --author alice --file app.py --lines 1 \
    --comment "alice on auth" --lane auth </dev/null >/dev/null
  "$RVW" add --author bob --file app.py --lines 2 \
    --comment "bob on auth" --lane auth </dev/null >/dev/null
  "$RVW" add --author alice --file app.py --lines 3 \
    --comment "alice on payments" --lane payments </dev/null >/dev/null

  run "$RVW" submit --author alice --lane auth \
    --decision request-changes \
    --summary "Alice's auth review." \
    --format json
  [ "$status" -eq 0 ]
  [ "$(jq -r '.comment_ids | join(",")' <<<"$output")" = "r1" ]

  run "$RVW" pull --lane auth
  [ "$status" -eq 0 ]
  [[ "$output" == *"Alice's auth review."* ]]
  [[ "$output" == *"alice on auth"* ]]
  [[ "$output" == *"bob on auth"* ]]
  [[ "$output" != *"alice on payments"* ]]

  run "$RVW" list --lane payments --format ids
  [ "$output" = "r3" ]
}

@test "a summary-only review remains visible and pullable" {
  run "$RVW" submit --decision comment --summary "No actionable findings." --format ids
  [ "$status" -eq 0 ]
  [ "$output" = "rv1" ]

  run "$RVW" count
  [ "$output" = "1" ]

  run "$RVW" pull
  [ "$status" -eq 0 ]
  [[ "$output" == *"No actionable findings."* ]]
}

@test "an explicit summary-only review does not absorb older pending comments" {
  queue app.py 1 "older standalone finding"

  run "$RVW" submit --no-comments --decision comment \
    --summary "This review pass found no actionable issue." --format json
  [ "$status" -eq 0 ]
  [ "$(jq -r '.comment_ids | length' <<<"$output")" = "0" ]

  run "$RVW" list --format ids
  [ "$output" = "r1" ]
}

@test "an invalid selection leaves every comment available for a later review" {
  "$RVW" add --author alice --file app.py --lines 1 \
    --comment "alice finding" </dev/null >/dev/null
  "$RVW" add --author bob --file app.py --lines 2 \
    --comment "bob finding" </dev/null >/dev/null

  run "$RVW" submit --author alice --id r1 --id r2 \
    --decision request-changes --summary "Must not be partially recorded."
  [ "$status" -eq 1 ]

  run "$RVW" submit --author alice --id r1 \
    --decision request-changes --summary "Alice review." --format ids
  [ "$status" -eq 0 ]
  [ "$output" = "rv1" ]

  run "$RVW" submit --author bob --id r2 \
    --decision request-changes --summary "Bob review." --format ids
  [ "$status" -eq 0 ]
  [ "$output" = "rv2" ]
}

@test "a linked comment cannot be resolved before its review is pulled" {
  queue app.py 1 "work me"
  "$RVW" submit --id r1 --decision request-changes --summary "Needs work." >/dev/null

  run "$RVW" resolve r1 --note "too early"
  [ "$status" -eq 1 ]
  [[ "$output" == *"pull rv1 first"* ]]

  "$RVW" pull >/dev/null
  run "$RVW" resolve r1 --note "done after handoff"
  [ "$status" -eq 0 ]
}

@test "flags are the whole interface: environment variables are ignored" {
  RVW_AUTHOR=env REVIEW_AUTHOR=env RVW_LANE=env REVIEW_LANE=env \
    "$RVW" add --file app.py --lines 1 --comment "note" </dev/null
  run "$RVW" list --format json
  [ "$(jq -r '.reviews[0].author' <<<"$output")" = "tester" ]
  [ "$(jq -r '.reviews[0].lane' <<<"$output")" = "null" ]

  git init -q "$TEST_ROOT/other"
  cd "$TEST_ROOT/other"
  RVW_WORKSPACE="$WORKSPACE" REVIEW_WORKSPACE="$WORKSPACE" run "$RVW" count
  [ "$output" = "0" ]
}

@test "pulling one linked comment hands over its whole review" {
  queue app.py 1 "first linked finding"
  queue app.py 2 "second linked finding"
  "$RVW" submit --id r1 --id r2 --decision request-changes \
    --summary "Treat these as one handoff." >/dev/null

  run "$RVW" pull --id r1 --format ids
  [ "$status" -eq 0 ]
  [ "$output" = "rv1" ]

  run "$RVW" count
  [ "$output" = "0" ]
}

@test "a review comment cannot belong to two reviews" {
  queue app.py 1 "one owner"
  "$RVW" submit --id r1 --decision comment --summary "First review." >/dev/null

  run "$RVW" submit --id r1 --decision comment --summary "Second review."
  [ "$status" -eq 1 ]
  [[ "$output" == *"r1 already belongs to rv1"* ]]
}

@test "a pinned pull reports a summary-only review waiting in another lane" {
  "$RVW" submit --lane payments --decision comment \
    --summary "Payments has no actionable findings." >/dev/null

  run "$RVW" pull --lane auth
  [ "$status" -eq 0 ]
  [[ "$output" == *"1 pending in other lanes: payments"* ]]
}

@test "pull: hands over every pending comment and empties the queue" {
  queue app.py 1 "first"
  queue app.py 2 "second"

  run "$RVW" pull
  [ "$status" -eq 0 ]
  [[ "$output" == *"first"* ]]
  [[ "$output" == *"second"* ]]

  run "$RVW" count
  [ "$output" = "0" ]
}

@test "a submitted review hands its decision, summary, and comments over together" {
  queue app.py 1 "first finding"
  queue app.py 2 "second finding"

  run "$RVW" submit --id r1 --id r2 \
    --decision request-changes \
    --summary "The error path must be fixed before merging." \
    --format ids
  [ "$status" -eq 0 ]
  [ "$output" = "rv1" ]

  run "$RVW" pull
  [ "$status" -eq 0 ]
  [[ "$output" == *"Request changes"* ]]
  [[ "$output" == *"The error path must be fixed before merging."* ]]
  [[ "$output" == *'`app.py:1` (r1'* ]]
  [[ "$output" == *'`app.py:2` (r2'* ]]

  run "$RVW" pull
  [ "$status" -eq 0 ]
  [[ "$output" != *"The error path must be fixed before merging."* ]]
  [[ "$output" != *"first finding"* ]]
  [[ "$output" != *"second finding"* ]]
}

@test "pull: a second pull returns nothing — a comment is never handed over twice" {
  queue app.py 1 "only once"
  "$RVW" pull >/dev/null

  run "$RVW" pull
  [ "$status" -eq 0 ]
  [[ "$output" == "No pending review comments for"* ]]
  [[ "$output" != *"only once"* ]]
}

@test "pull: --peek leaves the queue intact" {
  queue app.py 1 "still mine"
  run "$RVW" pull --peek
  [[ "$output" == *"still mine"* ]]

  run "$RVW" count
  [ "$output" = "1" ]
}

@test "pull: --limit takes the oldest comments only" {
  queue app.py 1 "first"
  queue app.py 2 "second"

  run "$RVW" pull --limit 1 --format ids
  [ "$output" = "r1" ]

  run "$RVW" list --format ids
  [ "$output" = "r2" ]
}

@test "pull: --id takes named comments only" {
  queue app.py 1 "first"
  queue app.py 2 "second"
  queue app.py 3 "third"

  run "$RVW" pull --id r2 --format ids
  [ "$output" = "r2" ]

  run "$RVW" list --format ids
  [ "$output" = "$(printf 'r1\nr3')" ]
}

@test "pull: rejects an id that is not pending" {
  queue app.py 1 "first"
  run "$RVW" pull --id r9
  [ "$status" -eq 1 ]
  [[ "$output" == *"not pending in this workspace: r9"* ]]
}

@test "pull: markdown carries the location, code and comment an agent needs" {
  queue app.py 2-3 "tighten this"
  run "$RVW" pull
  [[ "$output" == *'`app.py:2-3` (r1'* ]]
  [[ "$output" == *'```python'* ]]
  [[ "$output" == *"two"* ]]
  [[ "$output" == *"> tighten this"* ]]
}

@test "pull: json is a workspace envelope around the records" {
  queue app.py 1 "note"
  run "$RVW" pull --format json
  [ "$(jq -r '.count' <<<"$output")" = "1" ]
  [ "$(jq -r '.workspace' <<<"$output")" = "$WORKSPACE" ]
  [ "$(jq -r '.reviews[0].status' <<<"$output")" = "pulled" ]
  [ "$(jq -r '.reviews[0].pulled_at' <<<"$output")" != "null" ]
}

@test "pull: pulled comments stay readable as an archive" {
  queue app.py 1 "note"
  "$RVW" pull >/dev/null

  run "$RVW" list --status pulled --format ids
  [ "$output" = "r1" ]
  run "$RVW" list --status all --format count
  [ "$output" = "1" ]
}

# ── ids, listing, editing ─────────────────────────────────────────────────────

@test "ids are never reused after a pull or a rejection" {
  queue app.py 1 "first"
  "$RVW" pull >/dev/null
  run queue app.py 2 "second"
  [ "$output" = "r2" ]

  "$RVW" reject r2 --note "withdrawn" >/dev/null
  run queue app.py 3 "third"
  [ "$output" = "r3" ]
}

@test "list: --file scopes to one file, which is how the editor draws its signs" {
  queue app.py 1 "on app"
  queue other.rb 1 "on other"

  run "$RVW" list --file other.rb --format ids
  [ "$output" = "r2" ]

  run "$RVW" list --file "$WORKSPACE/app.py" --format count
  [ "$output" = "1" ]
}

@test "list: reports an empty queue on stderr, not as a failure" {
  run "$RVW" list
  [ "$status" -eq 0 ]
  [[ "$output" == "rvw: no review comments for"* ]]
}

@test "list --reviews: shows submitted review sheets, summary-only ones too" {
  queue app.py 1 "first finding"
  queue app.py 2 "second finding"
  "$RVW" submit --id r1 --id r2 --decision request-changes \
    --summary "Fix both findings." --format ids >/dev/null
  "$RVW" submit --no-comments --decision approve \
    --summary "Nothing else." --format ids >/dev/null

  run "$RVW" list --reviews
  [ "$status" -eq 0 ]
  [ "${#lines[@]}" -eq 2 ]
  [[ "${lines[0]}" == "rv1  [pending]  request-changes  2 review comment(s)  @tester  Fix both findings." ]]
  [[ "${lines[1]}" == "rv2  [pending]  approve  0 review comment(s)  @tester  Nothing else." ]]

  run "$RVW" list --reviews --format ids
  [ "$output" = "$(printf 'rv1\nrv2')" ]

  run "$RVW" list --reviews --format count
  [ "$output" = "2" ]

  run "$RVW" list --reviews --format json
  [ "$status" -eq 0 ]
  [ "$(jq -r '.count' <<<"$output")" = "2" ]
  [ "$(jq -r '.sheets[0].review.id' <<<"$output")" = "rv1" ]
  [ "$(jq -r '.sheets[0].state' <<<"$output")" = "pending" ]
  [ "$(jq -r '.sheets[0].comments | map(.id) | join(",")' <<<"$output")" = "r1,r2" ]

  run "$RVW" list --reviews --format markdown
  [[ "$output" == *"# Review rv1"* ]]
  [[ "$output" == *"Fix both findings."* ]]
  [[ "$output" == *"first finding"* ]]
}

@test "list --reviews: --status follows the sheet from pending to complete" {
  queue app.py 1 "first finding"
  "$RVW" submit --id r1 --decision request-changes --summary "one" --format ids >/dev/null
  "$RVW" submit --no-comments --decision comment --summary "two" --format ids >/dev/null
  "$RVW" pull --id rv1 >/dev/null

  run "$RVW" list --reviews --format ids
  [ "$output" = "rv2" ]
  run "$RVW" list --reviews --status pulled --format ids
  [ "$output" = "rv1" ]
  run "$RVW" list --reviews --status open --format ids
  [ "$output" = "$(printf 'rv1\nrv2')" ]

  "$RVW" resolve r1 --note "fixed" >/dev/null
  run "$RVW" list --reviews --status complete --format ids
  [ "$output" = "rv1" ]
  run "$RVW" list --reviews --status open --format ids
  [ "$output" = "rv2" ]
  run "$RVW" list --reviews --status all --format ids
  [ "$output" = "$(printf 'rv1\nrv2')" ]
}

@test "list --reviews: refuses a comment-only status" {
  run "$RVW" list --reviews --status done
  [ "$status" -eq 1 ]
  [[ "$output" == "rvw: --status 'done' is not one of pending, pulled, complete, open, all" ]]

  run "$RVW" list --status complete
  [ "$status" -eq 1 ]
}

@test "list --reviews: a pinned lane never shows another lane's or an unlaned review" {
  "$RVW" submit --no-comments --lane auth --decision comment --summary "auth" >/dev/null
  "$RVW" submit --no-comments --lane payments --decision comment --summary "payments" >/dev/null
  "$RVW" submit --no-comments --decision comment --summary "unlaned" >/dev/null

  run "$RVW" list --reviews --lane auth --format ids
  [ "$output" = "rv1" ]
  run "$RVW" list --reviews --format ids
  [ "$output" = "$(printf 'rv1\nrv2\nrv3')" ]
}

@test "list --reviews: --file keeps reviews linking a comment on that file, whatever its status" {
  queue app.py 1 "on app"
  queue other.rb 1 "on other"
  "$RVW" submit --id r1 --decision comment --summary "app review" >/dev/null
  "$RVW" submit --id r2 --decision comment --summary "other review" >/dev/null
  "$RVW" pull --id rv2 >/dev/null
  "$RVW" resolve r2 --note "done" >/dev/null

  run "$RVW" list --reviews --status all --file other.rb --format ids
  [ "$output" = "rv2" ]
  run "$RVW" list --reviews --file app.py --format ids
  [ "$output" = "rv1" ]
}

@test "list --reviews: reports no reviews on stderr, not as a failure" {
  queue app.py 1 "standalone"
  run "$RVW" list --reviews
  [ "$status" -eq 0 ]
  [[ "$output" == "rvw: no submitted reviews for"* ]]
}

@test "edit: replaces the text and keeps the id, file and range" {
  queue app.py 2-3 "old text"
  run "$RVW" edit r1 --comment "new text"
  [ "$status" -eq 0 ]

  run "$RVW" list --format json
  [ "$(jq -r '.reviews[0].id' <<<"$output")" = "r1" ]
  [ "$(jq -r '.reviews[0].comment' <<<"$output")" = "new text" ]
  [ "$(jq -r '.reviews[0].start_line' <<<"$output")" = "2" ]
}

@test "edit: refuses to blank out a comment" {
  queue app.py 1 "keep me"
  run "$RVW" edit r1 --comment "  " </dev/null
  [ "$status" -eq 1 ]
  run "$RVW" show r1
  [[ "$output" == *"keep me"* ]]
}

@test "edit: rejects an unknown id" {
  run "$RVW" edit r9 --comment x </dev/null
  [ "$status" -eq 1 ]
  [[ "$output" == *"no review 'r9'"* ]]
}

# ── workspaces ────────────────────────────────────────────────────────────────

@test "workspace: a subdirectory resolves to the git root, so one repo is one queue" {
  queue app.py 1 "from the root"
  mkdir -p "$WORKSPACE/deep/nested"
  cd "$WORKSPACE/deep/nested"

  run "$RVW" count
  [ "$output" = "1" ]
  run "$RVW" path --workspace-only
  [ "$output" = "$WORKSPACE" ]
}

@test "workspace: a git worktree is its own queue" {
  git -C "$WORKSPACE" -c user.email=t@t -c user.name=t add -A
  git -C "$WORKSPACE" -c user.email=t@t -c user.name=t commit -qm init
  git -C "$WORKSPACE" worktree add -q -b side "$TEST_ROOT/side"
  queue app.py 1 "on main tree"

  cd "$TEST_ROOT/side"
  run "$RVW" count
  [ "$output" = "0" ]
}

@test "workspace: queues in different repos never mix" {
  queue app.py 1 "note-from-the-repo"
  other="$TEST_ROOT/elsewhere"
  mkdir -p "$other"
  git -C "$other" init -q
  printf 'x\n' >"$other/f.txt"
  "$RVW" add --workspace "$other" --file "$other/f.txt" --lines 1 \
    --comment "note-from-elsewhere" </dev/null >/dev/null

  cd "$other"
  run "$RVW" list
  [[ "$output" == *"note-from-elsewhere"* ]]
  [[ "$output" != *"note-from-the-repo"* ]]

  cd "$WORKSPACE"
  run "$RVW" list
  [[ "$output" == *"note-from-the-repo"* ]]
  [[ "$output" != *"note-from-elsewhere"* ]]
}

@test "workspace: a directory outside git is refused, from cwd or --workspace" {
  plain="$TEST_ROOT/plain"
  mkdir -p "$plain"
  printf 'x\n' >"$plain/f.txt"

  cd "$plain"
  run "$RVW" add --file f.txt --lines 1 --comment "no git" </dev/null
  [ "$status" -eq 1 ]
  [ "${#lines[@]}" -eq 1 ]
  [ "$output" = "rvw: $plain is not inside a git repository" ]

  cd "$WORKSPACE"
  run "$RVW" --workspace "$plain" count
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: $plain is not inside a git repository" ]
  [ ! -f "$DB" ]
}

@test "workspace: --workspace retargets the queue, before or after the command" {
  queue app.py 1 "here"
  cd "$TEST_ROOT"

  run "$RVW" --workspace "$WORKSPACE" count
  [ "$output" = "1" ]

  run "$RVW" count --workspace "$WORKSPACE"
  [ "$output" = "1" ]
}

@test "workspaces: lists only workspaces holding pending comments" {
  queue app.py 1 "here"
  run "$RVW" workspaces
  [[ "$output" == *"1 pending  $WORKSPACE"* ]]

  "$RVW" pull >/dev/null
  run "$RVW" workspaces
  [[ "$output" != *"$WORKSPACE"* ]]

  run "$RVW" workspaces --all --format json
  [ "$(jq -r '.[0].pulled' <<<"$output")" = "1" ]
}

# ── lanes: one tree, several branches at once ────────────────────────────────

@test "lane: a comment records the lane it was raised on" {
  "$RVW" add --file app.py --lines 1 --comment "on auth" --lane auth </dev/null
  run "$RVW" list --format json
  [ "$(jq -r '.reviews[0].lane' <<<"$output")" = "auth" ]
}

@test "lane: --lane stamps an add and scopes a read" {
  "$RVW" add --lane auth --file app.py --lines 1 --comment "on auth" </dev/null
  "$RVW" add --file app.py --lines 2 --comment "on payments" --lane payments </dev/null

  run "$RVW" list --lane auth --format ids
  [ "$output" = "r1" ]
  run "$RVW" list --lane payments --format ids
  [ "$output" = "r2" ]
}

@test "lane: a pinned pull never swallows another lane's comments" {
  "$RVW" add --file app.py --lines 1 --comment "on auth" --lane auth </dev/null
  "$RVW" add --file app.py --lines 2 --comment "on payments" --lane payments </dev/null

  run "$RVW" pull --lane auth --format ids
  [ "$output" = "r1" ]

  # the other lane is untouched and still pullable by its own session
  run "$RVW" list --format ids
  [ "$output" = "r2" ]
  run "$RVW" pull --lane payments --format ids
  [ "$output" = "r2" ]
}

@test "lane: an unlaned comment is not picked up by a pinned pull" {
  "$RVW" add --file app.py --lines 1 --comment "belongs to nobody" </dev/null
  run "$RVW" pull --lane auth --format ids
  [[ "$output" != *"r1"* ]]

  run "$RVW" count
  [ "$output" = "1" ]
}

@test "lane: an empty pinned pull says comments are waiting elsewhere" {
  "$RVW" add --file app.py --lines 1 --comment "on payments" --lane payments </dev/null
  run "$RVW" pull --lane auth
  [ "$status" -eq 0 ]
  [[ "$output" == *"1 pending in other lanes: payments"* ]]
}

@test "lane: --all-lanes overrides a pinned session" {
  "$RVW" add --file app.py --lines 1 --comment "on auth" --lane auth </dev/null
  "$RVW" add --file app.py --lines 2 --comment "on payments" --lane payments </dev/null

  run "$RVW" list --lane auth --all-lanes --format ids
  [ "$output" = "$(printf 'r1\nr2')" ]
  run "$RVW" count --lane auth --all-lanes
  [ "$output" = "2" ]
}

@test "lane: an unpinned session sees every lane, which is what the editor does" {
  "$RVW" add --file app.py --lines 1 --comment "on auth" --lane auth </dev/null
  "$RVW" add --file app.py --lines 2 --comment "on payments" --lane payments </dev/null

  run "$RVW" list --format count
  [ "$output" = "2" ]
}

# ── authorship and the recorded decision ─────────────────────────────────────

@test "author: --author names who raised a comment" {
  "$RVW" add --author reviewer-agent --file app.py --lines 1 --comment "note" </dev/null
  run "$RVW" list --format json
  [ "$(jq -r '.reviews[0].author' <<<"$output")" = "reviewer-agent" ]

  run "$RVW" list
  [[ "$output" == *"@reviewer-agent"* ]]
}

@test "author: defaults to the login user" {
  "$RVW" add --file app.py --lines 1 --comment "note" </dev/null
  run "$RVW" list --format json
  [ "$(jq -r '.reviews[0].author' <<<"$output")" = "tester" ]
}

@test "author: the markdown handed to an agent carries the attribution" {
  "$RVW" add --file app.py --lines 1 --comment "note" --author gustavo --lane auth </dev/null
  run "$RVW" pull
  [[ "$output" == *"(r1 · @gustavo #auth)"* ]]
}

@test "resolve: records what became of a comment, and who decided" {
  queue app.py 1 "rename this"
  "$RVW" pull >/dev/null

  run "$RVW" resolve --author impl-agent r1 --note "renamed, test added"
  [ "$status" -eq 0 ]

  run "$RVW" list --status done --format json
  [ "$(jq -r '.reviews[0].status' <<<"$output")" = "done" ]
  [ "$(jq -r '.reviews[0].resolved_by' <<<"$output")" = "impl-agent" ]
  [ "$(jq -r '.reviews[0].resolution_note' <<<"$output")" = "renamed, test added" ]
  [ "$(jq -r '.reviews[0].resolved_at' <<<"$output")" != "null" ]
  [ "$(jq 'has("resolved_file_version")' <<<"$output")" = "false" ]
}

@test "resolve: records the resulting tracked file as an exact Git blob" {
  git add app.py
  queue app.py 1 "rename this"
  run "$RVW" list --format json
  file_version=$(jq -r '.reviews[0].file_version' <<<"$output")

  printf 'changed\nworking\ntree\n' >app.py
  "$RVW" pull >/dev/null

  run "$RVW" resolve r1 --note "renamed, test added"
  [ "$status" -eq 0 ]

  run "$RVW" list --status done --format json
  [ "$status" -eq 0 ]
  resolved_file_version=$(jq -r '.reviews[0].resolved_file_version' <<<"$output")
  [ "$resolved_file_version" != "$file_version" ]

  run git cat-file blob "$file_version"
  [ "$status" -eq 0 ]
  [ "$output" = "$(printf 'one\ntwo\nthree\nfour')" ]

  run git cat-file -t "$resolved_file_version"
  [ "$status" -eq 0 ]
  [ "$output" = "blob" ]

  run git cat-file blob "$resolved_file_version"
  [ "$status" -eq 0 ]
  [ "$output" = "$(printf 'changed\nworking\ntree')" ]
}

@test "reject: demands a reason — a silent decline is the thing being prevented" {
  git add app.py
  queue app.py 1 "change this"
  run "$RVW" reject r1
  [ "$status" -ne 0 ]
  [[ "$output" == *"--note"* ]]

  run "$RVW" reject r1 --note "intentional: the caller validates"
  [ "$status" -eq 0 ]
  run "$RVW" list --status rejected --format json
  [ "$(jq -r '.reviews[0].id' <<<"$output")" = "r1" ]
  [ "$(jq 'has("resolved_file_version")' <<<"$output")" = "false" ]
}

@test "resolve: a missing tracked path closes without an after version" {
  git add app.py
  queue app.py 1 "rename this"
  rm app.py
  "$RVW" pull >/dev/null

  run "$RVW" resolve r1 --note "handled after deletion"
  [ "$status" -eq 0 ]

  run "$RVW" list --status done --format json
  [ "$(jq 'has("resolved_file_version")' <<<"$output")" = "false" ]
}

@test "resolve: a renamed tracked path closes without an after version" {
  git add app.py
  queue app.py 1 "rename this"
  mv app.py renamed.py
  "$RVW" pull >/dev/null

  run "$RVW" resolve r1 --note "renamed elsewhere"
  [ "$status" -eq 0 ]

  run "$RVW" list --status done --format json
  [ "$(jq 'has("resolved_file_version")' <<<"$output")" = "false" ]
}

@test "resolve: an out-of-workspace path closes without an after version" {
  printf 'outside\nworkspace\n' >"$TEST_ROOT/outside.py"
  queue app.py 1 "look here"
  sqlite3 "$DB" "UPDATE comments SET path = '$TEST_ROOT/outside.py', file = '$TEST_ROOT/outside.py'"
  "$RVW" pull >/dev/null

  run "$RVW" resolve r1 --note "handled outside the workspace"
  [ "$status" -eq 0 ]

  run "$RVW" list --status done --format json
  [ "$(jq 'has("resolved_file_version")' <<<"$output")" = "false" ]
}

@test "resolve: Git inspection failure leaves the review unresolved and unchanged" {
  git add app.py
  queue app.py 1 "rename this"
  "$RVW" pull >/dev/null
  mkdir -p "$TEST_ROOT/bin"
  ln -s "$(command -v git)" "$TEST_ROOT/bin/real-git"
  cat >"$TEST_ROOT/bin/git" <<'SH'
#!/bin/sh
if [ "$3" = "ls-files" ]; then
  echo "inspection unavailable" >&2
  exit 128
fi
exec "$(dirname "$0")/real-git" "$@"
SH
  chmod +x "$TEST_ROOT/bin/git"

  run env PATH="$TEST_ROOT/bin:$PATH" \
    "$RVW" resolve r1 --note "should not be recorded"
  [ "$status" -eq 1 ]
  [[ "$output" == "rvw: cannot inspect Git metadata for app.py:"* ]]

  run "$RVW" list --status all --format json
  [ "$(jq -r '.reviews[0].status' <<<"$output")" = "pulled" ]
  [ "$(jq -r '.reviews[0].resolved_at' <<<"$output")" = "null" ]
  [ "$(jq 'has("resolved_file_version")' <<<"$output")" = "false" ]
}

@test "resolve: Git storage failure leaves the review unresolved and unchanged" {
  git add app.py
  queue app.py 1 "rename this"
  printf 'changed\nworking\ntree\n' >app.py
  "$RVW" pull >/dev/null
  mkdir -p "$TEST_ROOT/bin"
  ln -s "$(command -v git)" "$TEST_ROOT/bin/real-git"
  cat >"$TEST_ROOT/bin/git" <<'SH'
#!/bin/sh
if [ "$3" = "hash-object" ]; then
  echo "storage unavailable" >&2
  exit 1
fi
exec "$(dirname "$0")/real-git" "$@"
SH
  chmod +x "$TEST_ROOT/bin/git"

  run env PATH="$TEST_ROOT/bin:$PATH" \
    "$RVW" resolve r1 --note "should not be recorded"
  [ "$status" -eq 1 ]
  [[ "$output" == "rvw: cannot store the resolved version of app.py:"* ]]

  run "$RVW" list --status all --format json
  [ "$(jq -r '.reviews[0].status' <<<"$output")" = "pulled" ]
  [ "$(jq -r '.reviews[0].resolved_at' <<<"$output")" = "null" ]
  [ "$(jq 'has("resolved_file_version")' <<<"$output")" = "false" ]
}

@test "resolve: an older comment without a before version can still record the after version" {
  git add app.py
  queue app.py 1 "rename this"
  sqlite3 "$DB" "UPDATE comments SET file_version = NULL"
  printf 'changed\nworking\ntree\n' >app.py
  "$RVW" pull >/dev/null

  run "$RVW" resolve r1 --note "renamed, test added"
  [ "$status" -eq 0 ]

  run "$RVW" list --status done --format json
  resolved_file_version=$(jq -r '.reviews[0].resolved_file_version' <<<"$output")
  run git cat-file blob "$resolved_file_version"
  [ "$status" -eq 0 ]
  [ "$output" = "$(printf 'changed\nworking\ntree')" ]
}

@test "resolve: a decided comment cannot be quietly re-decided" {
  queue app.py 1 "note"
  "$RVW" resolve r1 --note "done it"

  run "$RVW" resolve r1 --note "no, again"
  [ "$status" -eq 1 ]
  [[ "$output" == *"already done"* ]]

  run "$RVW" reject r1 --note "changed my mind"
  [ "$status" -eq 1 ]
}

@test "resolve: rejects an unknown id" {
  run "$RVW" resolve r9
  [ "$status" -eq 1 ]
  [[ "$output" == *"no review 'r9'"* ]]
}

@test "status: open is everything raised but not yet decided" {
  queue app.py 1 "still queued"
  queue app.py 2 "handed over"
  queue app.py 3 "finished"
  "$RVW" pull --id r2 --id r3 >/dev/null
  "$RVW" resolve r3 --note "done"

  run "$RVW" list --status open --format ids
  [ "$output" = "$(printf 'r1\nr2')" ]
  run "$RVW" count --status open
  [ "$output" = "2" ]
}

@test "show: prints the decision alongside the comment" {
  queue app.py 1 "rename this"
  "$RVW" resolve r1 --note "renamed to first()" --author impl-agent

  run "$RVW" show r1
  [[ "$output" == *"[DONE]"* ]]
  [[ "$output" == *"done by @impl-agent"* ]]
  [[ "$output" == *"renamed to first()"* ]]
}

@test "show: json is the comment with its evidence" {
  queue app.py 1 "rename this"
  "$RVW" resolve r1 --note "renamed"

  run "$RVW" show r1 --format json
  [ "$status" -eq 0 ]
  [ "$(jq -r '.comment.id' <<<"$output")" = "r1" ]
  [ "$(jq -r '.comment.status' <<<"$output")" = "done" ]
  [ "$(jq -r '.diff | type' <<<"$output")" = "array" ]
  [ "$(jq -r 'has("diff_available")' <<<"$output")" = "true" ]

  run "$RVW" show r1 --format markdown
  [[ "$output" == *'`app.py:1` (r1'* ]]
}

@test "reject: retracts a pending comment, which leaves the queue but stays on record" {
  queue app.py 1 "never mind"
  queue app.py 2 "still wanted"

  run "$RVW" reject r1 --note "withdrawn: misread the diff" --author alice
  [ "$status" -eq 0 ]

  run "$RVW" count
  [ "$output" = "1" ]
  run "$RVW" list --format ids
  [ "$output" = "r2" ]
  run "$RVW" pull --format ids
  [ "$output" = "r2" ]

  run "$RVW" list --status rejected --format json
  [ "$(jq -r '.reviews[0].id' <<<"$output")" = "r1" ]
  [ "$(jq -r '.reviews[0].resolved_by' <<<"$output")" = "alice" ]
  [ "$(jq -r '.reviews[0].resolution_note' <<<"$output")" = "withdrawn: misread the diff" ]
}

@test "nothing is deleted: drop, clear and display are not commands" {
  queue app.py 1 "keep me"
  for sub in drop clear display; do
    run "$RVW" "$sub" r1
    [ "$status" -eq 1 ]
    [[ "$output" == "rvw: unknown command \"$sub\""* ]]
  done
  run "$RVW" count
  [ "$output" = "1" ]
}

@test "show: an open review line shows saved source before and after handoff" {
  queue app.py 2-3 "tighten this"

  run "$RVW" show r1
  [ "$status" -eq 0 ]
  [[ "$output" == *"REVIEW LINE r1"* ]]
  [[ "$output" == *"[PENDING]"* ]]
  [[ "$output" == *"app.py:2-3"* ]]
  [[ "$output" == *"> 2 | two"* ]]
  [[ "$output" == *"> 3 | three"* ]]
  [[ "$output" == *"tighten this"* ]]

  run "$RVW" list --status pending --format ids
  [ "$output" = "r1" ]

  "$RVW" pull >/dev/null

  run "$RVW" show r1
  [ "$status" -eq 0 ]
  [[ "$output" == *"[PULLED]"* ]]
  [[ "$output" == *"> 2 | two"* ]]
  [[ "$output" == *"tighten this"* ]]

  run "$RVW" list --status pulled --format ids
  [ "$output" = "r1" ]
}

@test "show: a completed review line shows every changed hunk between snapshots" {
  git add app.py
  queue app.py 2 "change this"
  "$RVW" pull >/dev/null
  printf 'one\nchanged-two\nthree-modified\nfour\n' > app.py
  "$RVW" resolve r1 --note "updated implementation" >/dev/null

  run "$RVW" show r1
  [ "$status" -eq 0 ]
  [[ "$output" == *"REVIEW LINE r1"* ]]
  [[ "$output" == *"[DONE]"* ]]
  [[ "$output" == *"DIFF"* ]]
  [[ "$output" == *"-two"* ]]
  [[ "$output" == *"+changed-two"* ]]
  [[ "$output" == *"-three"* ]]
  [[ "$output" == *"+three-modified"* ]]
  [[ "$output" == *"RESOLUTION"* ]]
  [[ "$output" == *"updated implementation"* ]]

  while IFS= read -r line; do
    [ "${#line}" -le 80 ]
  done <<< "$output"
}

@test "show: a rejected review line shows its source and reason without a diff" {
  queue app.py 2 "change this"
  "$RVW" pull >/dev/null
  "$RVW" reject r1 --note "keep the existing implementation" >/dev/null

  run "$RVW" show r1
  [ "$status" -eq 0 ]
  [[ "$output" == *"REVIEW LINE r1"* ]]
  [[ "$output" == *"[REJECTED]"* ]]
  [[ "$output" == *"> 2 | two"* ]]
  [[ "$output" == *"RESOLUTION"* ]]
  [[ "$output" == *"keep the existing implementation"* ]]
  [[ "$output" != *"DIFF"* ]]
}

@test "show: a submitted review sheet preserves summary order and evidence" {
  queue app.py 1 "first finding"
  queue app.py 2 "second finding"
  "$RVW" submit --id r1 --id r2 \
    --decision request-changes \
    --summary "The error path must be fixed before merging." \
    --format ids >/dev/null

  run "$RVW" show rv1
  [ "$status" -eq 0 ]
  [[ "$output" == *"REVIEW SHEET rv1"* ]]
  [[ "$output" == *"[PENDING]"* ]]
  [[ "$output" == *"REQUEST CHANGES"* ]]
  [[ "$output" == *"SUMMARY"* ]]
  [[ "$output" == *"The error path must be fixed before merging."* ]]
  [[ "$output" == *"LEDGER"* ]]
  [[ "$output" == *"1. r1"* ]]
  [[ "$output" == *"2. r2"* ]]
  [[ "$output" == *"EVIDENCE 1"* ]]
  [[ "$output" == *"EVIDENCE 2"* ]]
  [[ "$output" == *"> 1 | one"* ]]
  [[ "$output" == *"> 2 | two"* ]]
  [[ "$output" == *"first finding"* ]]
  [[ "$output" == *"second finding"* ]]

  first_ledger=$(grep -n "1. r1" <<< "$output" | head -1 | cut -d: -f1)
  second_ledger=$(grep -n "2. r2" <<< "$output" | head -1 | cut -d: -f1)
  [ "$first_ledger" -lt "$second_ledger" ]

  while IFS= read -r line; do
    [ "${#line}" -le 80 ]
  done <<< "$output"
}

@test "show: a submitted review completes only when every comment is terminal" {
  queue app.py 1 "first finding"
  queue app.py 2 "second finding"
  "$RVW" submit --id r1 --id r2 \
    --decision request-changes \
    --summary "Resolve every finding." \
    --format ids >/dev/null
  "$RVW" pull --id rv1 >/dev/null
  "$RVW" resolve r1 --note "fixed" >/dev/null

  run "$RVW" show rv1
  [ "$status" -eq 0 ]
  [[ "$output" == *"[PULLED]"* ]]
  [[ "$output" != *"[COMPLETE]"* ]]

  "$RVW" reject r2 --note "intentionally retained" >/dev/null
  run "$RVW" show rv1
  [ "$status" -eq 0 ]
  [[ "$output" == *"[COMPLETE]"* ]]
}

@test "show: a pulled summary-only review is complete" {
  "$RVW" submit --no-comments \
    --decision comment \
    --summary "No actionable findings." \
    --format ids >/dev/null
  "$RVW" pull --id rv1 >/dev/null

  run "$RVW" show rv1
  [ "$status" -eq 0 ]
  [[ "$output" == *"[COMPLETE]"* ]]
  [[ "$output" == *"(no linked comments)"* ]]
}

@test "show: a missing linked comment stays visible and keeps the review incomplete" {
  queue app.py 1 "missing evidence"
  "$RVW" submit --id r1 \
    --decision request-changes \
    --summary "The linked finding must remain accountable." \
    --format ids >/dev/null
  "$RVW" pull --id rv1 >/dev/null

  sqlite3 "$DB" "DELETE FROM comments WHERE seq = 1"

  run "$RVW" show rv1
  [ "$status" -eq 0 ]
  [[ "$output" == *"[PULLED]"* ]]
  [[ "$output" == *"MISSING r1"* ]]
  [[ "$output" != *"[COMPLETE]"* ]]
}

# ── MCP: the same queue as tool calls ────────────────────────────────────────

# Talk JSON-RPC to `rvw mcp serve` on stdio. Each arg is one request; each is
# sent once the previous one is answered, since the server runs calls
# concurrently and drops unanswered ones when stdin closes. Extra server
# flags go in MCP_FLAGS.
mcp_session() {
  local out="$TEST_ROOT/mcp.out"
  : >"$out"
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"bats","version":"1"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    local id=1
    for msg in "$@"; do
      printf '{"jsonrpc":"2.0","id":%d,%s}\n' "$id" "$msg"
      for _ in $(seq 100); do
        grep -q "\"id\":$id," "$out" && break
        sleep 0.1
      done
      id=$((id + 1))
    done
  } | "$RVW" mcp serve ${MCP_FLAGS:-} >"$out"
  cat "$out"
}

@test "mcp serve: an agent adds, pulls and resolves over stdio" {
  run mcp_session \
    '"method":"tools/call","params":{"name":"add","arguments":{"workspace":"'"$WORKSPACE"'","file":"app.py","start_line":2,"comment":"over mcp"}}' \
    '"method":"tools/call","params":{"name":"pull","arguments":{"workspace":"'"$WORKSPACE"'"}}' \
    '"method":"tools/call","params":{"name":"resolve","arguments":{"workspace":"'"$WORKSPACE"'","id":"r1","outcome":"done","note":"fixed"}}'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"id":3,'* ]]
  [[ "$output" != *'"isError":true'* ]]

  run "$RVW" list --status done --format ids
  [ "$output" = "r1" ]
  run "$RVW" show r1 --format json
  [[ "$output" == *'"resolved_by": "tester"'* ]]
}

@test "mcp serve: --author and --lane fill calls that leave them out" {
  MCP_FLAGS="--author claude --lane auth" run mcp_session \
    '"method":"tools/call","params":{"name":"add","arguments":{"workspace":"","file":"app.py","start_line":1,"comment":"x"}}'
  [ "$status" -eq 0 ]

  run "$RVW" list --lane auth --format json
  [[ "$output" == *'"author": "claude"'* ]]
  [[ "$output" == *'"lane": "auth"'* ]]
}

@test "mcp serve: a service failure is a tool error, not a crash" {
  run mcp_session \
    '"method":"tools/call","params":{"name":"resolve","arguments":{"workspace":"'"$WORKSPACE"'","id":"r9","outcome":"done"}}'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"isError":true'* ]]
  [[ "$output" == *"r9"* ]]
}

@test "mcp serve: refuses to start outside git" {
  plain="$TEST_ROOT/plain"
  mkdir -p "$plain"
  cd "$plain"
  run "$RVW" mcp serve </dev/null
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: $plain is not inside a git repository" ]

  cd "$WORKSPACE"
  run "$RVW" mcp serve --workspace "$plain" </dev/null
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: $plain is not inside a git repository" ]
}

@test "mcp serve: --http listens only on a loopback address" {
  for addr in 0.0.0.0:0 :0 '[::]:0' 192.168.1.5:0 example.com:0; do
    run "$RVW" mcp serve --http "$addr" </dev/null
    [ "$status" -eq 1 ]
    [ "$output" = "rvw: --http must be a loopback address, got '$addr'" ]
  done
}

@test "mcp serve: a call naming a workspace outside git is a tool error" {
  plain="$TEST_ROOT/plain"
  mkdir -p "$plain"
  run mcp_session \
    '"method":"tools/call","params":{"name":"count","arguments":{"workspace":"'"$plain"'"}}'
  [ "$status" -eq 0 ]
  [[ "$output" == *'"isError":true'* ]]
  [[ "$output" == *"workspace $plain is not inside a git repository"* ]]
}

@test "mcp config: prints the claude mcp add command and the .mcp.json entry" {
  run "$RVW" mcp config --author claude
  [ "$status" -eq 0 ]
  [[ "$output" == *"claude mcp add --scope user rvw -- $RVW mcp serve --author claude"* ]]
  [[ "$output" == *'"mcpServers"'* ]]
  [[ "$output" == *'"command": "'"$RVW"'"'* ]]

  run "$RVW" mcp config --db "$DB" --scope project --format json
  [ "$status" -eq 0 ]
  [[ "$output" == *'"args": ['*'"serve",'*'"--db",'*"\"$DB\""* ]]
  [[ "$output" != *"claude mcp add"* ]]
}

@test "mcp config: --http points Claude Code at a running server" {
  run "$RVW" mcp config --http 127.0.0.1:7777
  [ "$status" -eq 0 ]
  [[ "$output" == *"$RVW mcp serve --http 127.0.0.1:7777"* ]]
  [[ "$output" == *"claude mcp add --transport http --scope user rvw http://127.0.0.1:7777/mcp"* ]]
  [[ "$output" == *'"url": "http://127.0.0.1:7777/mcp"'* ]]
}

@test "mcp config: rejects a bad scope or address" {
  run "$RVW" mcp config --scope global
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: --scope 'global' is not one of local, project, user" ]

  run "$RVW" mcp config --http 7777
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: --http '7777' is not HOST:PORT" ]

  run "$RVW" mcp config --http 192.168.1.5:7777
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: --http must be a loopback address, got '192.168.1.5:7777'" ]

  mkdir -p "$TEST_ROOT/plain"
  run "$RVW" mcp config --workspace "$TEST_ROOT/plain"
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: $TEST_ROOT/plain is not inside a git repository" ]
}

# ── tui ───────────────────────────────────────────────────────────────────────

@test "tui: without a terminal it fails with one line and touches nothing" {
  run "$RVW" tui </dev/null
  [ "$status" -eq 1 ]
  [ "$output" = "rvw: tui needs an interactive terminal on stdin and stdout" ]
  [ ! -e "$DB" ]
}

@test "tui --help documents the keys and where comments are written" {
  run "$RVW" tui --help
  [ "$status" -eq 0 ]
  [[ "$output" == *"Usage:"*"rvw tui"* ]]
  [[ "$output" == *"--editor"*'$VISUAL'*'$EDITOR'*"vi"* ]]
  [[ "$output" == *"<leader> is space unless --leader"*"<leader>p go to file"*"<leader>l open comments"* ]]
  [[ "$output" == *"--leader string"* ]]
  [[ "$output" == *"V select lines"*"c comment"*"e edit"* ]]
  [[ "$output" == *"t files, changes or reviews"*"b compare with"*"uncommitted"*"default branch"*"previous commit"* ]]
  [[ "$output" == *"review or comment page"*"o open the file"*"esc back"* ]]
}

# The TUI on a pseudo-terminal needs util-linux script(1); BSD script differs.
needs_pty() {
  script --version 2>/dev/null | grep -q util-linux || skip "needs util-linux script(1)"
}

# Run `rvw tui FLAGS...` on a pseudo-terminal, typing each KEYS argument after
# `--` in turn (printf %b escapes: \r is Enter; space is the leader). An argument
# `wait:CMD` instead retries CMD until it succeeds, so keys never race the
# editor. Typing starts after a second: keys sent before the TUI takes the
# terminal go through its cooked mode, where Enter arrives as a newline.
tui_session() {
  local flags=() cmd
  while [ "$1" != "--" ]; do flags+=("$1"); shift; done
  shift
  cmd=$(printf '%q ' "$RVW" tui "${flags[@]}")
  {
    sleep 1
    for step in "$@"; do
      case "$step" in
        wait:*)
          for _ in $(seq 100); do
            eval "${step#wait:}" >/dev/null 2>&1 && break
            sleep 0.1
          done ;;
        *) printf '%b' "$step"; sleep 0.2 ;;
      esac
    done
  } | timeout 30 script -qec "$cmd" /dev/null >/dev/null
}

@test "tui: q quits on a terminal and writes nothing" {
  needs_pty
  tui_session -- q
  run "$RVW" count
  [ "$output" = "0" ]
}

@test "tui: comments on a selection and submits a review through the editor" {
  needs_pty
  plain="$TEST_ROOT/plain"
  mkdir -p "$plain/src/api" "$plain/test/api"
  git -C "$plain" init -q
  printf 'import parse\n\ndef handle(req):\n    if req.kind:\n        return parse(req)\n' >"$plain/src/api/parse.py"
  printf 'package api\n' >"$plain/test/api/api_parser_test.go"
  printf 'hello\n' >"$plain/README.md"
  cat >"$TEST_ROOT/editor" <<'SH'
#!/bin/sh
{ echo "from the editor"; cat "$1"; } >"$1.new" && mv "$1.new" "$1"
SH
  chmod +x "$TEST_ROOT/editor"
  cd "$plain"

  tui_session --editor "$TEST_ROOT/editor" -- ' papipar\r' '3GV2jc' \
    'wait:[ "$("$RVW" count)" = 1 ]' 's3' \
    'wait:"$RVW" list --reviews --format ids | grep -q rv1' q

  run "$RVW" list --format json
  [ "$(jq -r '.reviews[0].file' <<<"$output")" = "src/api/parse.py" ]
  [ "$(jq -r '.reviews[0].start_line' <<<"$output")" = "3" ]
  [ "$(jq -r '.reviews[0].end_line' <<<"$output")" = "5" ]
  [ "$(jq -r '.reviews[0].comment' <<<"$output")" = "from the editor" ]
  [ "$(jq -r '.reviews[0].author' <<<"$output")" = "tester" ]
  [ "$(jq -r '.reviews[0].file_version // "none"' <<<"$output")" = "none" ]

  run "$RVW" list --reviews --format json
  [ "$(jq -r '.sheets[0].review.decision' <<<"$output")" = "request-changes" ]
  [ "$(jq -r '.sheets[0].review.summary' <<<"$output")" = "from the editor" ]
  [ "$(jq -r '.sheets[0].comments | map(.id) | join(",")' <<<"$output")" = "r1" ]
}

@test "tui --leader takes space or one character, and fails before the terminal check" {
  run "$RVW" tui --leader ab </dev/null
  [ "$status" -eq 1 ]
  [ "$output" = 'rvw: --leader must be space or one character, got "ab"' ]
  run "$RVW" tui --leader ctrl+p </dev/null
  [ "$output" = 'rvw: --leader must be space or one character, got "ctrl+p"' ]
  run "$RVW" tui --leader "" </dev/null
  [ "$output" = 'rvw: --leader must be space or one character, got ""' ]
  run "$RVW" tui --leader $'\t' </dev/null
  [ "$output" = 'rvw: --leader must be space or one character, got "\t"' ]
  run "$RVW" tui --leader , </dev/null
  [ "$output" = "rvw: tui needs an interactive terminal on stdin and stdout" ]
  run "$RVW" tui --leader space </dev/null
  [ "$output" = "rvw: tui needs an interactive terminal on stdin and stdout" ]
}

# ── shape of the tool itself ──────────────────────────────────────────────────

@test "count: prints 0 for an untouched workspace instead of failing" {
  run "$RVW" count
  [ "$status" -eq 0 ]
  [ "$output" = "0" ]
}

@test "the store is one database under \$XDG_DATA_HOME, shared by every workspace" {
  queue app.py 1 "note"
  run "$RVW" path
  [ "$output" = "$XDG_DATA_HOME/rvw/rvw.db" ]
  [ -f "$output" ]

  other="$TEST_ROOT/elsewhere"
  mkdir -p "$other"
  git -C "$other" init -q
  run "$RVW" --workspace "$other" path
  [ "$output" = "$XDG_DATA_HOME/rvw/rvw.db" ]
}

@test "the store falls back to ~/.local/share, and --db overrides it" {
  unset XDG_DATA_HOME
  HOME="$TEST_ROOT/home" run "$RVW" path
  [ "$output" = "$TEST_ROOT/home/.local/share/rvw/rvw.db" ]

  XDG_DATA_HOME=relative/ignored HOME="$TEST_ROOT/home" run "$RVW" path
  [ "$output" = "$TEST_ROOT/home/.local/share/rvw/rvw.db" ]

  run "$RVW" --db "$TEST_ROOT/custom.db" path
  [ "$output" = "$TEST_ROOT/custom.db" ]

  "$RVW" --db "$TEST_ROOT/custom.db" add --file app.py --lines 1 --comment "elsewhere" </dev/null
  [ -f "$TEST_ROOT/custom.db" ]
  run "$RVW" count
  [ "$output" = "0" ]
}

@test "--help works for the tool and every subcommand" {
  run "$RVW" --help
  [ "$status" -eq 0 ]
  [[ "$output" == *"per-workspace queue of code review feedback"* ]]

  for sub in add submit list pull show edit resolve reject count workspaces path mcp tui; do
    run "$RVW" "$sub" --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage:"*"rvw $sub"* ]]
  done
}

@test "--version prints the version" {
  run "$RVW" --version
  [ "$status" -eq 0 ]
  [[ "$output" == "rvw version "?* ]]
}

@test "bare invocation prints help instead of failing" {
  run "$RVW"
  [ "$status" -eq 0 ]
  [[ "$output" == *"Available Commands"* ]]
}

@test "concurrent adds do not lose comments" {
  for i in 1 2 3 4 5 6 7 8; do
    "$RVW" add --file app.py --lines 1 --comment "note $i" </dev/null >/dev/null &
  done
  wait

  run "$RVW" count
  [ "$output" = "8" ]
  run "$RVW" list --format ids
  [ "$(sort -u <<<"$output" | wc -l)" -eq 8 ]
}
