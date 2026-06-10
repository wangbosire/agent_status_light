#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist"
KEEP_STAGING=0
SKIP_BUILD=0
PLATFORM="all"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform)
      PLATFORM="$2"
      shift 2
      ;;
    --keep)
      KEEP_STAGING=1
      shift
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --out)
      DIST_DIR="$2"
      shift 2
      ;;
    -h|--help)
      cat <<'HELP'
Usage: scripts/package.sh [--platform all|macos|windows] [--keep] [--skip-build] [--out DIR]

Build and package AgentStatusLight desktop tools for distribution.

What this script does:
  1. Builds release binaries with cargo.
  2. Creates a clean package folder for each platform.
  3. Copies the executable, USER_MANUAL.md, and README.txt into the folder.
  4. Compresses the folder into a zip file under ./dist by default.

Default behavior:
  scripts/package.sh
  Builds both:
    - macOS package for the current Mac architecture
    - Windows x64 package using x86_64-pc-windows-gnu

Options:
  --platform all|macos|windows
      Select which package(s) to build.
      Default: all.

  --keep
      Keep the temporary staging folders next to the zip files.
      Useful when you want to inspect package contents before sending.

  --skip-build
      Do not run cargo build.
      Reuse existing release binaries from target/<target>/release/.
      Useful when you already built binaries and only want to rebuild zip files.

  --out DIR
      Write package output to DIR instead of ./dist.

  -h, --help
      Show this help message.

Generated files:
  dist/AgentStatusLight-<version>-macos-<arch>.zip
  dist/AgentStatusLight-<version>-windows-x64.zip

Package contents:
  macOS:
    agent_status_light
    USER_MANUAL.md
    README.txt

  Windows:
    agent_status_light.exe
    USER_MANUAL.md
    README.txt

Examples:
  scripts/package.sh
      Build macOS and Windows packages.

  scripts/package.sh --platform macos
      Build only the macOS package.

  scripts/package.sh --platform windows
      Build only the Windows x64 package.

  scripts/package.sh --skip-build
      Reuse existing release binaries and regenerate zip files.

  scripts/package.sh --keep
      Keep staging folders under ./dist for inspection.

  scripts/package.sh --out /tmp/asl-dist
      Write packages to /tmp/asl-dist.

Requirements:
  - Rust and cargo must be installed.
  - zip is recommended. If zip is unavailable, the script falls back to tar.gz.
  - Windows packaging requires the Rust target:
      x86_64-pc-windows-gnu

    Install it with:
      rustup target add x86_64-pc-windows-gnu

Notes:
  - macOS packaging uses the current Mac host target, for example:
      aarch64-apple-darwin -> macos-arm64
      x86_64-apple-darwin  -> macos-x64
  - macOS packages can only be built on a macOS Rust host.
  - Windows packages are cross-compiled from the current host.
  - The generated package is intended for end users. They do not need source code.
HELP
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

cd "$ROOT_DIR"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
if [[ -z "$VERSION" ]]; then
  echo "failed to read package version from Cargo.toml" >&2
  exit 1
fi

HOST_TARGET="$(rustc -vV | sed -n 's/^host: //p')"
if [[ -z "$HOST_TARGET" ]]; then
  echo "failed to read rust host target" >&2
  exit 1
fi

case "$HOST_TARGET" in
  aarch64-apple-darwin) MAC_ARCH="arm64" ;;
  x86_64-apple-darwin) MAC_ARCH="x64" ;;
  *)
    MAC_ARCH=""
    ;;
esac

WINDOWS_TARGET="x86_64-pc-windows-gnu"

require_target() {
  local target="$1"
  if ! rustup target list --installed | grep -qx "$target"; then
    echo "missing Rust target: $target" >&2
    echo "install it with: rustup target add $target" >&2
    exit 1
  fi
}

write_readme() {
  local output_dir="$1"
  cat > "$output_dir/README.txt" <<EOF
AgentStatusLight $VERSION

这个压缩包给普通用户使用，不需要源码，也不需要编译。

快速测试：

macOS:
  ./agent_status_light send --mode demo
  ./agent_status_light send --mode off

Windows:
  .\\agent_status_light.exe send --mode demo
  .\\agent_status_light.exe send --mode off

安装 Hook：

macOS:
  ./agent_status_light install cursor
  ./agent_status_light install codex
  ./agent_status_light install claude

Windows:
  .\\agent_status_light.exe install cursor
  .\\agent_status_light.exe install codex
  .\\agent_status_light.exe install claude

排障：
  ./agent_status_light status --verbose
  ./agent_status_light logs --limit 100

完整说明见 USER_MANUAL.md。
EOF
}

make_archive() {
  local package_name="$1"
  local staging_dir="$DIST_DIR/$package_name"
  local zip_path="$DIST_DIR/$package_name.zip"

  rm -f "$zip_path"
  if command -v zip >/dev/null 2>&1; then
    (
      cd "$DIST_DIR"
      zip -qr "$zip_path" "$package_name"
    )
    CREATED_PACKAGES+=("$zip_path")
  else
    local tar_path="$DIST_DIR/$package_name.tar.gz"
    rm -f "$tar_path"
    tar -czf "$tar_path" -C "$DIST_DIR" "$package_name"
    CREATED_PACKAGES+=("$tar_path")
  fi

  if [[ "$KEEP_STAGING" -eq 0 ]]; then
    rm -rf "$staging_dir"
  fi
}

package_target() {
  local target="$1"
  local os_name="$2"
  local arch_name="$3"
  local bin_name="$4"

  require_target "$target"

  if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "building release binary for $os_name-$arch_name ($target)..."
    cargo build --release --target "$target"
  fi

  local target_bin="$ROOT_DIR/target/$target/release/$bin_name"
  if [[ ! -f "$target_bin" ]]; then
    echo "release binary not found: $target_bin" >&2
    echo "run scripts/package.sh without --skip-build first" >&2
    exit 1
  fi

  local package_name="AgentStatusLight-$VERSION-$os_name-$arch_name"
  local staging_dir="$DIST_DIR/$package_name"

  mkdir -p "$DIST_DIR"
  rm -rf "$staging_dir"
  mkdir -p "$staging_dir"

  cp "$target_bin" "$staging_dir/$bin_name"
  chmod +x "$staging_dir/$bin_name" 2>/dev/null || true
  cp "$ROOT_DIR/docs/USER_MANUAL.md" "$staging_dir/USER_MANUAL.md"
  write_readme "$staging_dir"
  make_archive "$package_name"
}

CREATED_PACKAGES=()

case "$PLATFORM" in
  all|macos|windows) ;;
  *)
    echo "invalid --platform value: $PLATFORM" >&2
    echo "expected one of: all, macos, windows" >&2
    exit 1
    ;;
esac

if [[ "$PLATFORM" == "all" || "$PLATFORM" == "macos" ]]; then
  if [[ -z "$MAC_ARCH" ]]; then
    echo "macOS package can only be built on a macOS Rust host; current host is $HOST_TARGET" >&2
    exit 1
  fi
  package_target "$HOST_TARGET" "macos" "$MAC_ARCH" "agent_status_light"
fi

if [[ "$PLATFORM" == "all" || "$PLATFORM" == "windows" ]]; then
  package_target "$WINDOWS_TARGET" "windows" "x64" "agent_status_light.exe"
fi

echo "packages created:"
for package in "${CREATED_PACKAGES[@]}"; do
  echo "$package"
done
