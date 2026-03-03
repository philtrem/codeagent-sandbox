#!/usr/bin/env bash
#
# Build the sandbox binary and guest VM images, then copy them to
# the Tauri bundle directories (sidecar + resources).
#
# Usage:
#   ./desktop/scripts/copy-sidecar.sh           # release build
#   ./desktop/scripts/copy-sidecar.sh --debug    # debug build

set -euo pipefail

# Ensure cargo is in PATH (Git Bash on Windows doesn't source .cargo/env)
if ! command -v cargo &>/dev/null; then
    if [[ -f "$HOME/.cargo/env" ]]; then
        source "$HOME/.cargo/env"
    else
        echo "error: cargo not found. Install Rust from https://rustup.rs/" >&2
        exit 1
    fi
fi

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BINARIES_DIR="$REPO_ROOT/desktop/src-tauri/binaries"
RESOURCES_DIR="$REPO_ROOT/desktop/src-tauri/resources/guest"

PROFILE="release"
if [[ "${1:-}" == "--debug" ]]; then
    PROFILE="debug"
fi

# Detect target triple and guest architecture
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64)
        TARGET="x86_64-unknown-linux-gnu"
        GUEST_ARCH="x86_64"
        ;;
    Linux-aarch64)
        TARGET="aarch64-unknown-linux-gnu"
        GUEST_ARCH="aarch64"
        ;;
    Darwin-x86_64)
        TARGET="x86_64-apple-darwin"
        GUEST_ARCH="x86_64"
        ;;
    Darwin-arm64)
        TARGET="aarch64-apple-darwin"
        GUEST_ARCH="aarch64"
        ;;
    MINGW*|MSYS*|CYGWIN*)
        TARGET="x86_64-pc-windows-msvc"
        GUEST_ARCH="x86_64"
        ;;
    *)
        echo "Unsupported platform: $(uname -s)-$(uname -m)" >&2
        exit 1
        ;;
esac

# --- Sandbox binary ---

echo "Building sandbox binary ($PROFILE, $TARGET)..."
if [[ "$PROFILE" == "release" ]]; then
    cargo build --release -p codeagent-sandbox --manifest-path "$REPO_ROOT/Cargo.toml"
    SOURCE="$REPO_ROOT/target/release"
else
    cargo build -p codeagent-sandbox --manifest-path "$REPO_ROOT/Cargo.toml"
    SOURCE="$REPO_ROOT/target/debug"
fi

mkdir -p "$BINARIES_DIR"

if [[ "$TARGET" == *windows* ]]; then
    SRC_NAME="sandbox.exe"
    DST_NAME="sandbox-${TARGET}.exe"
else
    SRC_NAME="sandbox"
    DST_NAME="sandbox-${TARGET}"
fi

cp "$SOURCE/$SRC_NAME" "$BINARIES_DIR/$DST_NAME"
echo "Copied sidecar: $BINARIES_DIR/$DST_NAME"

# --- Guest VM images ---

GUEST_DIR="$REPO_ROOT/target/guest/$GUEST_ARCH"

# Build guest images if Docker is available and they don't already exist
if [[ ! -f "$GUEST_DIR/vmlinuz" || ! -f "$GUEST_DIR/initrd.img" ]]; then
    if command -v docker &>/dev/null; then
        echo ""
        echo "Building guest VM image ($GUEST_ARCH)..."
        cargo xtask build-guest --arch "$GUEST_ARCH" --manifest-path "$REPO_ROOT/xtask/Cargo.toml" 2>/dev/null \
            || cargo run --manifest-path "$REPO_ROOT/xtask/Cargo.toml" -- build-guest --arch "$GUEST_ARCH" \
            || { echo "warning: guest image build failed (Docker may not be running)" >&2; }
    else
        echo ""
        echo "warning: Docker not found — skipping guest image build." >&2
        echo "  Run 'cargo xtask build-guest' manually, then re-run this script." >&2
    fi
fi

mkdir -p "$RESOURCES_DIR"

if [[ -f "$GUEST_DIR/vmlinuz" && -f "$GUEST_DIR/initrd.img" ]]; then
    cp "$GUEST_DIR/vmlinuz" "$RESOURCES_DIR/vmlinuz"
    cp "$GUEST_DIR/initrd.img" "$RESOURCES_DIR/initrd.img"
    echo "Copied guest images: $RESOURCES_DIR/{vmlinuz,initrd.img}"
else
    echo ""
    echo "warning: Guest images not found at $GUEST_DIR" >&2
    echo "  The desktop app will work but cannot start the VM without kernel/initrd." >&2
    echo "  Build them with: cargo xtask build-guest --arch $GUEST_ARCH" >&2
fi
