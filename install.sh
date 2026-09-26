#!/bin/sh
# Install rvw from a GitHub release.
#
#   curl -fsSL https://github.com/gustavofsantos/rvw/releases/latest/download/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- -b /usr/local/bin -v v0.1.0
#
# -b DIR  install into DIR (default: ~/.local/bin)
# -v TAG  install release TAG (default: the latest release)
set -eu

repo=gustavofsantos/rvw
bindir="$HOME/.local/bin"
tag=

usage() {
  echo "usage: install.sh [-b DIR] [-v TAG]" >&2
  exit 1
}

fail() {
  echo "install.sh: $*" >&2
  exit 1
}

while getopts b:v:h opt; do
  case $opt in
    b) bindir=$OPTARG ;;
    v) tag=$OPTARG ;;
    *) usage ;;
  esac
done

case $(uname -s) in
  Linux) os=linux ;;
  Darwin) os=darwin ;;
  *) fail "unsupported OS: $(uname -s)" ;;
esac

case $(uname -m) in
  x86_64 | amd64) arch=amd64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *) fail "unsupported architecture: $(uname -m)" ;;
esac

if [ -n "$tag" ]; then
  base="https://github.com/$repo/releases/download/$tag"
else
  base="https://github.com/$repo/releases/latest/download"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
else
  fail "needs curl or wget"
fi

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  fail "needs sha256sum or shasum"
fi

asset="rvw_${os}_${arch}.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch "$base/$asset" "$tmp/$asset" || fail "download failed: $base/$asset"
fetch "$base/checksums.txt" "$tmp/checksums.txt" || fail "download failed: $base/checksums.txt"

want=$(awk -v f="$asset" '$2 == f { print $1 }' "$tmp/checksums.txt")
[ -n "$want" ] || fail "$asset is not in checksums.txt"
[ "$(sha256 "$tmp/$asset")" = "$want" ] || fail "checksum mismatch for $asset"

tar -xzf "$tmp/$asset" -C "$tmp" rvw
mkdir -p "$bindir"
install -m 0755 "$tmp/rvw" "$bindir/rvw"
echo "installed rvw to $bindir/rvw" >&2

case ":$PATH:" in
  *":$bindir:"*) ;;
  *) echo "note: $bindir is not on your PATH" >&2 ;;
esac
