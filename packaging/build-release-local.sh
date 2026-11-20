#!/usr/bin/env bash
# Build this machine's release packages in one step.
#
#   packaging/build-release-local.sh
#
# Linux x86_64 writes dist/release/fieldcraft-<version>-linux-x86_64.tar.gz,
# the AppImage, the .deb and the .rpm. macOS Apple silicon writes the .dmg and
# the CLI zip (ad-hoc signed, not notarized). Windows is
# packaging/windows/build-release-local.ps1. A Mac or Windows package cannot
# be produced from Linux.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT"

os="$(uname -s)"
arch="$(uname -m)"

case "$os-$arch" in
  Darwin-arm64)
    exec "$HERE/macos/build-arm-local.sh"
    ;;
  Linux-x86_64)
    ;;
  Darwin-*)
    echo "error: this Mac is $arch. The release package is Apple silicon (arm64)." >&2
    exit 1
    ;;
  Linux-*)
    echo "error: this machine is $arch. The Linux release package is x86_64." >&2
    exit 1
    ;;
  *)
    echo "error: $os is not packaged by this script. On Windows run: pwsh packaging/windows/build-release-local.ps1" >&2
    exit 1
    ;;
esac

for cmd in cargo rustc curl tar strip sha256sum; do
  if ! command -v "$cmd" >/dev/null; then
    echo "error: $cmd is not on PATH" >&2
    exit 1
  fi
done

install_cargo_tool() {
  local bin="$1" version="$2" crate="$3"
  if command -v "$bin" >/dev/null; then
    return 0
  fi
  echo "==> install $crate $version"
  cargo install --locked --version "$version" "$crate"
}

install_cargo_tool cargo-deb 3.8.0 cargo-deb
install_cargo_tool cargo-generate-rpm 0.21.0 cargo-generate-rpm

echo "==> third-party licences"
cargo xtask licences

echo "==> package"
exec "$HERE/linux/package.sh"
