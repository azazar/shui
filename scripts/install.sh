#!/bin/sh

# Keep the entry point last so a truncated download cannot start installation.
install_shui() (
  set -eu
  version=v0.1.0-rc.13
  case "$(uname -s):$(uname -m)" in
    Linux:x86_64) target=x86_64-unknown-linux-musl ;;
    Linux:aarch64|Linux:arm64) target=aarch64-unknown-linux-musl ;;
    Linux:armv7l) target=armv7-unknown-linux-musleabihf ;;
    Darwin:arm64) target=aarch64-apple-darwin ;;
    Darwin:x86_64) target=x86_64-apple-darwin ;;
    *) echo 'Unsupported platform; see https://github.com/azazar/shui#installation.' >&2; exit 1 ;;
  esac
  archive="shui-$target.tar.gz"
  base="https://github.com/azazar/shui/releases/download/$version"
  destination=${SHUI_INSTALL_DIR:-"$HOME/.local/bin"}
  case "$destination" in
    /*) ;;
    *) echo 'SHUI_INSTALL_DIR must be an absolute path.' >&2; exit 1 ;;
  esac
  temporary=$(mktemp -d)
  trap 'rm -rf "$temporary"' EXIT
  trap 'exit 1' HUP INT TERM
  cd "$temporary"
  curl --fail --location --proto '=https' --output "$archive" "$base/$archive"
  curl --fail --location --proto '=https' --output SHA256SUMS "$base/SHA256SUMS"
  awk -v name="$archive" '$2 == name { print; count++ } END { if (count != 1) exit 1 }' SHA256SUMS > checksum
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c checksum
  else
    shasum -a 256 -c checksum
  fi
  tar -xzf "$archive" shui
  mkdir -p "$destination"
  staged=$(mktemp "$destination/.shui-install.XXXXXX")
  trap 'rm -rf "$temporary"; rm -f "$staged"' EXIT
  install -m 755 shui "$staged"
  mv -f "$staged" "$destination/shui"
  printf 'Installed Shui %s to %s\n' "$version" "$destination/shui"
  case ":$PATH:" in
    *":$destination:"*) printf 'Run: shui\n' ;;
    *) printf 'Add %s to your PATH, then run shui.\n' "$destination" ;;
  esac
)

install_shui
