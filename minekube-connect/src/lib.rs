//! Minekube Connect: a free `<name>.play.minekube.net` address for a server, and who may join it.
//!
//! Paper and Spigot servers get Minekube's Connect plugin. Every other server runs behind
//! Minekube's Gate proxy, which signs players in and passes them to the server in offline mode
//! on this machine; Gate starts and stops with the server.
//!
//! Settings live in `.boulder_connect.json` in the server's folder, Gate's files in
//! `.boulder_gate/`, and the Gate program once for every server in the pebble's data folder.

mod access;

use std::collections::HashMap;

use boulder_pebble::{self as host, Event, Program, Server, Value, json};
use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = ".boulder_connect.json";
const GATE_DIR: &str = ".boulder_gate";
const PLUGIN_JAR: &str = "plugins/connect-spigot.jar";
const PLUGIN_CONFIG: &str = "plugins/connect/config.yml";
const PLUGIN_URL: &str =
    "https://github.com/minekube/connect-java/releases/download/latest/connect-spigot.jar";
const GATE_RELEASE: &str = "https://api.github.com/repos/minekube/gate/releases/latest";
const USER_AGENT: &str = "ayushO4-dev/boulder-pebbles minekube-connect";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Minekube's Connect plugin, on Paper and Spigot
    Plugin,
    /// Minekube's Gate proxy in front of the server, on every other server type
    Gate,
}

fn mode_of(server: &Server) -> Mode {
    match server.variant.as_str() {
        "paper" | "spigot" => Mode::Plugin,
        _ => Mode::Gate,
    }
}

fn default_true() -> bool {
    true
}

/// What is remembered in `.boulder_connect.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Config {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    endpoint: String,
    /// server.properties values before Connect changed them, put back when it is turned off
    #[serde(default)]
    previous_properties: HashMap<String, String>,
    /// Gate connects by itself when the server starts
    #[serde(default = "default_true")]
    auto_connect: bool,
    /// Connect is on but the pebble is switched off, so its changes are taken off the server
    #[serde(default)]
    paused: bool,
    /// players without a paid Minecraft account can join
    #[serde(default)]
    pub(crate) offline_mode: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: String::new(),
            previous_properties: HashMap::new(),
            auto_connect: true,
            paused: false,
            offline_mode: false,
        }
    }
}

pub(crate) fn read_config() -> Config {
    host::read_json(CONFIG_FILE)
}

fn write_config(config: &Config) -> host::Result<()> {
    host::write_json(CONFIG_FILE, config)
}

