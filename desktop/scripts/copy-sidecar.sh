#!/usr/bin/env bash
#
# Build the sandbox binary and copy it to the Tauri sidecar directory
# with the target triple suffix that Tauri expects.
#
# Usage:
#   ./desktop/scripts/copy-sidecar.sh           # release build
#   ./desktop/scripts/copy-sidecar.sh --debug    # debug build

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BINARIES_DIR="$REPO_ROOT/desktop/src-tauri/binaries"

PROFILE="release"
if [[ "${1:-}" == "--debug" ]]; then
    PROFILE="debug"
fi

# Detect target triple
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64)   TARGET="x86_64-unknown-linux-gnu" ;;
    Linux-aarch64)  TARGET="aarch64-unknown-linux-gnu" ;;
    Darwin-x86_64)  TARGET="x86_64-apple-darwin" ;;
    Darwin-arm64)   TARGET="aarch64-apple-darwin" ;;
    MINGW*|MSYS*|CYGWIN*)
        TARGET="x86_64-pc-windows-msvc"
        ;;
    *)
        echo "Unsupported platform: $(uname -s)-$(uname -m)" >&2
        exit 1
        ;;
esac

echo "Building sandbox binary ($PROFILE, $TARGET)..."
if [[ "$PROFILE" == "release" ]]; then
    cargo build --release -p codeagent-sandbox --manifest-path "$REPO_ROOT/Cargo.toml"
    SOURCE="$REPO_ROOT/target/release"
else
    cargo build -p codeagent-sandbox --manifest-path "$REPO_ROOT/Cargo.toml"
    SOURCE="$REPO_ROOT/target/debug"
fi

mkdir -p "$BINARIES_DIR"

# Determine binary name with extension
if [[ "$TARGET" == *windows* ]]; then
    SRC_NAME="sandbox.exe"
    DST_NAME="sandbox-${TARGET}.exe"
else
    SRC_NAME="sandbox"
    DST_NAME="sandbox-${TARGET}"
fi

cp "$SOURCE/$SRC_NAME" "$BINARIES_DIR/$DST_NAME"
echo "Copied $BINARIES_DIR/$DST_NAME"
