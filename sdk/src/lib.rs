//! Write pebbles, Boulder's add-ons, in Rust. See PROTOCOL.md in the repository.
//!
//! A pebble is a function from an [`Event`] to a JSON reply, registered with [`pebble!`]:
//!
//! ```ignore
//! fn handle(event: Event) -> Result<Value, String> {
//!     match event.name.as_str() {
//!         "view" => Ok(json!({ "view": { "sections": [] } })),
//!         _ => Ok(json!({})),
//!     }
//! }
//! boulder_pebble::pebble!(handle);
//! ```
//!
//! The functions in this crate ask Boulder to do things for the pebble.

use std::collections::HashMap;

pub use serde_json::{Value, json};

/// An event Boulder sent: its name and every other field
#[derive(Debug, Clone)]
pub struct Event {
    pub name: String,
    pub data: Value,
}

impl Event {
    /// A string field of the event, empty if missing
    pub fn str(&self, field: &str) -> &str {
        self.data.get(field).and_then(Value::as_str).unwrap_or_default()
    }

    pub fn get(&self, field: &str) -> &Value {
        self.data.get(field).unwrap_or(&Value::Null)
    }
}

pub type Result<T> = std::result::Result<T, String>;

// ---------------------------------------------------------------- the module interface

#[doc(hidden)]
#[unsafe(no_mangle)]
pub extern "C" fn boulder_alloc(size: u32) -> u32 {
    let mut buffer = Vec::<u8>::with_capacity(size as usize);
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr as u32
}

/// Hands bytes to Boulder: they stay allocated, the instance ends after the event anyway
fn pack(bytes: Vec<u8>) -> u64 {
    let len = bytes.len() as u64;
    let ptr = Box::leak(bytes.into_boxed_slice()).as_ptr() as u64;
    (ptr << 32) | len
}

/// # Safety
/// `packed` must come from Boulder, pointing at memory it wrote into this module
unsafe fn unpack(packed: u64) -> Vec<u8> {
    let ptr = (packed >> 32) as usize as *const u8;
    let len = (packed & 0xffff_ffff) as usize;
    unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "boulder")]
