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
    fn apply(&mut self, content: &Content) -> Result<()> {
        self.applied.lock().unwrap().push(content.clone());
        if self.echo {
            self.copies.lock().unwrap().push(content.clone());
        }
        Ok(())
    }
    fn remember(&mut self, content: &Content) -> Result<()> {
        self.remembered.lock().unwrap().push(content.clone());
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
            State::load(path).unwrap(),
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
        State::load(&mac_path).unwrap(),
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