/// Minekube accepts endpoint names of 4 to 16 characters; they become part of an address, so
/// only lowercase letters, digits and inner hyphens are allowed
fn is_valid_endpoint(name: &str) -> bool {
    (4..=16).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// A valid endpoint name from a server name, "My Survival World!" -> "my-survival-worl"
fn suggest_endpoint(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let mut slug: String = slug.chars().take(16).collect();
    while slug.ends_with('-') {
        slug.pop();
    }
    while slug.len() < 4 {
        slug.push('0');
    }
    slug
}

fn address_of(endpoint: &str) -> String {
    format!("{endpoint}.play.minekube.net")
}

// ---------------------------------------------------------------- server.properties

/// The current values of some server.properties keys, with Minecraft's defaults for missing ones
fn current_properties(defaults: &[(&str, &str)]) -> host::Result<HashMap<String, String>> {
    let keys: Vec<&str> = defaults.iter().map(|(key, _)| *key).collect();
    let values = host::properties(&keys)?;
    Ok(defaults
        .iter()
        .map(|(key, default)| {
            let value = values.get(*key).cloned().flatten();
            (key.to_string(), value.unwrap_or_else(|| default.to_string()))
        })
        .collect())
}

fn set_properties(values: &[(&str, &str)]) -> host::Result<()> {
    let values: Vec<(&str, String)> = values.iter().map(|(k, v)| (*k, v.to_string())).collect();
    host::set_properties(&values)
}

fn restore_properties(previous: &HashMap<String, String>) -> host::Result<()> {
    let values: Vec<(&str, String)> = previous.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    host::set_properties(&values)
}

// ---------------------------------------------------------------- plugin

fn enable_plugin(config: &mut Config) -> host::Result<()> {
    host::download(PLUGIN_URL, PLUGIN_JAR, false)?;
    write_plugin_config(config)?;
    if config.previous_properties.is_empty() {
        config.previous_properties = current_properties(&[("enforce-secure-profile", "true")])?;
    }
    // the plugin can't pass signed chat through
    set_properties(&[("enforce-secure-profile", "false")])
}

/// Sets the endpoint and offline players in the plugin's config, keeping its other settings
fn write_plugin_config(config: &Config) -> host::Result<()> {
    let text = host::read(PLUGIN_CONFIG)?.unwrap_or_default();
    let mut lines: Vec<String> = text
        .lines()
        .filter(|line| {
            !line.starts_with("endpoint:") && !line.starts_with("allow-offline-mode-players:")
        })
        .map(String::from)
        .collect();
    lines.insert(0, format!("endpoint: {}", config.endpoint));
    lines.insert(1, format!("allow-offline-mode-players: {}", config.offline_mode));
    host::write(PLUGIN_CONFIG, &(lines.join("\n") + "\n"))
}

fn disable_plugin(config: &mut Config) -> host::Result<()> {
    host::remove(PLUGIN_JAR)?;
    let previous = std::mem::take(&mut config.previous_properties);
    restore_properties(&previous)
}

// ---------------------------------------------------------------- gate

fn gate_path() -> host::Result<String> {
    let (os, _) = host::platform()?;
    Ok(if os == "windows" { "pebble://bin/gate.exe" } else { "pebble://bin/gate" }.to_string())
}

/// The Gate program, downloaded from Minekube's releases the first time
fn gate_binary() -> host::Result<String> {
    let path = gate_path()?;
    if host::exists(&path)? {
        return Ok(path);
    }
    let (os, arch) = host::platform()?;
    let os = match os.as_str() {
        "macos" => "darwin",
        other => other,
    };
    let arch = match arch.as_str() {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => return Err(format!("Gate does not run on {other}")),
    };
    let release: Value = host::http(
        "GET",
        GATE_RELEASE,
        &[("User-Agent", USER_AGENT), ("Accept", "application/vnd.github+json")],
        None,
    )?
    .json()?;
    let suffix = format!("_{os}_{arch}{}", if os == "windows" { ".exe" } else { "" });
    let url = release["assets"]
        .as_array()
        .and_then(|assets| {
            assets.iter().find(|asset| {
                asset["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("gate_") && name.ends_with(&suffix))
            })
        })
        .and_then(|asset| asset["browser_download_url"].as_str())
        .ok_or_else(|| format!("No Gate download for {os} {arch}"))?
        .to_string();
    host::download(&url, &path, true)?;
    Ok(path)
}

/// Writes Gate's config: it signs players in, then passes them to the server on this machine
fn write_gate_config(server: &Server, config: &Config) -> host::Result<()> {
    let motd = serde_json::to_string(&server.name).unwrap_or_default();
    // Gate only listens on this machine, on a port the system picks; players reach it
    // through Connect
    let text = format!(
        "# Generated by Boulder for Minekube Connect, changes are overwritten
config:
  bind: 127.0.0.1:0
  onlineMode: true
  forceKeyAuthentication: false
  servers:
    server: 127.0.0.1:{port}
  try:
    - server
  forwarding:
    mode: none
  status:
    motd: {motd}
connect:
  enabled: true
  name: {endpoint}
  allowOfflineModePlayers: {offline}
",
        port = server.port,
        endpoint = config.endpoint,
        offline = config.offline_mode,
    );
    host::write(&format!("{GATE_DIR}/config.yml"), &text)
}

fn start_gate(server: &Server, config: &Config) -> host::Result<()> {
    write_gate_config(server, config)?;
    let gate = gate_binary()?;
    // Gate keeps its Connect token in connect.json next to its config
    host::start(Program {
        name: "gate",
        path: &gate,
        args: &["--config", "config.yml"],
        cwd: Some(GATE_DIR),
        log: Some(&format!("{GATE_DIR}/gate.log")),
        shared: false,
    })?;
    host::log("info", &format!("Started Minekube Connect for {} as {}", server.name, config.endpoint));
    Ok(())
}

fn stop_gate() -> host::Result<()> {
    host::stop("gate", false)
}

fn gate_running() -> bool {
    host::running("gate", false).unwrap_or(false)
}

fn enable_gate(config: &mut Config) -> host::Result<()> {
    if config.previous_properties.is_empty() {
        config.previous_properties = current_properties(&[
            ("online-mode", "true"),
            ("server-ip", ""),
            ("enforce-secure-profile", "true"),
        ])?;
    }
    // Gate signs players in, so the server runs in offline mode and only listens on this
    // machine, where nobody but Gate can reach it
    set_properties(&[
        ("online-mode", "false"),
        ("server-ip", "127.0.0.1"),
        ("enforce-secure-profile", "false"),
    ])
    // Gate follows the server: it starts with the server
}

fn disable_gate(config: &mut Config) -> host::Result<()> {
    stop_gate()?;
    let previous = std::mem::take(&mut config.previous_properties);
    restore_properties(&previous)
}

/// Whether Gate should run while the server does
fn gate_wanted(config: &Config) -> bool {
    config.enabled && !config.paused
}

// ---------------------------------------------------------------- turning on and off

/// Takes Connect off the server, putting back the settings it changed
fn take_off(server: &Server, config: &mut Config) -> host::Result<()> {
    if mode_of(server) == Mode::Gate && server.state != "Stopped" {
        return Err(
            "Stop the server first, Minekube Connect has to put its network settings back".into(),
        );
    }
    match mode_of(server) {
        Mode::Plugin => disable_plugin(config),
        Mode::Gate => disable_gate(config),
    }
}

/// Turns Connect off, putting back what it changed; when the pebble is removed
fn turn_off(server: &Server) -> host::Result<()> {
    let mut config = read_config();
    if config.enabled && !config.paused {
        take_off(server, &mut config)?;
    }
    config.enabled = false;
    config.paused = false;
    write_config(&config)
}

/// Takes Connect off while the pebble is switched off, remembering that it was on
fn pause(server: &Server) -> host::Result<()> {
    let mut config = read_config();
    if !config.enabled || config.paused {
        return Ok(());
    }
    take_off(server, &mut config)?;
    config.paused = true;
    write_config(&config)
}

/// Puts Connect back when the pebble is switched on again
fn resume(server: &Server) -> host::Result<()> {
    let mut config = read_config();
    if !config.enabled || !config.paused {
        return Ok(());
    }
    match mode_of(server) {
        Mode::Plugin => enable_plugin(&mut config)?,
        Mode::Gate => {
            if server.state != "Stopped" {
                return Err(
                    "Stop the server first, Minekube Connect has to change its network settings"
                        .into(),
                );
            }
            enable_gate(&mut config)?
        }
    }
    config.paused = false;
    write_config(&config)
}

/// Turns Connect on or off, or changes its name
fn set_connect(server: &Server, enabled: bool, endpoint: &str) -> host::Result<()> {
    let endpoint = endpoint.trim().to_lowercase();
    if !is_valid_endpoint(&endpoint) {
        return Err(
            "Use 4 to 16 lowercase letters, digits and hyphens, not starting or ending with a hyphen"
                .into(),
        );
    }
    let mode = mode_of(server);
    let mut config = read_config();
    let turning_on = enabled && !config.enabled;
    let turning_off = !enabled && config.enabled;
    // Gate changes how the server listens, which only takes effect when it starts
    if mode == Mode::Gate && (turning_on || turning_off) && server.state != "Stopped" {
        return Err("Stop the server before turning Connect on or off".into());
    }
    let endpoint_changed = config.endpoint != endpoint;
    config.endpoint = endpoint;
    match mode {
        Mode::Plugin if turning_on => enable_plugin(&mut config)?,
        Mode::Plugin if enabled && endpoint_changed => write_plugin_config(&config)?,
        Mode::Plugin if turning_off => disable_plugin(&mut config)?,
        Mode::Gate if turning_on => enable_gate(&mut config)?,
        Mode::Gate if enabled && endpoint_changed && gate_running() => start_gate(server, &config)?,
        Mode::Gate if turning_off => disable_gate(&mut config)?,
        _ => {}
    }
    config.enabled = enabled;
    write_config(&config)
}

fn set_offline_mode(server: &Server, offline: bool) -> host::Result<()> {
    let mut config = read_config();
    if config.offline_mode == offline {
        return Ok(());
    }
    config.offline_mode = offline;
    if config.enabled && !config.paused {
        match mode_of(server) {
            Mode::Plugin => write_plugin_config(&config)?,
            Mode::Gate if gate_running() => start_gate(server, &config)?,
            Mode::Gate => {}
        }
    }
    write_config(&config)
}

/// Connects or disconnects Gate by hand while the server runs
fn set_session(server: &Server, connected: bool) -> host::Result<()> {
    if mode_of(server) != Mode::Gate {
        return Err("The Connect plugin connects whenever the server runs".into());
    }
    if connected {
        let config = read_config();
        if !gate_wanted(&config) {
            return Err("Turn Connect on first".into());
        }
        if server.state != "Running" {
            return Err("Start the server first".into());
        }
        if !gate_running() {
            start_gate(server, &config)?;
        }
        Ok(())
    } else {
        stop_gate()
    }
}

// ---------------------------------------------------------------- the page

fn view(server: &Server) -> Value {
    let config = read_config();
    let mode = mode_of(server);
    let gate = mode == Mode::Gate;
    let endpoint = if config.endpoint.is_empty() {
        suggest_endpoint(&server.name)
    } else {
        config.endpoint.clone()
    };
    let server_running = server.state == "Running";
    let on = config.enabled && !config.paused;
    let connected = on
        && match mode {
            Mode::Plugin => server_running && host::exists(PLUGIN_JAR).unwrap_or(false),
            Mode::Gate => gate_running(),
        };
    let switch_blocked = gate && server.state != "Stopped";
    let restart_note = if gate { "" } else { ". Restart the server to apply" };

    let status = if !config.enabled {
        json!({ "text": "Off", "color": "gray" })
    } else if connected {
        json!({ "text": "Connected", "color": "green" })
    } else if server_running {
        json!({ "text": "Not connected", "color": "yellow" })
    } else {
        json!({ "text": "Waiting for server", "color": "gray" })
    };

    let mut items = vec![
        json!({
            "type": "toggle",
            "id": "connect",
            "label": "Connect",
            "value": config.enabled,
            "disabled": switch_blocked,
            "description": if switch_blocked {
                "Stop the server to turn Connect on or off"
            } else if gate {
                "Runs Minekube's Gate proxy in front of this server, which then only accepts players through Connect"
            } else {
                "Installs Minekube's Connect plugin, which connects whenever the server runs. Restart the server to apply"
            },
            "confirm": if gate { json!({
                "title": "Turn on Minekube Connect?",
                "body": "This server runs behind Minekube's Gate proxy, which signs players in. The server switches to offline mode and only accepts players through Connect, so the old address stops working. Players on an existing world get new player data, so their inventories and progress start over. Turning Connect off puts the settings back.",
                "button": "Turn on Connect",
            }) } else { Value::Null },
        }),
        json!({
            "type": "text",
            "id": "endpoint",
            "label": "Name",
            "value": endpoint,
            "placeholder": "my-server",
            "pattern": "^[a-z0-9](?:[a-z0-9-]{2,14})[a-z0-9]$",
            "pattern_error": "Use 4 to 16 lowercase letters, digits and hyphens, not starting or ending with a hyphen",
            "description": "4 to 16 lowercase letters, digits and hyphens, unique across Minekube",
        }),
    ];
    if gate {
        items.push(json!({
            "type": "toggle",
            "id": "auto_connect",
            "label": "Auto connect on server start",
            "value": config.auto_connect,
            "disabled": !config.enabled,
            "description": if config.enabled {
                "Gate connects to Minekube when the server starts. When off, connect by hand below"
            } else {
                "Turn Connect on first"
            },
        }));
    }
    items.push(json!({
        "type": "toggle",
        "id": "offline_mode",
        "label": "Offline mode",
        "value": config.offline_mode,
        "description": format!(
            "{}{}",
            if config.offline_mode {
                "Players without a paid Minecraft account can join too, with any name they like, so make the server private"
            } else {
                "Only players with a paid Minecraft account can join"
            },
            if gate { ". Changes reach Minekube within a minute" } else { restart_note },
        ),
    }));
    if gate && config.enabled {
        items.push(json!({
            "type": "button",
            "id": "session",
            "label": "Connection",
            "description": if server_running {
                "Connect or disconnect while the server runs"
            } else {
                "Start the server to connect"
            },
            "button": if connected { "Disconnect" } else { "Connect now" },
            "style": if connected { "danger" } else { "primary" },
            "disabled": !server_running,
        }));
    }
    items.push(json!({
        "type": "copy",
        "label": "Address",
        "description": if config.enabled { "Players join here" } else { "Players will join here once Connect is on" },
        "value": address_of(&endpoint),
        "status": status,
    }));

    json!({
        "sections": [
            {
                "title": "Minekube Connect",
                "description": "A free address for this server that players can join without port forwarding.",
                "items": items,
            },
            access::section(&config),
        ],
        // the connection follows the server and Gate, keep it current
        "refresh_seconds": 10,
    })
}

fn action(server: &Server, event: &Event) -> host::Result<Option<String>> {
    let value = event.get("value");
    let config = read_config();
    let endpoint = if config.endpoint.is_empty() {
        suggest_endpoint(&server.name)
    } else {
        config.endpoint.clone()
    };
    match event.str("action") {
        "connect" => set_connect(server, value.as_bool().unwrap_or(false), &endpoint)?,
        "endpoint" => set_connect(server, config.enabled, value.as_str().unwrap_or_default())?,
        "auto_connect" => {
            let mut config = config;
            config.auto_connect = value.as_bool().unwrap_or(true);
            write_config(&config)?;
        }
        "offline_mode" => set_offline_mode(server, value.as_bool().unwrap_or(false))?,
        "session" => set_session(server, !gate_running())?,
        "private" => access::set_private(server, value.as_bool().unwrap_or(false))?,
        "players" => {
            if let Some(name) = value.get("add").and_then(Value::as_str) {
                access::add_player(server, &config, name)?;
            } else if let Some(name) = value.get("remove").and_then(Value::as_str) {
                access::remove_player(server, name)?;
            }
        }
        other => return Err(format!("Unknown action {other}")),
    }
    Ok(None)
}

// ---------------------------------------------------------------- events

fn on_server_state(server: &Server, state: &str) -> host::Result<()> {
    if mode_of(server) != Mode::Gate {
        return Ok(());
    }
    match state {
        "Running" => {
            let config = read_config();
            if gate_wanted(&config) && config.auto_connect && !gate_running() {
                start_gate(server, &config)?;
            }
            Ok(())
        }
        "Stopped" | "Error" => stop_gate(),
        _ => Ok(()),
    }
}

fn handle(event: Event) -> host::Result<Value> {
    let server = host::server()?;
    match event.name.as_str() {
        "install" | "core_start" => Ok(json!({})),
        "uninstall" => turn_off(&server).map(|_| json!({})),
        "enable" => resume(&server).map(|_| json!({})),
        "disable" => pause(&server).map(|_| json!({})),
        "server_state" => on_server_state(&server, event.str("state")).map(|_| json!({})),
        "address" => {
            let config = read_config();
            let address = (config.enabled && !config.paused && !config.endpoint.is_empty())
                .then(|| address_of(&config.endpoint));
            Ok(json!({ "address": address }))
        }
        "view" => Ok(json!({ "view": view(&server) })),
        "action" => {
            let message = action(&server, &event)?;
            // the server may have changed state meanwhile
            let server = host::server()?;
            Ok(json!({ "view": view(&server), "message": message }))
        }
        other => Err(format!("Unknown event {other}")),
    }
}

host::pebble!(handle);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_names() {
        assert!(is_valid_endpoint("my-server"));
        assert!(!is_valid_endpoint("abc"));
        assert!(!is_valid_endpoint("-abcd"));
        assert!(!is_valid_endpoint("abcd-"));
        assert!(!is_valid_endpoint("Abcd"));
        assert!(!is_valid_endpoint("abcdefghijklmnopq"));
    }

    #[test]
    fn suggested_names_are_valid() {
        for name in ["My Survival World!", "a", "Fabric Test", "!!!", "x-y"] {
            assert!(is_valid_endpoint(&suggest_endpoint(name)), "{name}");
        }
    }
}
