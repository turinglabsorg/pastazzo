//! Two devices syncing through a real server running in-process, over HTTP.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pastazzo_core::api::Registration;
use pastazzo_core::invite::{Invite, InviteKey};
use pastazzo_core::item::Content;
use pastazzo_core::server::ServerKeys;
use pastazzo_server::app::{App, router};
use pastazzo_server::store::Store;
use pastazzo_sync::clipboard::Clipboard;
use pastazzo_sync::daemon::Daemon;
use pastazzo_sync::remote::Remote;
use pastazzo_sync::secrets::NoKeychain;
use pastazzo_sync::state::State;
use pastazzo_sync::{Result, account};
use rand::rngs::OsRng;

const PASSWORD: &str = "correct horse battery staple";

/// Starts a server on a free local port and returns its URL, its
/// fingerprint and invite links made the way the admin CLI makes them.
fn start_server(invites: usize) -> (String, [u8; 32], Vec<String>) {
    let keys = ServerKeys::generate(&mut OsRng);
    let fingerprint = keys.identity().fingerprint();
    let store = Store::open(Path::new(":memory:")).unwrap();
    let invite_keys: Vec<InviteKey> = (0..invites)
        .map(|_| InviteKey::generate(&mut OsRng))
        .collect();
    for key in &invite_keys {
        let verifier = key.verifier();
        store
            .add_invite(&verifier.id, &verifier.public, 0, u64::MAX / 2)
            .unwrap();
    }
    let app = Arc::new(App::new(keys, store, Registration::Invite));

    let (address_tx, address_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                address_tx.send(listener.local_addr().unwrap()).unwrap();
                axum::serve(listener, router(app)).await.unwrap();
            });
    });
    let url = format!("http://{}", address_rx.recv().unwrap());
    let links = invite_keys
        .into_iter()
        .map(|key| {
            Invite {
                server_url: url.clone(),
                fingerprint,
                key,
            }
            .to_link()
            .to_string()
        })
        .collect();
    (url, fingerprint, links)
}

/// A clipboard the test drives: copies made "by the user" and what the
/// daemon applied. With `echo`, applying an item also makes it show up as a
/// new local copy, the way the pastazzo history does on Linux.
#[derive(Clone, Default)]
struct FakeClipboard {
    copies: Arc<Mutex<Vec<Content>>>,
    applied: Arc<Mutex<Vec<Content>>>,
    remembered: Arc<Mutex<Vec<Content>>>,
    origins: Arc<Mutex<Vec<String>>>,
    cleared: Arc<Mutex<usize>>,
    echo: bool,
}

impl FakeClipboard {
    fn copy(&self, content: Content) {
        self.copies.lock().unwrap().push(content);
    }
    fn applied(&self) -> Vec<Content> {
        self.applied.lock().unwrap().clone()
    }
}

impl Clipboard for FakeClipboard {
    fn poll(&mut self) -> Vec<Content> {
        std::mem::take(&mut *self.copies.lock().unwrap())
    }
    fn apply(&mut self, content: &Content, origin: &str) -> Result<()> {
        self.applied.lock().unwrap().push(content.clone());
        self.origins.lock().unwrap().push(origin.to_owned());
        if self.echo {
            self.copies.lock().unwrap().push(content.clone());
        }
        Ok(())
    }
    fn remember(&mut self, content: &Content, origin: &str) -> Result<()> {
        self.remembered.lock().unwrap().push(content.clone());
        self.origins.lock().unwrap().push(origin.to_owned());
        Ok(())
    }
    fn clear_history(&mut self) -> Result<()> {
        *self.cleared.lock().unwrap() += 1;
        Ok(())
    }
}

fn temp_state(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pastazzo-e2e-{}-{name}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("sync.json")
}

fn device(state: State, path: &Path, echo: bool) -> (Daemon, FakeClipboard) {
    state.save(path).unwrap();
    let clipboard = FakeClipboard {
        echo,
        ..Default::default()
    };
    (
        Daemon::new(
            State::load_with(path, &NoKeychain).unwrap(),
            path,
            Box::new(clipboard.clone()),
        ),
        clipboard,
    )
}

