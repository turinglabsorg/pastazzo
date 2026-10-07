//! pastazzo-server: stores and relays end-to-end encrypted clipboard items.
//!
//! ```text
//! pastazzo-server serve [--listen 127.0.0.1:4320] [--registration invite|open|closed]
//! pastazzo-server invite --url https://clip.example.org [--hours 72]
//! pastazzo-server info
//! pastazzo-server users
//! pastazzo-server delete-user <username>
//! ```
//!
//! Every command takes `--data <dir>` (default `$PASTAZZO_SERVER_DATA`, then
//! `~/.local/share/pastazzo-server`), holding the server keys and database.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pastazzo_core::api::{B64, Registration};
use pastazzo_core::invite::{Invite, InviteKey};
use pastazzo_core::server::ServerKeys;
use rand::rngs::OsRng;

use pastazzo_server::app;
use pastazzo_server::store::Store;

const USAGE: &str = "usage:
  pastazzo-server serve [--listen 127.0.0.1:4320] [--registration invite|open|closed]
  pastazzo-server invite --url <public url> [--hours 72]
  pastazzo-server info
  pastazzo-server users
  pastazzo-server delete-user <username>

every command takes --data <dir> (default $PASTAZZO_SERVER_DATA or ~/.local/share/pastazzo-server)";

fn main() {
    if let Err(error) = run() {
        eprintln!("pastazzo-server: {error}");
        std::process::exit(1);
    }
}

struct Args {
    command: String,
    positional: Vec<String>,
    options: Vec<(String, String)>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = std::env::args().skip(1);
        let command = args.next().ok_or(USAGE)?;
        let (mut positional, mut options) = (Vec::new(), Vec::new());
        while let Some(arg) = args.next() {
            if let Some(name) = arg.strip_prefix("--") {
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

    fn data_dir(&self) -> Result<PathBuf, String> {
        if let Some(dir) = self.option("data") {
            return Ok(PathBuf::from(dir));
        }
        if let Ok(dir) = std::env::var("PASTAZZO_SERVER_DATA") {
            return Ok(PathBuf::from(dir));
        }
        let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
        Ok(PathBuf::from(home).join(".local/share/pastazzo-server"))
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;
    let data = args.data_dir()?;
    match args.command.as_str() {
        "serve" => serve(&args, &data),
        "invite" => invite(&args, &data),
        "info" => {
            let keys = load_or_create_keys(&data)?;
            print_identity(&keys);
            Ok(())
        }
        "users" => {
            for (username, devices) in open_store(&data)?.usernames().map_err(|e| e.to_string())? {
                println!("{username}\t{devices} device(s)");
            }
            Ok(())
        }
        "delete-user" => {
            let username = args.positional.first().ok_or(USAGE)?;
            if open_store(&data)?
                .delete_account(username)
                .map_err(|e| e.to_string())?
            {
                println!("deleted {username}, with its devices and items");
                Ok(())
            } else {
                Err(format!("no user named {username}"))
            }
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other}\n{USAGE}")),
    }
}

fn create_data_dir(data: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(data)
        .map_err(|e| format!("create {}: {e}", data.display()))
}

fn open_store(data: &Path) -> Result<Store, String> {
    create_data_dir(data)?;
    Store::open(&data.join("pastazzo.db")).map_err(|e| format!("open database: {e}"))
}

/// The server keys live in `<data>/keys`, readable only by the owner. They're
/// created on first use; losing them means every account must register again.
fn load_or_create_keys(data: &Path) -> Result<ServerKeys, String> {
    let path = data.join("keys");
    match fs::read(&path) {
        Ok(bytes) => ServerKeys::from_secret_bytes(&zeroize::Zeroizing::new(bytes))
            .map_err(|e| format!("read {}: {e}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_data_dir(data)?;
            let keys = ServerKeys::generate(&mut OsRng);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|e| format!("create {}: {e}", path.display()))?;
            file.write_all(&keys.to_secret_bytes())
                .map_err(|e| e.to_string())?;
            eprintln!(
                "pastazzo-server: created new server keys in {}",
                path.display()
            );
            Ok(keys)
        }
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

fn print_identity(keys: &ServerKeys) {
    println!(
        "fingerprint: {}",
        B64::encode(&keys.identity().fingerprint())
    );
}

fn invite(args: &Args, data: &Path) -> Result<(), String> {
    let url = args
        .option("url")
        .ok_or("invite needs --url <the URL clients use to reach this server>")?;
    let hours: u64 = args
        .option("hours")
        .unwrap_or("72")
        .parse()
        .map_err(|_| "--hours must be a number")?;
    let keys = load_or_create_keys(data)?;
    let store = open_store(data)?;
    let key = InviteKey::generate(&mut OsRng);
    let verifier = key.verifier();
    let now = app::now();
    store
        .add_invite(
            &verifier.id,
            &verifier.public,
            now,
            now + hours * 60 * 60 * 1000,
        )
        .map_err(|e| e.to_string())?;
    let invite = Invite {
        server_url: url.trim_end_matches('/').to_owned(),
        fingerprint: keys.identity().fingerprint(),
        key,
    };
    // The secret is only in this link: hand it over out of band.
    println!("{}", invite.to_link().as_str());
    eprintln!("one-time invite, valid for {hours} hours");
    Ok(())
}

fn serve(args: &Args, data: &Path) -> Result<(), String> {
    let listen = args.option("listen").unwrap_or("127.0.0.1:4320").to_owned();
    let registration = match args.option("registration").unwrap_or("invite") {
        "invite" => Registration::Invite,
        "open" => Registration::Open,
        "closed" => Registration::Closed,
        other => return Err(format!("unknown registration mode {other}")),
    };
    let keys = load_or_create_keys(data)?;
    print_identity(&keys);
    let app = Arc::new(app::App::new(keys, open_store(data)?, registration));

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&listen)
            .await
            .map_err(|e| format!("listen on {listen}: {e}"))?;
        eprintln!("pastazzo-server: listening on {listen}, registration {registration:?}");
        axum::serve(listener, app::router(app))
            .with_graceful_shutdown(shutdown())
            .await
            .map_err(|e| e.to_string())
    })
}

async fn shutdown() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}
