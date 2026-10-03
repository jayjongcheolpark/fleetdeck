#!/bin/sh
# Install the latest fleetdeck release.
#
#   curl -fsSL https://raw.githubusercontent.com/jayjongcheolpark/fleetdeck/main/install.sh | sh
#
# The script downloads the archive for this OS and CPU, checks it against the
# release's SHA256SUMS, and installs the binary to ~/.local/bin, or to
# $FLEETDECK_INSTALL_DIR when it is set. Set FLEETDECK_VERSION (for example
# 0.2.0) to install that version in place of the latest one.

set -eu

REPO=jayjongcheolpark/fleetdeck

say() { printf 'fleetdeck: %s\n' "$*"; }
die() {
	printf 'fleetdeck: %s\n' "$*" >&2
	exit 1
}

need() { command -v "$1" >/dev/null 2>&1 || die "this script needs $1"; }

need curl
need tar
need uname

case "$(uname -s)" in
Darwin) os=apple-darwin ;;
Linux) os=unknown-linux-gnu ;;
*) die "no release for $(uname -s); build from source with cargo" ;;
esac
case "$(uname -m)" in
arm64 | aarch64) arch=aarch64 ;;
x86_64 | amd64) arch=x86_64 ;;
*) die "no release for $(uname -m); build from source with cargo" ;;
esac
target="$arch-$os"

if [ -n "${FLEETDECK_VERSION:-}" ]; then
	version=${FLEETDECK_VERSION#v}
else
	# /releases/latest redirects to /releases/tag/v<version>.
	url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") ||
		die "cannot reach GitHub"
	version=${url##*/v}
	case "$version" in
	'' | */*) die "cannot find the latest release from $url" ;;
	esac
fi

if command -v sha256sum >/dev/null 2>&1; then
	sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
	sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
	die "this script needs sha256sum or shasum"
fi

dir="${FLEETDECK_INSTALL_DIR:-$HOME/.local/bin}"
name="fleetdeck-$version-$target"
base="https://github.com/$REPO/releases/download/v$version"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading $name.tar.gz"
curl -fsSL -o "$tmp/$name.tar.gz" "$base/$name.tar.gz" || die "cannot download $base/$name.tar.gz"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "cannot download $base/SHA256SUMS"

want=$(awk -v f="$name.tar.gz" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
[ -n "$want" ] || die "SHA256SUMS has no entry for $name.tar.gz"
got=$(sha256 "$tmp/$name.tar.gz")
[ "$want" = "$got" ] || die "checksum mismatch for $name.tar.gz (expected $want, got $got)"
say "checksum OK"

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$dir"
# Copy next to the old binary, then rename over it, so a failed copy keeps the old one.
cp "$tmp/$name/fleetdeck" "$dir/.fleetdeck.new"
chmod 755 "$dir/.fleetdeck.new"
mv -f "$dir/.fleetdeck.new" "$dir/fleetdeck"
say "installed $("$dir/fleetdeck" --version) to $dir/fleetdeck"

case ":$PATH:" in
*":$dir:"*) ;;
*)
	say "$dir is not on your PATH. Add this line to your shell profile:"
	# shellcheck disable=SC2016 # print $PATH literally
	printf '\n    export PATH="%s:$PATH"\n\n' "$dir"
	;;
esac
say "run 'fleetdeck update' later to install a newer release"
