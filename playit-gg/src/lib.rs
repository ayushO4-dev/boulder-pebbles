//! Playit.gg: tunnels from playit.gg to the server.
//!
//! One Playit agent runs for the whole Boulder core and serves every server with this pebble.
//! Signing in links the agent to a Playit account through Playit's claim flow; the account key
//! is kept in the pebble's data folder. Tunnels are created on playit.gg, the pebble lists them
//! and shows the one that points at the server's port.

use boulder_pebble::{self as host, Event, Program, Server, Value, json};
use serde::{Deserialize, Serialize};

const API: &str = "https://api.playit.gg";
const SECRET: &str = "pebble://secret.txt";
const CLAIM: &str = "pebble://claim.json";
const AGENT: &str = "agent";
const RELEASE: &str = "https://api.github.com/repos/playit-cloud/playit-agent/releases/latest";
const USER_AGENT: &str = "ayushO4-dev/boulder-pebbles playit-gg";
const TUNNELS_PAGE: &str = "https://playit.gg/account/tunnels";

// ---------------------------------------------------------------- the Playit API

/// Calls the Playit API, which answers `{"status": "success" | "fail" | "error", "data": …}`
fn api(path: &str, body: Value, secret: Option<&str>) -> host::Result<Value> {
    let auth = secret.map(|secret| format!("Agent-Key {}", secret.trim()));
    let mut headers = vec![("Content-Type", "application/json")];
    if let Some(auth) = &auth {
        headers.push(("Authorization", auth));
    }
    let response = host::http("POST", &format!("{API}{path}"), &headers, Some(&body.to_string()))?;
    let reply: Value = response
        .json()
        .map_err(|_| format!("Playit answered {} for {path}", response.status))?;
    match reply["status"].as_str() {
        Some("success") => Ok(reply["data"].clone()),
        _ => Err(format!("Playit refused {path}: {}", reply["data"])),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Claim {
    code: String,
}

fn secret() -> Option<String> {
    host::read(SECRET)
        .ok()
        .flatten()
        .map(|secret| secret.trim().to_string())
        .filter(|secret| !secret.is_empty())
}

/// Checks a pending sign in; once the user accepted it on playit.gg, swaps it for the key
fn check_claim() -> host::Result<Option<String>> {
    let Some(claim) = host::read(CLAIM)?.and_then(|text| serde_json::from_str::<Claim>(&text).ok())
    else {
        return Ok(None);
    };
    let state = api(
        "/claim/setup",
        json!({ "code": claim.code, "agent_type": "assignable", "version": "boulder-pebble 1.0.0" }),
        None,
    )?;
    match state.as_str() {
        Some("UserAccepted") => {
            let key = api("/claim/exchange", json!({ "code": claim.code }), None)?;
            let key = key["secret_key"]
                .as_str()
                .ok_or("Playit didn't send a key")?
                .to_string();
            host::write(SECRET, &key)?;
            host::remove(CLAIM)?;
            start_agent()?;
            Ok(Some("Signed in to Playit".into()))
        }
        Some("UserRejected") => {
            host::remove(CLAIM)?;
            Err("The sign in was declined on playit.gg".into())
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Clone)]
struct Tunnel {
    name: String,
    address: String,
    local_port: u32,
    active: bool,
}

/// The tunnels of the account that are set up and served by an agent
fn tunnels(secret: &str) -> host::Result<Vec<Tunnel>> {
    let data = api("/tunnels/list", json!({ "tunnel_id": null, "agent_id": null }), Some(secret))?;
    Ok(data["tunnels"]
        .as_array()
        .map(|tunnels| {
            tunnels
                .iter()
                .filter_map(|tunnel| {
                    // pending or disabled tunnels have no allocation, ones without an agent
                    // have no local origin
                    let alloc = &tunnel["alloc"]["data"];
                    let origin = &tunnel["origin"]["data"];
                    let domain = alloc["assigned_domain"].as_str()?;
                    let port = alloc["port_start"].as_u64()?;
                    origin.as_object()?;
                    Some(Tunnel {
                        name: tunnel["name"].as_str().unwrap_or_default().to_string(),
                        address: format!("{domain}:{port}"),
                        // Playit uses the public port when no local port is set
                        local_port: origin["local_port"].as_u64().unwrap_or(port) as u32,
                        active: tunnel["active"].as_bool().unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

// ---------------------------------------------------------------- the agent

fn agent_path() -> host::Result<String> {
    let (os, _) = host::platform()?;
    Ok(if os == "windows" { "pebble://bin/playit.exe" } else { "pebble://bin/playit" }.into())
}

/// The agent program from Playit's releases, downloaded the first time
fn agent_binary() -> host::Result<String> {
    let path = agent_path()?;
    if host::exists(&path)? {
        return Ok(path);
    }
    let (os, arch) = host::platform()?;
    let asset = match (os.as_str(), arch.as_str()) {
        ("linux", "x86_64") => "playit-linux-amd64",
        ("linux", "aarch64") => "playit-linux-aarch64",
        ("windows", "x86_64") => "playit-windows-x86_64-signed.exe",
        _ => return Err(format!("Playit's agent isn't available for {os} {arch}")),
    };
    let release: Value = host::http(
        "GET",
        RELEASE,
        &[("User-Agent", USER_AGENT), ("Accept", "application/vnd.github+json")],
        None,
    )?
    .json()?;
    let url = release["assets"]
        .as_array()
        .and_then(|assets| assets.iter().find(|a| a["name"].as_str() == Some(asset)))
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| format!("Playit's release has no {asset}"))?
        .to_string();
    host::download(&url, &path, true)?;
    Ok(path)
}

/// Starts the one agent of the core, when signed in
fn start_agent() -> host::Result<()> {
    if secret().is_none() || host::running(AGENT, true)? {
        return Ok(());
    }
    let agent = agent_binary()?;
    let (os, _) = host::platform()?;
    let mut args = vec!["--secret-path", SECRET, "--log-path", "pebble://agent.log"];
    // the control socket stays in the pebble's folder; Windows uses a named pipe instead
    if os != "windows" {
        args.extend(["--socket-path", "pebble://playitd.sock"]);
    }
    host::start(Program {
        name: AGENT,
        path: &agent,
        args: &args,
        cwd: Some("pebble://"),
        log: Some("pebble://agent-output.log"),
        shared: true,
    })
}

/// Stops the agent when no other server has the pebble on
fn stop_agent_if_unused() -> host::Result<()> {
    let me = host::server()?.uuid;
    if host::servers()?.iter().all(|server| server.uuid == me) {
        host::stop(AGENT, true)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- the page

fn view(server: &Server, message: Option<&str>) -> Value {
    let mut sections = vec![];
    let mut items = vec![];
    let pending = host::exists(CLAIM).unwrap_or(false);
    let mut refresh = None;

    if let Some(message) = message {
        items.push(json!({ "type": "info", "text": message, "color": "green" }));
    }

    match secret() {
        None if pending => {
            let claim: Option<Claim> = host::read(CLAIM).ok().flatten().and_then(|t| serde_json::from_str(&t).ok());
            items.push(json!({
                "type": "link",
                "label": "Sign in to Playit",
                "description": "Open playit.gg, sign in and accept the agent. This page notices when you're done",
                "url": claim.map(|c| format!("https://playit.gg/claim/{}", c.code)).unwrap_or_default(),
                "button": "Open playit.gg",
            }));
            items.push(json!({
                "type": "button",
                "id": "cancel_signin",
                "label": "Waiting for playit.gg",
                "description": "Started a sign in by mistake?",
                "button": "Cancel",
                "style": "default",
            }));
            refresh = Some(3);
        }
        None => {
            items.push(json!({
                "type": "button",
                "id": "signin",
                "label": "Playit account",
                "description": "Link a Playit account to forward its tunnels to this machine",
                "button": "Sign in",
                "style": "primary",
            }));
        }
        Some(secret) => {
            let running = host::running(AGENT, true).unwrap_or(false);
            items.push(json!({
                "type": "button",
                "id": "agent",
                "label": "Agent",
                "description": if running {
                    "The Playit agent is running and serves every server with this pebble"
                } else {
                    "The Playit agent isn't running"
                },
                "button": if running { "Restart" } else { "Start" },
                "style": if running { "default" } else { "primary" },
            }));
            match tunnels(&secret) {
                Ok(tunnels) => {
                    let mine = tunnels
                        .iter()
                        .find(|t| t.active && t.local_port == server.port);
                    match mine {
                        Some(tunnel) => items.push(json!({
                            "type": "copy",
                            "label": "Address",
                            "description": format!("Players join here, through the tunnel \"{}\"", tunnel.name),
                            "value": tunnel.address,
                            "status": if running {
                                json!({ "text": "Online", "color": "green" })
                            } else {
                                json!({ "text": "Agent stopped", "color": "yellow" })
                            },
                        })),
                        None => items.push(json!({
                            "type": "link",
                            "label": "No tunnel for this server",
                            "description": format!(
                                "Create a Minecraft Java tunnel on playit.gg with local port {}",
                                server.port
                            ),
                            "url": TUNNELS_PAGE,
                            "button": "Create a tunnel",
                        })),
                    }
                    let rows: Vec<Value> = tunnels
                        .iter()
                        .map(|t| {
                            json!({
                                "key": t.address,
                                "title": t.name,
                                "subtitle": format!(
                                    "{} → port {}{}",
                                    t.address,
                                    t.local_port,
                                    if t.active { "" } else { " (off)" }
                                ),
                            })
                        })
                        .collect();
                    sections.push(json!({
                        "title": "Tunnels",
                        "description": "The tunnels of your Playit account. Create and change them on playit.gg.",
                        "items": [
                            { "type": "list", "id": "tunnels", "label": "Your tunnels", "description": "Every server's tunnels", "rows": rows, "empty": "No tunnels yet" },
                            { "type": "link", "label": "Manage tunnels", "description": "Create, change or remove tunnels", "url": TUNNELS_PAGE, "button": "Open playit.gg" },
                        ],
                    }));
                }
                Err(error) => items.push(json!({ "type": "info", "text": error, "color": "red" })),
            }
            items.push(json!({
                "type": "button",
                "id": "signout",
                "label": "Sign out",
                "description": "Unlinks the Playit account from Boulder and stops the agent for every server",
                "button": "Sign out",
                "style": "danger",
            }));
            refresh = Some(15);
        }
    }

    sections.insert(
        0,
        json!({
            "title": "Playit.gg",
            "description": "Tunnels from playit.gg let players reach this server without port forwarding.",
            "items": items,
        }),
    );
    json!({ "sections": sections, "refresh_seconds": refresh })
}

fn action(event: &Event) -> host::Result<Option<String>> {
    match event.str("action") {
        "signin" => {
            let code = host::random_hex(5)?;
            host::write_json(CLAIM, &Claim { code })?;
            Ok(None)
        }
        "cancel_signin" => host::remove(CLAIM).map(|_| None),
        "agent" => {
            host::stop(AGENT, true)?;
            start_agent()?;
            Ok(Some("Started the Playit agent".into()))
        }
        "signout" => {
            host::stop(AGENT, true)?;
            host::remove(SECRET)?;
            host::remove(CLAIM)?;
            Ok(Some("Signed out of Playit".into()))
        }
        "tunnels" => Ok(None),
        other => Err(format!("Unknown action {other}")),
    }
}

fn handle(event: Event) -> host::Result<Value> {
    match event.name.as_str() {
        "install" | "enable" | "core_start" => start_agent().map(|_| json!({})),
        "uninstall" | "disable" => stop_agent_if_unused().map(|_| json!({})),
        "server_state" => Ok(json!({})),
        "address" => {
            let server = host::server()?;
            let address = secret().and_then(|secret| {
                tunnels(&secret)
                    .ok()?
                    .into_iter()
                    .find(|t| t.active && t.local_port == server.port)
                    .map(|t| t.address)
            });
            Ok(json!({ "address": address }))
        }
        "view" => {
            // a pending sign in is checked whenever the page refreshes
            let message = check_claim()?;
            Ok(json!({ "view": view(&host::server()?, message.as_deref()) }))
        }
        "action" => {
            let message = action(&event)?;
            Ok(json!({ "view": view(&host::server()?, message.as_deref()) }))
        }
        other => Err(format!("Unknown event {other}")),
    }
}

host::pebble!(handle);
