//! Who can join through Connect: a private switch (the server's whitelist, enforced) and the
//! players allowed in.
//!
//! The Connect address is public and Minekube lists endpoints in its Browser Hub, so the
//! whitelist is what keeps strangers out. Behind Gate the server runs offline and sees players
//! with offline UUIDs, so the whitelist holds those; with the plugin the server stays online and
//! holds the players' account UUIDs.

use boulder_pebble::{self as host, Server, Value, json};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use crate::{Config, Mode, mode_of};

const WHITELIST_FILE: &str = "whitelist.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AllowedPlayer {
    name: String,
    uuid: String,
}

/// The UUID Minecraft gives a player on an offline mode server, like Java's
/// `UUID.nameUUIDFromBytes("OfflinePlayer:" + name)`
fn offline_uuid(name: &str) -> String {
    let mut hash: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    hash[6] = (hash[6] & 0x0f) | 0x30;
    hash[8] = (hash[8] & 0x3f) | 0x80;
    hyphenated(&hash.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn hyphenated(hex: &str) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn is_valid_name(name: &str) -> bool {
    (3..=16).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The Minecraft account with this name, as (name with its real capitals, account UUID)
fn lookup_account(name: &str) -> host::Result<Option<(String, String)>> {
    #[derive(Deserialize)]
    struct Profile {
        id: String,
        name: String,
    }
    let response = host::http(
        "GET",
        &format!("https://api.mojang.com/users/profiles/minecraft/{name}"),
        &[],
        None,
    )?;
    if response.status == 404 || response.status == 204 {
        return Ok(None);
    }
    if !response.ok() {
        return Err(format!("Failed to look up the player ({})", response.status));
    }
    let profile: Profile = response.json()?;
    if profile.id.len() != 32 {
        return Err("Mojang returned an invalid player UUID".into());
    }
    Ok(Some((profile.name, hyphenated(&profile.id))))
}

fn read_whitelist() -> Vec<AllowedPlayer> {
    host::read_json(WHITELIST_FILE)
}

fn write_whitelist(players: &[AllowedPlayer]) -> host::Result<()> {
    host::write_json(WHITELIST_FILE, &players)
}

fn is_private() -> bool {
    host::properties(&["white-list", "enforce-whitelist"])
        .map(|values| {
            values
                .values()
                .all(|value| value.as_deref() == Some("true"))
        })
        .unwrap_or(false)
}

/// Runs a console command when the server is running, so a change applies right away
fn tell_server(server: &Server, command: &str) {
    if server.state == "Running" {
        let _ = host::console(command);
    }
}

pub fn set_private(server: &Server, private: bool) -> host::Result<()> {
    let value = private.to_string();
    host::set_properties(&[("white-list", value.clone()), ("enforce-whitelist", value)])?;
    // the whitelist applies now; enforce-whitelist (kicking players already online who aren't
    // on it) only when the server next starts
    tell_server(server, if private { "whitelist on" } else { "whitelist off" });
    Ok(())
}

pub fn add_player(server: &Server, config: &Config, name: &str) -> host::Result<()> {
    let name = name.trim();
    if !is_valid_name(name) {
        return Err("Player names are 3 to 16 letters, digits and underscores".into());
    }
    let (name, uuid) = match lookup_account(name)? {
        // behind Gate the server only knows players by their offline UUID
        Some((name, account)) => match mode_of(server) {
            Mode::Gate => (name.clone(), offline_uuid(&name)),
            Mode::Plugin => (name, account),
        },
        None if config.offline_mode => (name.to_string(), offline_uuid(name)),
        None => return Err(format!("There is no Minecraft account called {name}")),
    };
    let mut players = read_whitelist();
    players.retain(|p| !p.name.eq_ignore_ascii_case(&name));
    players.push(AllowedPlayer { name, uuid });
    write_whitelist(&players)?;
    tell_server(server, "whitelist reload");
    Ok(())
}

pub fn remove_player(server: &Server, name: &str) -> host::Result<()> {
    let mut players = read_whitelist();
    players.retain(|p| !p.name.eq_ignore_ascii_case(name));
    write_whitelist(&players)?;
    tell_server(server, "whitelist reload");
    // a removed player who is online leaves when the server is private
    if is_private() {
        tell_server(server, &format!("kick {name} You are no longer on the whitelist"));
    }
    Ok(())
}

/// The "Who can join" section of the page
pub fn section(config: &Config) -> Value {
    let private = is_private();
    let mut players = read_whitelist();
    players.sort_by_key(|p| p.name.to_lowercase());
    let rows: Vec<Value> = players
        .iter()
        .map(|player| {
            json!({
                "key": player.name,
                "title": player.name,
                "subtitle": player.uuid,
                "image": format!("https://mc-heads.net/avatar/{}/24", player.name),
                "removable": true,
            })
        })
        .collect();
    json!({
        "title": "Who can join",
        "description": "Anyone can find a Connect address, and Minekube lists servers in its Browser Hub. Make the server private to let in only the players below.",
        "items": [
            {
                "type": "toggle",
                "id": "private",
                "label": "Private",
                "value": private,
                "description": if !private {
                    "Anyone with the address can join"
                } else if config.offline_mode {
                    "Only players on the list can join, but Offline mode is on, so anyone can join under a listed name"
                } else {
                    "Only players on the list can join, everyone else is turned away"
                },
            },
            {
                "type": "list",
                "id": "players",
                "label": "Allowed players",
                "description": "Minecraft names of the players who may join",
                "add": { "placeholder": "Player name", "button": "Add" },
                "empty": "No players on the list yet",
                "rows": rows,
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuids_match_minecraft() {
        assert_eq!(offline_uuid("Notch"), "b50ad385-829d-3141-a216-7e7d7539ba7f");
    }

    #[test]
    fn player_names() {
        assert!(is_valid_name("Steve_01"));
        assert!(!is_valid_name("ab"));
        assert!(!is_valid_name("has space"));
    }
}
