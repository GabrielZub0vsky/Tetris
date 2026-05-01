#!/usr/bin/env zsh

setopt -eu

cd server/
cargo run -- "$@"
