#!/usr/bin/env bash
# Build the GPUI app for wasm32 and write the files the host page loads
# (`cloudflare/public/fastdash.js` and `fastdash_bg.wasm`).
set -euo pipefail
cd "$(dirname "$0")/.."

profile=release
target_dir="target/wasm32-unknown-unknown/release"
if [[ "${1:-}" == "--debug" ]]; then
  profile=dev
  target_dir="target/wasm32-unknown-unknown/debug"
fi

args=(build -p fastdash-app --target wasm32-unknown-unknown)
if [[ "$profile" == release ]]; then
  args+=(--release)
fi
cargo "${args[@]}"

version="$(cargo metadata --format-version 1 --filter-platform wasm32-unknown-unknown \
  | python3 -c 'import json,sys; packages=json.load(sys.stdin)["packages"]; print(next(p["version"] for p in packages if p["name"]=="wasm-bindgen"))')"

if ! wasm-bindgen --version 2>/dev/null | grep -q "$version"; then
  cargo install wasm-bindgen-cli --version "$version" --locked || cargo install wasm-bindgen-cli --version "$version"
fi

wasm-bindgen --target web --out-dir cloudflare/public "$target_dir/fastdash.wasm"
echo "wrote cloudflare/public/fastdash.js (wasm-bindgen $version)"