#[test]
fn two_devices_sync_both_ways() {
    let (url, fingerprint, links) = start_server(1);
    let laptop_path = temp_state("laptop");
    let mac_path = temp_state("mac");

    let laptop = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let mac = account::login(&url, &fingerprint, "seb", PASSWORD, "Mac Pro").unwrap();
    assert_eq!(
        laptop.account_key.expose_secret(),
        mac.account_key.expose_secret()
    );
    let names: Vec<String> = account::device_names(&mac)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, ["laptop", "Mac Pro"]);

    // The laptop's history echoes everything it applies; the Mac's doesn't.
    let (laptop, laptop_clipboard) = device(laptop, &laptop_path, true);
    let (mac, mac_clipboard) = device(mac, &mac_path, false);

    // Laptop → Mac: text.
    laptop_clipboard.copy(Content::Text("ciao dal portatile".into()));
    laptop.send_new_copies();
    mac.receive_once(5).unwrap();
    assert_eq!(
        mac_clipboard.applied(),
        [Content::Text("ciao dal portatile".into())]
    );

    // The Mac knows where it came from.
    assert_eq!(*mac_clipboard.origins.lock().unwrap(), ["laptop"]);

    // Mac → laptop: an image.
    let image = Content::Image {
        mime: "image/png".into(),
        data: vec![0x89, b'P', b'N', b'G', 1, 2, 3],
    };
    mac_clipboard.copy(image.clone());
    mac.send_new_copies();
    laptop.receive_once(5).unwrap();
    assert_eq!(laptop_clipboard.applied(), std::slice::from_ref(&image));

    // The laptop's history now shows the image as a new "copy": it must not
    // bounce back to the Mac.
    laptop.send_new_copies();
    mac.receive_once(0).unwrap();
    assert_eq!(mac_clipboard.applied().len(), 1);

    // A device never applies its own items.
    laptop.receive_once(0).unwrap();
    assert_eq!(laptop_clipboard.applied().len(), 1);

    // The cursor survives a restart: nothing is applied twice.
    let restarted_clipboard = FakeClipboard::default();
    let restarted = Daemon::new(
        State::load_with(&mac_path, &NoKeychain).unwrap(),
        &mac_path,
        Box::new(restarted_clipboard.clone()),
    );
    restarted.receive_once(0).unwrap();
    assert!(restarted_clipboard.applied().is_empty());
}

#[test]
fn wrong_password_and_reused_invite_are_refused() {
    let (url, fingerprint, links) = start_server(1);
    account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let error = account::login(&url, &fingerprint, "seb", "not the password", "x")
        .err()
        .unwrap();
    assert!(error.contains("wrong username or password"), "{error}");
    let error = account::join(&links[0], "other", PASSWORD, "x")
        .err()
        .unwrap();
    assert!(error.contains("invite"), "{error}");
}

#[test]
fn a_pinned_fingerprint_that_doesnt_match_stops_the_client() {
    let (url, _, links) = start_server(1);
    account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let error = account::login(&url, &[7; 32], "seb", PASSWORD, "x")
        .err()
        .unwrap();
    assert!(error.contains("fingerprint"), "{error}");
}

#[test]
fn revoked_devices_are_locked_out() {
    let (url, fingerprint, links) = start_server(1);
    let laptop = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let phone = account::login(&url, &fingerprint, "seb", PASSWORD, "phone").unwrap();
    Remote::new(&url)
        .revoke_device(&laptop, &phone.device.id())
        .unwrap();
    let error = Remote::new(&url).items(&phone, 0, 0).err().unwrap();
    assert!(error.contains("revoked"), "{error}");
    let names: Vec<String> = account::device_names(&laptop)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, ["laptop"]);
}

/// Some bytes no padding or compression makes small.
fn noise(len: usize) -> Vec<u8> {
    let mut data = vec![0u8; len];
    rand::RngCore::fill_bytes(&mut OsRng, &mut data);
    data
}

