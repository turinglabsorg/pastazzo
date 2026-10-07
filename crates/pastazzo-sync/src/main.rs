//! pastazzo-sync: end-to-end encrypted clipboard sync.
//!
//! ```text
//! pastazzo-sync join <invite link> --username <name> [--name <device name>]
//! pastazzo-sync login --server <url> --fingerprint <fp> --username <name> [--name <device name>]
//! pastazzo-sync run
//! pastazzo-sync send <text>
//! pastazzo-sync status [--json]
//! pastazzo-sync devices
//! pastazzo-sync revoke <device id>
//! pastazzo-sync clear [--everywhere]
//! pastazzo-sync logout
//! ```
//!
//! Passwords are read from the terminal, or from `--password-file` for
//! scripted setups.

use std::process::Command;

use pastazzo_core::api::B64;
use pastazzo_core::item::Content;
use pastazzo_core::{Id, display_fingerprint};
use pastazzo_sync::remote::Remote;
use pastazzo_sync::state::State;
use pastazzo_sync::{Result, account, clipboard, daemon};
use zeroize::Zeroizing;

const USAGE: &str = "usage:
  pastazzo-sync join <invite link> --username <name> [--name <device name>]
  pastazzo-sync login --server <url> --fingerprint <fp> --username <name> [--name <device name>]
  pastazzo-sync run
  pastazzo-sync send <text>
  pastazzo-sync status [--json]
  pastazzo-sync devices
  pastazzo-sync revoke <device id>
  pastazzo-sync clear [--everywhere]
  pastazzo-sync logout

join and login read the password from the terminal, or from --password-file <file>";

/// Options that take no value.
const FLAGS: &[&str] = &["--json", "--everywhere"];

/// Passwords for new accounts must be at least this long.
const MIN_PASSWORD_CHARS: usize = 12;

fn main() {
    if let Err(error) = run() {
        eprintln!("pastazzo-sync: {error}");
        std::process::exit(1);
    }
}

struct Args {
    command: String,
    positional: Vec<String>,
    options: Vec<(String, String)>,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut args = std::env::args().skip(1);
        let command = args.next().ok_or(USAGE)?;
        let (mut positional, mut options) = (Vec::new(), Vec::new());
        while let Some(arg) = args.next() {
            if FLAGS.contains(&arg.as_str()) {
                options.push((arg.trim_start_matches('-').to_owned(), String::new()));
            } else if let Some(name) = arg.strip_prefix("--") {
                let value = args.next().ok_or(format!("--{name} needs a value"))?;
                options.push((name.to_owned(), value));
            } else {
                positional.push(arg);
            }
        }
        Ok(Self {
            command,
            positional,
            options,
        })
    }

    fn option(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn required(&self, name: &str) -> Result<&str> {
        self.option(name)
            .ok_or(format!("--{name} is required\n{USAGE}"))
    }

    fn device_name(&self) -> String {
        self.option("name")
            .map(str::to_owned)
            .unwrap_or_else(default_device_name)
    }

    fn password(&self, confirm: bool) -> Result<Zeroizing<String>> {
        if let Some(path) = self.option("password-file") {
            let text = Zeroizing::new(
                std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?,
            );
            return Ok(Zeroizing::new(
                text.trim_end_matches(['\r', '\n']).to_owned(),
            ));
        }
        let password =
            Zeroizing::new(rpassword::prompt_password("Password: ").map_err(|e| e.to_string())?);
        if confirm {
            let again = Zeroizing::new(
                rpassword::prompt_password("Password again: ").map_err(|e| e.to_string())?,
            );
            if *again != *password {
                return Err("the passwords don't match".to_owned());
            }
        }
        Ok(password)
    }
}

fn default_device_name() -> String {
    let name = if cfg!(target_os = "macos") {
        Command::new("scutil")
            .args(["--get", "ComputerName"])
            .output()
            .ok()
    } else {
        Command::new("hostname").output().ok()
    };
    name.and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "device".to_owned())
}

