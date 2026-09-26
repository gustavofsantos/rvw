#!/bin/sh
# Installs rvw from its GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/gustavofsantos/rvw/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/gustavofsantos/rvw/main/install.sh | sh -s -- --version v1.1.0 --dir /usr/local/bin
#
# Downloads the archive for this OS and CPU, checks it against the release's
# checksums.txt, and puts the rvw binary in --dir (default: ~/.local/bin).
# Run it again to upgrade.
set -eu

repo=gustavofsantos/rvw
version=latest
dir="${HOME}/.local/bin"

usage() {
	cat <<'EOF'
Usage: install.sh [--version TAG] [--dir DIR]

  --version TAG  release to install, such as v1.1.0 (default: the latest)
  --dir DIR      where to put the rvw binary (default: ~/.local/bin)
EOF
}

fail() {
	echo "install.sh: $*" >&2
	exit 1
}

while [ $# -gt 0 ]; do
	case "$1" in
	--version)
		[ $# -ge 2 ] || fail "--version needs a tag"
		version=$2
		shift 2
		;;
	--dir)
		[ $# -ge 2 ] || fail "--dir needs a directory"
		dir=$2
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		usage >&2
		fail "unknown argument: $1"
		;;
	esac
done

case "$(uname -s)" in
Linux) os=linux ;;
Darwin) os=darwin ;;
*) fail "no prebuilt binary for $(uname -s); download one from https://github.com/$repo/releases or run: go install github.com/$repo/cmd/rvw@latest" ;;
esac

case "$(uname -m)" in
x86_64 | amd64) arch=amd64 ;;
aarch64 | arm64) arch=arm64 ;;
*) fail "no prebuilt binary for $(uname -m); run: go install github.com/$repo/cmd/rvw@latest" ;;
esac

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

if [ "$version" = latest ]; then
	base="https://github.com/$repo/releases/latest/download"
else
	case "$version" in
	v*) ;;
	*) version="v$version" ;;
	esac
	base="https://github.com/$repo/releases/download/$version"
fi
archive="rvw_${os}_${arch}.tar.gz"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "Downloading $archive ($version)" >&2
curl -fsSL -o "$tmp/$archive" "$base/$archive" || fail "could not download $base/$archive"
curl -fsSL -o "$tmp/checksums.txt" "$base/checksums.txt" || fail "could not download $base/checksums.txt"

want=$(awk -v f="$archive" '$2 == f { print $1 }' "$tmp/checksums.txt")
[ -n "$want" ] || fail "$archive is not listed in checksums.txt"
if command -v sha256sum >/dev/null 2>&1; then
	got=$(sha256sum "$tmp/$archive" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
	got=$(shasum -a 256 "$tmp/$archive" | awk '{ print $1 }')
else
	fail "sha256sum or shasum is required to verify the download"
fi
[ "$want" = "$got" ] || fail "checksum mismatch for $archive"

tar -xzf "$tmp/$archive" -C "$tmp" rvw
mkdir -p "$dir" || fail "cannot create $dir; pick another with --dir"
# Move into place from the same directory, so a running rvw is never
# overwritten halfway.
cp "$tmp/rvw" "$dir/.rvw.new" || fail "cannot write to $dir; pick another with --dir"
chmod 755 "$dir/.rvw.new"
mv -f "$dir/.rvw.new" "$dir/rvw"

echo "Installed $("$dir/rvw" --version) to $dir/rvw" >&2
case ":$PATH:" in
*":$dir:"*) ;;
*) echo "Note: $dir is not on your PATH. Add it, for example: export PATH=\"$dir:\$PATH\"" >&2 ;;
esac
