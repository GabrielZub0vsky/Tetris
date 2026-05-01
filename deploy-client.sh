#!/usr/bin/env zsh

setopt -euo pipefail

# Note: this would be better as a Makefile (or hashing the contents using another tool)
# also, a better optimization would compile everything in release mode with LTO.

echo "Compiling the client..."
cd client/
cargo build
cd ../

echo "Generating WebAssembly bindings..."
wasm-bindgen --no-typescript --target web \
    --out-dir ./static/ \
    --out-name "client" \
    ./target/wasm32-unknown-unknown/debug/client.wasm

# echo "Optimizing the WebAssembly module..."
# cd site/
# wasm-opt -Oz -o client_bg.wasm client_bg.wasm
