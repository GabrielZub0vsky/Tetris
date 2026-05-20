#!/usr/bin/env bash

set -eu

cd server/
cargo nt --no-tests=pass -- "$@"
