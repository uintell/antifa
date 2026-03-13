#!/usr/bin/env bash
set -euo pipefail

out_dir="target/x86_64-pc-windows-gnu/release"

cargo build --release --target x86_64-pc-windows-gnu

printf '\nBuilt %s\n' "${out_dir}/antifa.exe"
