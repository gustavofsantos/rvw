#!/usr/bin/env bash
# Prepares a machine to build and check rvw: the Rust toolchain, the system
# packages `make check` and the desktop app need, and the pinned cargo-audit.
# Idempotent. Used as the Claude Code on the web environment setup script
# (paste this file's content, or `bash scripts/setup-dev.sh` once the repo is
# cloned). The system provisioning needs no checkout; the project warm-up at
# the end runs only when the script is run from one.
set -euo pipefail

root=""
if [ -f "${BASH_SOURCE[0]}" ] && [ -f "$(dirname "${BASH_SOURCE[0]}")/../Cargo.toml" ]; then
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi

# rust-version in Cargo.toml, checked by the msrv job in ci.yml.
MSRV=1.93.0

sudo=""
if [ "$(id -u)" -ne 0 ]; then sudo="sudo"; fi

# cargo and rustc must come from the same toolchain. A distro rustc earlier in
# PATH than ~/.cargo/bin builds dependencies with a different compiler than
# cargo and clippy, and every build fails with E0514.
export PATH="$HOME/.cargo/bin:$PATH"

if command -v apt-get >/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  $sudo apt-get update -qq
  # build-essential and pkg-config: the bundled SQLite and native crates.
  # sqlite3 and bats: test/rvw.bats. The lib*-dev packages: rvw-gui (gpui).
  $sudo apt-get install -y -qq --no-install-recommends \
    build-essential pkg-config curl ca-certificates git sqlite3 bats \
    libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev
fi

if ! command -v rustup >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --profile minimal --default-toolchain stable
fi
rustup toolchain install stable --profile minimal --component clippy --component rustfmt
rustup default stable
rustup toolchain install "$MSRV" --profile minimal

# bats tests commit in scratch repositories.
if [ -z "$(git config --global user.name || true)" ]; then
  git config --global user.name rvw-dev
  git config --global user.email rvw-dev@localhost
fi

# Persist what later shells need: the toolchain order and a UTF-8 locale.
for rc in "$HOME/.bashrc" "${CLAUDE_ENV_FILE:-}"; do
  [ -n "$rc" ] || continue
  grep -qs 'rvw-dev-env' "$rc" || cat >>"$rc" <<'EOF'
# rvw-dev-env
export PATH="$HOME/.cargo/bin:$PATH"
export LC_ALL=C.UTF-8
EOF
done

if [ -n "$root" ]; then
  cd "$root"
  cargo fetch --locked
  make target/tools/bin/cargo-audit # pinned in the Makefile, kept under target/
fi

echo "ready: run 'make check'"
