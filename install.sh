#!/bin/sh
# Install fr: download the release binary for this machine, verify its checksum, and
# put it on your PATH.
#
#   curl -fsSL https://raw.githubusercontent.com/omraval18/flow-record/main/install.sh | sh
#
# Environment:
#   FR_INSTALL_DIR  where to put the binary           (default: ~/.local/bin)
#   FR_VERSION      release tag to install, e.g. v0.1.0 (default: latest)
#   FR_REPO         GitHub repository, owner/name      (default: omraval18/flow-record)
#   FR_BASE_URL     override the download location (used by tests)
set -eu

REPO="${FR_REPO:-omraval18/flow-record}"
INSTALL_DIR="${FR_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${FR_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1"; }

need curl
need tar

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)                  target="aarch64-apple-darwin" ;;
  Darwin-x86_64)                 target="x86_64-apple-darwin" ;;
  Linux-x86_64)                  target="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64 | Linux-arm64)   target="aarch64-unknown-linux-gnu" ;;
  *) die "no prebuilt binary for $(uname -s) $(uname -m); build from source with: cargo install --git https://github.com/$REPO" ;;
esac

if [ -n "${FR_BASE_URL:-}" ]; then
  base="$FR_BASE_URL"
elif [ "$VERSION" = "latest" ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
  die "need sha256sum or shasum to verify the download"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

archive="fr-$target.tar.gz"
say "Downloading $archive ($VERSION)…"
curl -fsSL "$base/$archive" -o "$tmp/$archive" || die "download failed: $base/$archive"
curl -fsSL "$base/$archive.sha256" -o "$tmp/$archive.sha256" || die "checksum download failed"

expected="$(cut -d ' ' -f 1 "$tmp/$archive.sha256")"
actual="$(sha256 "$tmp/$archive")"
[ "$expected" = "$actual" ] || die "checksum mismatch (expected $expected, got $actual)"

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/fr" "$INSTALL_DIR/fr"

say "Installed $("$INSTALL_DIR/fr" --version) to $INSTALL_DIR/fr"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) say "Note: $INSTALL_DIR is not on your PATH. Add it, e.g.  export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
say ""
say "Last step: enable the shell integration (zsh, after compinit in ~/.zshrc):"
say "  eval \"\$(fr init zsh)\""