unsafe extern "C" {
    fn host_call(ptr: u32, len: u32) -> u64;
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn host_call(_ptr: u32, _len: u32) -> u64 {
    panic!("pebbles only run inside Boulder")
}

#[doc(hidden)]
pub fn dispatch(ptr: u32, len: u32, handler: fn(Event) -> Result<Value>) -> u64 {
    let input = unsafe { std::slice::from_raw_parts(ptr as usize as *const u8, len as usize) };
    let reply = match serde_json::from_slice::<Value>(input) {
        Ok(data) => {
            let name = data["event"].as_str().unwrap_or_default().to_string();
            match handler(Event { name, data }) {
                Ok(Value::Null) => json!({}),
                Ok(value) => value,
                Err(error) => json!({ "error": error }),
            }
        }
        Err(error) => json!({ "error": format!("Invalid event: {error}") }),
    };
    pack(serde_json::to_vec(&reply).unwrap_or_default())
}

/// Registers the function that handles the pebble's events
#[macro_export]
macro_rules! pebble {
    ($handler:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn boulder_handle(ptr: u32, len: u32) -> u64 {
            $crate::dispatch(ptr, len, $handler)
        }
    };
}

/// Runs a host operation, see PROTOCOL.md
pub fn call(op: &str, mut args: Value) -> Result<Value> {
    if !args.is_object() {
        args = json!({});
    }
    args["op"] = json!(op);
    let request = serde_json::to_vec(&args).map_err(|e| e.to_string())?;
    let reply = unsafe { unpack(host_call(request.as_ptr() as u32, request.len() as u32)) };
    let reply: Value = serde_json::from_slice(&reply).map_err(|e| e.to_string())?;
    match reply.get("error").and_then(Value::as_str) {
        Some(error) => Err(error.to_string()),
        None => Ok(reply),
    }
}

// ---------------------------------------------------------------- the server

/// The server the pebble runs for
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Server {
    pub uuid: String,
    pub name: String,
    pub port: u32,
    /// `Starting`, `Running`, `Stopping`, `Stopped` or `Error`
    pub state: String,
    /// `vanilla`, `fabric`, `paper`, `spigot`, `forge` or `neoforge`
    #[serde(default)]
    pub variant: String,
    #[serde(default)]
    pub version: String,
}

pub fn server() -> Result<Server> {
    serde_json::from_value(call("server", json!({}))?).map_err(|e| e.to_string())
}

/// Every server with this pebble installed and on
pub fn servers() -> Result<Vec<Server>> {
    serde_json::from_value(call("servers", json!({}))?["servers"].take())
        .map_err(|e| e.to_string())
}

/// Values from server.properties, `None` for keys that aren't set
pub fn properties(keys: &[&str]) -> Result<HashMap<String, Option<String>>> {
    serde_json::from_value(call("properties.get", json!({ "keys": keys }))?["values"].take())
        .map_err(|e| e.to_string())
}

pub fn set_properties(values: &[(&str, String)]) -> Result<()> {
    let values: HashMap<&str, &String> = values.iter().map(|(k, v)| (*k, v)).collect();
    call("properties.set", json!({ "values": values })).map(|_| ())
}

/// Runs a console command; fails when the server isn't running
pub fn console(command: &str) -> Result<()> {
    call("console", json!({ "command": command })).map(|_| ())
}

// ---------------------------------------------------------------- files

/// Reads a text file, `None` when it doesn't exist. Paths are relative to the server's folder,
/// or start with `pebble://` for the pebble's own data folder.
pub fn read(path: &str) -> Result<Option<String>> {
    Ok(call("fs.read", json!({ "path": path }))?["text"].as_str().map(String::from))
}

pub fn write(path: &str, text: &str) -> Result<()> {
    call("fs.write", json!({ "path": path, "text": text })).map(|_| ())
}

pub fn remove(path: &str) -> Result<()> {
    call("fs.remove", json!({ "path": path })).map(|_| ())
}

pub fn exists(path: &str) -> Result<bool> {
    Ok(call("fs.exists", json!({ "path": path }))?["exists"].as_bool().unwrap_or(false))
}

/// Reads a JSON file into `T`, or `T::default()` when it is missing or unreadable
pub fn read_json<T: serde::de::DeserializeOwned + Default>(path: &str) -> T {
    read(path)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn write_json<T: serde::Serialize>(path: &str, value: &T) -> Result<()> {
    write(path, &serde_json::to_string_pretty(value).map_err(|e| e.to_string())?)
}

// ---------------------------------------------------------------- network

pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_str(&self.body).map_err(|e| format!("Unexpected reply: {e}"))
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// An HTTPS request
pub fn http(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&str>) -> Result<Response> {
    let headers: HashMap<&str, &str> = headers.iter().copied().collect();
    let reply = call(
        "http",
        json!({ "method": method, "url": url, "headers": headers, "body": body }),
    )?;
    Ok(Response {
        status: reply["status"].as_u64().unwrap_or(0) as u16,
        body: reply["body"].as_str().unwrap_or_default().to_string(),
    })
}

/// Downloads a file over HTTPS
pub fn download(url: &str, path: &str, executable: bool) -> Result<()> {
    call("download", json!({ "url": url, "path": path, "executable": executable })).map(|_| ())
}

// ---------------------------------------------------------------- programs

/// A program to start, see [`start`]
pub struct Program<'a> {
    /// names the program; shared programs are named once for the whole core
    pub name: &'a str,
    pub path: &'a str,
    pub args: &'a [&'a str],
    pub cwd: Option<&'a str>,
    /// where its output goes
    pub log: Option<&'a str>,
    pub shared: bool,
}

pub fn start(program: Program) -> Result<()> {
    call(
        "process.start",
        json!({
            "name": program.name,
            "path": program.path,
            "args": program.args,
            "cwd": program.cwd,
            "log": program.log,
            "shared": program.shared,
        }),
    )
    .map(|_| ())
}

pub fn stop(name: &str, shared: bool) -> Result<()> {
    call("process.stop", json!({ "name": name, "shared": shared })).map(|_| ())
}

pub fn running(name: &str, shared: bool) -> Result<bool> {
    Ok(call("process.running", json!({ "name": name, "shared": shared }))?["running"]
        .as_bool()
        .unwrap_or(false))
}

// ---------------------------------------------------------------- misc

/// (`linux` | `macos` | `windows`, `x86_64` | `aarch64`)
pub fn platform() -> Result<(String, String)> {
    let reply = call("platform", json!({}))?;
    Ok((
        reply["os"].as_str().unwrap_or_default().to_string(),
        reply["arch"].as_str().unwrap_or_default().to_string(),
    ))
}

/// `bytes` random bytes, hex encoded
pub fn random_hex(bytes: u32) -> Result<String> {
    Ok(call("random", json!({ "bytes": bytes }))?["hex"].as_str().unwrap_or_default().to_string())
}

pub fn log(level: &str, message: &str) {
    let _ = call("log", json!({ "level": level, "message": message }));
}
