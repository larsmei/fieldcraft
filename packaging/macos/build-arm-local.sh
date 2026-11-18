#!/usr/bin/env bash
# Build the macOS Apple-silicon package on this machine.
#
#   packaging/macos/build-arm-local.sh
#
# Run on an Apple Silicon Mac with the Xcode command line tools and rustup.
# Ad-hoc signed, not notarized. Writes:
#   dist/release/fieldcraft-<version>-macos-aarch64.dmg
#   dist/release/fieldcraft-cli-<version>-macos-aarch64.zip
#
# Gatekeeper blocks the first launch. Right-click FieldCraft.app and choose
# Open, or run: xattr -dr com.apple.quarantine on the app.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
cd "$ROOT"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "error: this packages a Mac app and must run on macOS (codesign, hdiutil)." >&2
  exit 1
fi
if [ "$(uname -m)" != "arm64" ]; then
  echo "error: this builds the arm64 package and must run on Apple Silicon." >&2
  exit 1
fi

for cmd in cargo rustc codesign hdiutil lipo plutil ditto; do
  if ! command -v "$cmd" >/dev/null; then
    echo "error: $cmd is not on PATH (install the Xcode command line tools and rustup)" >&2
    exit 1
  fi
done

if command -v rustup >/dev/null; then
  rustup target add aarch64-apple-darwin
fi

echo "==> third-party licences"
cargo xtask licences

echo "==> package"
exec "$HERE/package.sh" --arch aarch64