fn run() -> Result<()> {
    let args = Args::parse()?;
    let path = State::default_path()?;
    match args.command.as_str() {
        "join" => {
            if path.exists() {
                return Err(format!(
                    "already logged in ({}): run `pastazzo-sync logout` first",
                    path.display()
                ));
            }
            let link = args.positional.first().ok_or(USAGE)?;
            let username = args.required("username")?;
            let password = args.password(true)?;
            if password.chars().count() < MIN_PASSWORD_CHARS {
                return Err(format!(
                    "use a password of at least {MIN_PASSWORD_CHARS} characters: it's the only thing protecting your account from the server"
                ));
            }
            let mut state = account::join(link, username, &password, &args.device_name())?;
            state.save_new(&path)?;
            println!("account {username} created, this device is logged in");
            print_login_command(&state);
            Ok(())
        }
        "login" => {
            if path.exists() {
                return Err(format!(
                    "already logged in ({}): run `pastazzo-sync logout` first",
                    path.display()
                ));
            }
            let fingerprint = B64::decode(args.required("fingerprint")?)
                .and_then(|f| f.try_into().ok())
                .ok_or("--fingerprint isn't a valid fingerprint")?;
            let password = args.password(false)?;
            let mut state = account::login(
                args.required("server")?,
                &fingerprint,
                args.required("username")?,
                &password,
                &args.device_name(),
            )?;
            state.save_new(&path)?;
            println!("logged in as {} on {}", state.username, state.server_url);
            Ok(())
        }
        "run" => daemon::Daemon::new(State::load(&path)?, &path, clipboard::platform()?)
            .with_status_dir(clipboard::data_dir()?.join("sync"))?
            .run(),
        "send" => {
            let text = args.positional.join(" ");
            if text.is_empty() {
                return Err(USAGE.to_owned());
            }
            let state = State::load(&path)?;
            daemon::Daemon::new(state, &path, Box::new(NoClipboard)).send(&Content::Text(text))?;
            println!("sent");
            Ok(())
        }
        "status" => {
            if args.option("json").is_some() {
                println!("{}", status_json(&path));
                return Ok(());
            }
            let state = State::load(&path)?;
            println!("server:      {}", state.server_url);
            println!("username:    {}", state.username);
            println!(
                "device:      {} ({})",
                state.device_name,
                B64::encode(&state.device.id())
            );
            println!();
            println!(
                "account key: {}  (the same on all your devices)",
                display_fingerprint(&state.account_key.fingerprint())
            );
            println!(
                "server:      {}",
                display_fingerprint(&state.identity.fingerprint())
            );
            println!(
                "this device: {}",
                display_fingerprint(&state.device.public().fingerprint())
            );
            match state.storage {
                pastazzo_sync::state::KeyStorage::Keychain => {
                    println!("keys:        in the system keychain")
                }
                pastazzo_sync::state::KeyStorage::File => {
                    println!("keys:        in {} (no keychain)", path.display())
                }
            }
            print_login_command(&state);
            Ok(())
        }
        "devices" => {
            let state = State::load(&path)?;
            for (name, this) in account::device_names(&state)? {
                println!("{name}{}", if this { "  (this device)" } else { "" });
            }
            Ok(())
        }
        "revoke" => {
            let id: Id = args
                .positional
                .first()
                .and_then(|id| B64::decode(id))
                .and_then(|id| id.try_into().ok())
                .ok_or("revoke needs a device id, as `pastazzo-sync status --json` shows it")?;
            let state = State::load(&path)?;
            Remote::new(&state.server_url).revoke_device(&state, &id)?;
            if id == state.device.id() {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("remove {}: {e}", path.display()))?;
                println!("this device was removed and logged out");
            } else {
                println!("device removed: it can't sync with this account any more");
            }
            Ok(())
        }
        "clear" => {
            clipboard::platform()?.clear_history()?;
            if args.option("everywhere").is_none() {
                println!("history cleared on this device");
                return Ok(());
            }
            let state = State::load(&path)?;
            Remote::new(&state.server_url).delete_items(&state)?;
            // The other devices clear theirs when they get this.
            daemon::Daemon::new(state, &path, Box::new(NoClipboard))
                .send(&Content::ClearHistory)?;
            println!(
                "history cleared here and on the server; the other devices clear theirs as they sync"
            );
            Ok(())
        }
        "logout" => {
            let state = State::load(&path)?;
            let revoked = Remote::new(&state.server_url).revoke_device(&state, &state.device.id());
            state.forget(&path)?;
            match revoked {
                Ok(()) => println!("logged out, and this device was revoked on the server"),
                Err(error) => {
                    println!("logged out locally, but revoking the device failed: {error}")
                }
            }
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other}\n{USAGE}")),
    }
}

/// How to add another device: the server and its fingerprint, from a device
/// already logged in.
fn print_login_command(state: &State) {
    println!();
    println!("to add another device, run there:");
    println!(
        "  pastazzo-sync login --server {} --fingerprint {} --username {}",
        state.server_url,
        B64::encode(&state.identity.fingerprint()),
        state.username
    );
}

/// For one-off sends: no local clipboard involved.
struct NoClipboard;

impl clipboard::Clipboard for NoClipboard {
    fn poll(&mut self) -> Vec<Content> {
        Vec::new()
    }
    fn apply(&mut self, _: &Content, _: &str) -> Result<()> {
        Ok(())
    }
    fn remember(&mut self, _: &Content, _: &str) -> Result<()> {
        Ok(())
    }
    fn clear_history(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Everything the settings screens show, as JSON. Never fails: problems are
/// reported inside, so the UIs always get something to render.
fn status_json(path: &std::path::Path) -> String {
    use serde_json::json;
    let state = match State::load(path) {
        Ok(state) => state,
        Err(error) => return json!({ "logged_in": false, "error": error }).to_string(),
    };
    let (devices, devices_error) = match account::devices(&state) {
        Ok(devices) => (devices, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    json!({
        "logged_in": true,
        "server_url": state.server_url,
        "username": state.username,
        "server_fingerprint": display_fingerprint(&state.identity.fingerprint()),
        "account_key_fingerprint": display_fingerprint(&state.account_key.fingerprint()),
        "key_epoch": state.account_key.epoch(),
        "key_storage": state.storage,
        "this_device": {
            "id": B64::encode(&state.device.id()),
            "name": state.device_name,
            "fingerprint": display_fingerprint(&state.device.public().fingerprint()),
        },
        "devices": devices.iter().map(|device| json!({
            "id": B64::encode(&device.id),
            "name": device.name,
            "fingerprint": display_fingerprint(&device.fingerprint),
            "this": device.this,
        })).collect::<Vec<_>>(),
        "devices_error": devices_error,
    })
    .to_string()
}
