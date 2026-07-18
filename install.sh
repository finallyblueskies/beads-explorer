#!/bin/sh
set -eu

BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"

if ! command -v cargo >/dev/null 2>&1; then
    echo "install: cargo is required (https://rustup.rs)" >&2
    exit 1
fi

cd "$(dirname "$0")"
cargo build --release
mkdir -p "$BIN_DIR"
install -m 755 target/release/be "$BIN_DIR/be"
echo "installed $BIN_DIR/be"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "note: $BIN_DIR is not on your PATH" ;;
esac
