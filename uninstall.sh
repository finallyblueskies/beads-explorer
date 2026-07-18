#!/bin/sh
set -eu

BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"

if [ -f "$BIN_DIR/be" ]; then
    rm "$BIN_DIR/be"
    echo "removed $BIN_DIR/be"
else
    echo "uninstall: $BIN_DIR/be not found" >&2
    exit 1
fi
