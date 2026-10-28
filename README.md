# Boulder pebbles

Pebbles are optional add-ons for [Boulder](https://github.com/ayushO4-dev/boulder) servers.
They aren't part of Boulder: a server's owner installs them from the Pebbles page, and Boulder
downloads them from this repository.

Each pebble is a sandboxed WebAssembly module. It can only do what Boulder allows (read and
change the server's settings and files, make https requests, run programs it downloaded) and it
describes its own page, which the dashboard draws with Boulder's components. The contract
between a pebble and Boulder is in [PROTOCOL.md](PROTOCOL.md).

## Pebbles

| Pebble | What it does |
|---|---|
| [Minekube Connect](minekube-connect) | A free `<name>.play.minekube.net` address without port forwarding, and a private switch with a whitelist |
| [Playit.gg](playit-gg) | Forwards tunnels from playit.gg to the server |

## Layout

```
index.json          every pebble, read by Boulder (written by build.sh)
<pebble>/           a pebble: pebble.json (its manifest), pebble.wasm (built), src/
sdk/                boulder-pebble, a Rust crate for writing pebbles
build.sh            builds every pebble and writes index.json
```

## Building

```bash
rustup target add wasm32-unknown-unknown
./build.sh
```

`build.sh` builds every folder with a `pebble.json`, copies its module next to the manifest and
writes `index.json` with each module's sha256, which Boulder checks before running it. Commit the
built modules and `index.json` together.

## Developing

Point a Boulder core at a local checkout instead of GitHub:

```bash
BOULDER_PEBBLES=/path/to/boulder-pebbles cargo run -p boulder_core -- --is-cli
```

Boulder keeps the module it downloaded, and fetches a pebble again when the version in
`pebble.json` changes, so bump the version when you change a pebble.

## Writing a pebble

Make a folder with a `pebble.json` and a Rust crate built as a `cdylib`, depend on `../sdk` and
handle events:

```rust
use boulder_pebble::{self as host, Event, Value, json};

fn handle(event: Event) -> host::Result<Value> {
    match event.name.as_str() {
        "view" => Ok(json!({ "view": { "sections": [] } })),
        _ => Ok(json!({})),
    }
}

host::pebble!(handle);
```

Add it to the workspace in `Cargo.toml` and run `./build.sh`. Pebbles can be written in any
language that compiles to WebAssembly, as long as they follow PROTOCOL.md.