#[test]
fn big_items_are_announced_and_show_up_as_pending() {
    let (url, fingerprint, links) = start_server(1);
    let laptop = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let mac = account::login(&url, &fingerprint, "seb", PASSWORD, "Mac Pro").unwrap();
    let remote = Remote::new(&url);

    // The Mac is waiting; the laptop announces an upload: the wait ends at
    // once with the transfer in it, from the laptop.
    let seen = remote.items(&mac, mac.cursor, 0).unwrap().pending_version;
    remote.announce(&laptop, &[9; 16], 3_000_000).unwrap();
    let started = std::time::Instant::now();
    let page = remote
        .items_with_progress(&mac, mac.cursor, 10, Some(seen), &mut |_, _| {})
        .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(page.pending.len(), 1);
    assert_eq!(page.pending[0].device.0, laptop.device.id().to_vec());
    assert_eq!(page.pending[0].size, 3_000_000);

    // A real big item goes through the announced path and arrives whole,
    // with progress written for the UIs and nothing left pending after.
    let laptop_path = temp_state("big-laptop");
    let mac_path = temp_state("big-mac");
    let status_dir = laptop_path.parent().unwrap().join("status");
    let (laptop, laptop_clipboard) = device(laptop, &laptop_path, false);
    let laptop = laptop.with_status_dir(status_dir.clone()).unwrap();
    let (mac, mac_clipboard) = device(mac, &mac_path, false);
    let image = Content::Image {
        mime: "image/png".into(),
        data: noise(2_000_000),
    };
    laptop_clipboard.copy(image.clone());
    laptop.send_new_copies();
    mac.receive_once(5).unwrap();
    while mac_clipboard.applied().is_empty() {
        mac.receive_once(5).unwrap();
    }
    assert_eq!(mac_clipboard.applied(), [image]);
    let transfers = std::fs::read_to_string(status_dir.join("transfers.json")).unwrap();
    assert!(transfers.contains("\"transfers\":[]"), "{transfers}");
    let status = std::fs::read_to_string(status_dir.join("status.json")).unwrap();
    assert!(status.contains("\"device_name\":\"laptop\""), "{status}");
}

#[test]
fn clearing_everywhere_empties_the_server_and_reaches_the_other_devices() {
    let (url, fingerprint, links) = start_server(1);
    let laptop = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let mac = account::login(&url, &fingerprint, "seb", PASSWORD, "Mac Pro").unwrap();
    let laptop_path = temp_state("clear-laptop");
    let mac_path = temp_state("clear-mac");
    let (laptop, _) = device(laptop, &laptop_path, false);
    let (mac, mac_clipboard) = device(mac, &mac_path, false);

    laptop
        .send(&Content::Text("something private".into()))
        .unwrap();
    let state = State::load_with(&laptop_path, &NoKeychain).unwrap();
    let remote = Remote::new(&url);
    remote.delete_items(&state).unwrap();
    assert!(remote.items(&state, 0, 0).unwrap().items.is_empty());

    laptop.send(&Content::ClearHistory).unwrap();
    mac.receive_once(5).unwrap();
    assert_eq!(*mac_clipboard.cleared.lock().unwrap(), 1);
    // A clear is never mistaken for a copy.
    assert!(mac_clipboard.applied().is_empty());
}

#[test]
fn every_device_shows_the_same_account_key_fingerprint() {
    let (url, fingerprint, links) = start_server(1);
    let laptop = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let mac = account::login(&url, &fingerprint, "seb", PASSWORD, "Mac Pro").unwrap();
    assert_eq!(
        laptop.account_key.fingerprint(),
        mac.account_key.fingerprint()
    );
    let from_laptop = account::devices(&laptop).unwrap();
    let from_mac = account::devices(&mac).unwrap();
    assert_eq!(from_laptop.len(), 2);
    // Both see the same devices with the same key fingerprints.
    let fingerprints = |devices: &[account::DeviceInfo]| {
        devices
            .iter()
            .map(|d| (d.name.clone(), d.fingerprint))
            .collect::<Vec<_>>()
    };
    assert_eq!(fingerprints(&from_laptop), fingerprints(&from_mac));
    assert_eq!(
        from_mac.iter().find(|d| d.this).unwrap().fingerprint,
        mac.device.public().fingerprint()
    );
}

#[test]
fn devices_dont_download_their_own_items() {
    let (url, fingerprint, links) = start_server(1);
    let laptop = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let mac = account::login(&url, &fingerprint, "seb", PASSWORD, "Mac Pro").unwrap();
    let laptop_path = temp_state("own-laptop");
    let (laptop_daemon, _) = device(laptop, &laptop_path, false);
    laptop_daemon.send(&Content::Text("mine".into())).unwrap();

    let remote = Remote::new(&url);
    let laptop = State::load_with(&laptop_path, &NoKeychain).unwrap();
    let own = remote.items(&laptop, 0, 0).unwrap();
    assert!(own.items.is_empty());
    assert!(own.cursor > 0, "the cursor still moves past them");
    assert_eq!(remote.items(&mac, 0, 0).unwrap().items.len(), 1);
}

#[test]
fn public_endpoints_refuse_big_bodies() {
    let (url, _, _) = start_server(0);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let big = format!(
        "{{\"username\":\"seb\",\"request\":\"{}\"}}",
        "A".repeat(200 * 1024)
    );
    let response = agent
        .post(format!("{url}/v1/login/start"))
        .content_type("application/json")
        .send(big.as_bytes())
        .unwrap();
    assert_eq!(response.status().as_u16(), 413);
}
