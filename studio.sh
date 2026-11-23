#!/usr/bin/env sh
# Migrate Anything Studio launcher for macOS and Linux.
#   ./studio.sh        opens Studio
#   ./studio.sh demo   opens Studio with the demo ready
# Keep the terminal open while you work: approval codes are shown here.
set -eu
cd "$(dirname "$0")"
EXE="target/release/migrate-anything"

if command -v cargo >/dev/null 2>&1; then
  echo "Preparing Migrate Anything (the first start takes a few minutes)..."
  cargo build --release --locked -p migrate-anything
elif [ ! -x "$EXE" ]; then
  echo "Rust is needed to build Migrate Anything the first time."
  echo "Install it from https://rustup.rs and run this script again."
  exit 1
fi
exec "$EXE" "$@"
