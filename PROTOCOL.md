# Pebble protocol (version 1)

A pebble is a WebAssembly module (`wasm32-unknown-unknown`, no WASI) that Boulder downloads when
a server installs it. Boulder runs it in a sandbox: a pebble can't touch files, the network or
processes on its own. It asks Boulder through one function, and Boulder only does what this
protocol allows, limited to the server the pebble runs for and the pebble's own data folder.

Everything crossing the boundary is UTF-8 JSON.

## Module interface

The module exports:

| Export | Signature | Purpose |
|---|---|---|
| `memory` | memory | the module's memory |
| `boulder_alloc` | `(size: u32) -> u32` | allocates `size` bytes and returns a pointer, so Boulder can write into the module |
| `boulder_handle` | `(ptr: u32, len: u32) -> u64` | handles one event (JSON at `ptr`/`len`) and returns its reply, packed as `ptr << 32 \| len` |

The module imports one function from the `boulder` module:

| Import | Signature | Purpose |
|---|---|---|
| `boulder.host_call` | `(ptr: u32, len: u32) -> u64` | runs one host operation (JSON at `ptr`/`len`); the reply is written with `boulder_alloc` and returned packed as `ptr << 32 \| len` |

Boulder creates a fresh instance for every event, so a pebble keeps nothing in memory between
events; it stores what it needs in files (see `fs.*`). Events for the same pebble on the same
server never run at the same time. An event that runs longer than 120 seconds is stopped.

## Events

Boulder calls `boulder_handle` with `{"event": "<name>", ...}`. A reply of `{"error": "message"}`
fails the event and the message is shown to the user; any other object is a success.

| Event | Extra fields | Reply | When |
|---|---|---|---|
| `install` | | `{}` | the pebble is installed on a server |
| `uninstall` | | `{}` | the pebble is removed; undo every change it made to the server |
| `enable` | | `{}` | the pebble is switched back on |
| `disable` | | `{}` | the pebble is switched off; stop doing anything, keep its settings |
| `server_state` | `state`: `Starting`, `Running`, `Stopping`, `Stopped` or `Error` | `{}` | the server changes state (only while enabled) |
| `core_start` | | `{}` | Boulder starts (only while enabled) |
| `address` | | `{"address": "host:port" \| null}` | Boulder asks for the public address the pebble gives the server, shown on its overview |
| `view` | | `{"view": View}` | the pebble's page is opened or refreshed |
| `action` | `action`: item id, `value` | `{"view"?: View, "message"?: string}` | the user used an item on the pebble's page |

`uninstall` and `disable` are also sent when the server can't be reached by the pebble's
changes any more, so they should succeed whenever possible.

## Host operations

The pebble calls `host_call` with `{"op": "<name>", ...}`. The reply is the result object, or
`{"error": "message"}`.

Paths are relative to the server's folder, or start with `pebble://` for the pebble's own data
folder, which is shared by every server that has the pebble (for downloaded programs and account
keys). Absolute paths and `..` are refused.

| Operation | Fields | Result |
|---|---|---|
| `server` | | `{"uuid", "name", "port", "state", "variant", "version"}` (`variant`: `vanilla`, `fabric`, `paper`, `spigot`, `forge`, `neoforge`) |
| `servers` | | `{"servers": [{"uuid", "name", "port", "state"}]}`: every server with this pebble installed and on |
| `properties.get` | `keys: [string]` | `{"values": {key: string \| null}}` from `server.properties` |
| `properties.set` | `values: {key: string}` | `{}`; Boulder keeps the server's settings in step |
| `console` | `command` | `{}`; runs a console command, fails when the server isn't running |
| `fs.read` | `path` | `{"text": string \| null}`; `null` when the file doesn't exist |
| `fs.write` | `path`, `text` | `{}`; creates missing folders |
| `fs.remove` | `path` | `{}`; removes a file or folder, nothing if it doesn't exist |
| `fs.exists` | `path` | `{"exists": bool}` |
| `http` | `method`, `url`, `headers?: {name: value}`, `body?: string` | `{"status": number, "body": string}`; HTTPS only |
| `download` | `url`, `path`, `executable?: bool` | `{}`; saves a file (HTTPS only) |
| `process.start` | `name`, `path`, `args?: [string]`, `cwd?`, `log?`, `shared?: bool` | `{}`; starts a program the pebble downloaded, replacing one with the same name |
| `process.stop` | `name`, `shared?: bool` | `{}` |
| `process.running` | `name`, `shared?: bool` | `{"running": bool}` |
| `platform` | | `{"os": "linux" \| "macos" \| "windows", "arch": "x86_64" \| "aarch64"}` |
| `random` | `bytes: number` | `{"hex": string}` |
| `log` | `level: "info" \| "warn" \| "error"`, `message` | `{}` |

Programs must live in the server's folder or the pebble's data folder. A program is named per
server, or with `shared: true` once for the whole Boulder core. Boulder stops a pebble's programs
when the pebble is switched off or removed, and all of them when it shuts down.

## Views

A pebble's page is a list of sections, drawn by the dashboard with Boulder's own components.

```json
{
  "sections": [
    { "title": "Minekube Connect", "description": "A free address…", "items": [ … ] }
  ],
  "refresh_seconds": 10
}
```

`refresh_seconds` asks the dashboard to send `view` again after that many seconds.

| Item `type` | Fields | `action` value |
|---|---|---|
| `toggle` | `id`, `label`, `description`, `value: bool`, `disabled?`, `confirm?: {"title", "body", "button"}` | the new `bool` |
| `text` | `id`, `label`, `description`, `value`, `placeholder?`, `pattern?`, `pattern_error?`, `disabled?` | the new `string` |
| `button` | `id`, `label`, `description`, `button`, `style?: "primary" \| "danger" \| "default"`, `disabled?` | `null` |
| `copy` | `label`, `description`, `value`, `status?: {"text", "color": "green" \| "yellow" \| "red" \| "gray"}` | none |
| `link` | `label`, `description`, `url`, `button` | none (opens the link) |
| `info` | `text`, `color?` | none |
| `list` | `id`, `label`, `description`, `rows: [{"key", "title", "subtitle?", "image?", "removable?"}]`, `add?: {"placeholder", "button"}`, `empty?`, `disabled?` | `{"add": string}` or `{"remove": key}` |

## Repository layout

```
index.json              every pebble in the repo, read by Boulder
<pebble>/pebble.json    the pebble's manifest
<pebble>/pebble.wasm    the built module Boulder downloads
<pebble>/src/           its source
sdk/                    a Rust crate for writing pebbles
build.sh                builds every pebble and writes index.json
```

A manifest:

```json
{
  "id": "minekube-connect",
  "name": "Minekube Connect",
  "tagline": "One line for the card",
  "description": "A paragraph",
  "author": "Boulder",
  "version": "1.0.0",
  "icon": "globe",
  "category": "Networking",
  "supports": ["vanilla", "fabric", "paper", "spigot", "forge", "neoforge"]
}
```

`index.json` holds every manifest plus `wasm` (path of the module in the repo) and `sha256`
(of the module), which Boulder checks before running it.
