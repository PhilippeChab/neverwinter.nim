#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

echo "Building WASM..."
cargo build --target wasm32-unknown-unknown --release

echo "Generating JS bindings..."
wasm-bindgen \
    --target nodejs \
    --out-dir pkg \
    target/wasm32-unknown-unknown/release/nwscript_compiler.wasm

echo ""
echo "Output:"
ls -la pkg/nwscript_compiler_bg.wasm pkg/nwscript_compiler.js
echo ""
echo "Gzipped size:"
gzip -c pkg/nwscript_compiler_bg.wasm | wc -c | awk '{printf "  %.1f KB\n", $1/1024}'

echo ""
echo "Running WASM tests..."
node test_wasm.js
