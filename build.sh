#!/usr/bin/env bash
# Builds every pebble to WebAssembly, puts each module next to its manifest and writes
# index.json, the list of pebbles Boulder reads. Needs the wasm32-unknown-unknown target:
#   rustup target add wasm32-unknown-unknown
set -euo pipefail
cd "$(dirname "$0")"

pebbles=()
for manifest in */pebble.json; do
  pebbles+=("$(dirname "$manifest")")
done

packages=()
for pebble in "${pebbles[@]}"; do packages+=(-p "$pebble"); done
cargo build --release --target wasm32-unknown-unknown "${packages[@]}"

for pebble in "${pebbles[@]}"; do
  cp "target/wasm32-unknown-unknown/release/${pebble//-/_}.wasm" "$pebble/pebble.wasm"
done

python3 - "${pebbles[@]}" <<'PY'
import hashlib, json, sys
entries = []
for pebble in sys.argv[1:]:
    with open(f"{pebble}/pebble.json") as f:
        manifest = json.load(f)
    with open(f"{pebble}/pebble.wasm", "rb") as f:
        wasm = f.read()
    manifest["wasm"] = f"{pebble}/pebble.wasm"
    manifest["sha256"] = hashlib.sha256(wasm).hexdigest()
    manifest["size"] = len(wasm)
    entries.append(manifest)
entries.sort(key=lambda m: m["name"].lower())
with open("index.json", "w") as f:
    json.dump({"protocol": 1, "pebbles": entries}, f, indent=2)
    f.write("\n")
print(f"index.json: {len(entries)} pebbles")
PY
