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

#[test]
fn qr_pairing_is_passwordless_single_device_and_syncs_both_ways() {
    use pastazzo_sync::pairing;
    let (_, _, links) = start_server(1);
    let owner = account::join(&links[0], "seb", PASSWORD, "MacBook").unwrap();
    let (link, offer) = pairing::create(&owner).unwrap();
    let link = link.to_link().to_string();
    let (sender, receiver) = std::sync::mpsc::channel();
    let phone = std::thread::spawn(move || {
        pairing::join(&link, "iPhone", &mut |code| {
            sender.send(code.to_owned()).unwrap();
        })
    });
    let code = receiver
        .recv_timeout(std::time::Duration::from_secs(20))
        .unwrap();
    for _ in 0..100 {
        if pairing::status(&owner, &offer).unwrap().peer.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(pairing::approve(&owner, &offer, "WRONG CODE").is_err());
    pairing::approve(&owner, &offer, &code).unwrap();
    let phone = phone.join().unwrap().unwrap();
    assert_eq!(
        phone.account_key.fingerprint(),
        owner.account_key.fingerprint()
    );
    assert!(pairing::status(&owner, &offer).unwrap().completed);
    assert!(
        account::device_names(&owner)
            .unwrap()
            .iter()
            .any(|(name, _)| name == "iPhone")
    );
    let (owner, local) = device(owner, &temp_state("qr-mac"), false);
    let (phone, mobile) = device(phone, &temp_state("qr-phone"), false);
    local.copy(Content::Text("Mac QR test".into()));
    owner.send_new_copies();
    phone.receive_once(0).unwrap();
    assert_eq!(mobile.applied(), [Content::Text("Mac QR test".into())]);
    assert_eq!(*mobile.origins.lock().unwrap(), ["MacBook"]);
    mobile.copy(Content::Text("iPhone QR test".into()));
    phone.send_new_copies();
    owner.receive_once(0).unwrap();
    assert_eq!(local.applied(), [Content::Text("iPhone QR test".into())]);
    assert_eq!(*local.origins.lock().unwrap(), ["iPhone"]);
}

#[test]
fn qr_requests_reject_swaps_reuse_cancellation_and_revoked_owners() {
    use pastazzo_core::api::{B64, PairingPeer, PairingReply};
    use pastazzo_core::device::DeviceKeys;
    use pastazzo_core::pairing;
    use pastazzo_sync::pairing as client;
    let (url, fingerprint, links) = start_server(1);
    let owner = account::join(&links[0], "seb", PASSWORD, "MacBook").unwrap();
    let other = login_approved(&url, &fingerprint, &owner, "Mac Pro");
    let (link, offer) = client::create(&owner).unwrap();
    let remote = Remote::new(&url);
    let path = format!("/v1/pairings/{}/request", B64::encode(&offer.id));
    let device = DeviceKeys::generate(&mut OsRng);
    let peer = |device: &DeviceKeys, name: &str| PairingPeer {
        device: B64(device.public().to_bytes().to_vec()),
        name: name.into(),
        proof: B64(link
            .invite
            .key
            .start_proof("seb", &pairing::peer_message(&device.public(), name))
            .to_vec()),
    };
    let valid = peer(&device, "iPhone");
    let mut invalid = valid.clone();
    invalid.name = "Swapped name".into();
    assert!(
        remote
            .post_json::<_, PairingReply>(&path, &invalid)
            .unwrap_err()
            .contains("403")
    );
    invalid = valid.clone();
    invalid.device = B64(DeviceKeys::generate(&mut OsRng)
        .public()
        .to_bytes()
        .to_vec());
    assert!(
        remote
            .post_json::<_, PairingReply>(&path, &invalid)
            .unwrap_err()
            .contains("403")
    );
    assert!(
        remote
            .post_json::<_, PairingReply>(&path, &valid)
            .unwrap()
            .grant
            .is_none()
    );
    let second = peer(&DeviceKeys::generate(&mut OsRng), "Other iPhone");
    assert!(
        remote
            .post_json::<_, PairingReply>(&path, &second)
            .unwrap_err()
            .contains("409")
    );
    assert!(
        remote
            .pairing_cancel(&other, &offer.id)
            .unwrap_err()
            .contains("403")
    );
    let grant = pairing::seal_grant(
        &owner.device,
        &offer.id,
        &owner.account,
        &device.public(),
        &owner.account_key,
        owner.account_secret.as_ref().unwrap(),
        &mut OsRng,
    );
    assert!(
        remote
            .pairing_grant(&other, &offer.id, &grant)
            .unwrap_err()
            .contains("403")
    );
    remote.pairing_grant(&owner, &offer.id, &grant).unwrap();
    remote.pairing_grant(&owner, &offer.id, &grant).unwrap();
    let mut changed = grant.clone();
    changed[100] ^= 1;
    assert!(
        remote
            .pairing_grant(&owner, &offer.id, &changed)
            .unwrap_err()
            .contains("409")
    );
    let reply: PairingReply = remote.post_json(&path, &valid).unwrap();
    let bytes = reply.grant.unwrap().0;
    assert!(
        !bytes
            .windows(32)
            .any(|w| w == owner.account_key.expose_secret())
    );
    assert!(
        !bytes
            .windows(32)
            .any(|w| w == owner.account_secret.as_ref().unwrap().expose_secret())
    );
    pairing::open_grant(&link, &device, &bytes).unwrap();
    assert!(
        remote
            .post_json::<_, PairingReply>(&path, &second)
            .unwrap_err()
            .contains("409")
    );
    remote.pairing_cancel(&owner, &offer.id).unwrap();
    assert!(
        remote
            .post_json::<_, PairingReply>(&path, &valid)
            .unwrap_err()
            .contains("404")
    );
    let (_, expired_offer) = client::create(&owner).unwrap();
    let _ = client::create(&owner).unwrap();
    assert!(
        client::status(&owner, &expired_offer)
            .unwrap_err()
            .contains("404")
    );
    let (revoked_link, revoked_offer) = client::create(&owner).unwrap();
    remote.revoke_device(&other, &owner.device.id()).unwrap();
    let proof = PairingPeer {
        device: valid.device,
        name: "iPhone".into(),
        proof: B64(revoked_link
            .invite
            .key
            .start_proof("seb", &pairing::peer_message(&device.public(), "iPhone"))
            .to_vec()),
    };
    assert!(
        remote
            .post_json::<_, PairingReply>(
                &format!("/v1/pairings/{}/request", B64::encode(&revoked_offer.id)),
                &proof
            )
            .unwrap_err()
            .contains("403")
    );
}

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

/// Logs a device in the way the CLI does, with `approver` approving it once
/// it waits, after checking both show the same code.
fn login_approved(url: &str, fingerprint: &[u8; 32], approver: &State, name: &str) -> State {
    let (url, fingerprint, name) = (url.to_owned(), *fingerprint, name.to_owned());
    let (code_tx, code_rx) = std::sync::mpsc::channel();
    let waiting = std::thread::spawn(move || {
        account::login(&url, &fingerprint, "seb", PASSWORD, &name, &mut |code| {
            code_tx.send(code.to_owned()).unwrap();
        })
    });
    let code = code_rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .unwrap();
    let pending = loop {
        let (_, pending) = account::devices_and_pending(approver).unwrap();
        if !pending.is_empty() {
            break pending;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(pending[0].code, code, "both devices show the same code");
    account::approve(approver, &pending[0].public.id).unwrap();
    waiting.join().unwrap().unwrap()
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
    let mac = login_approved(&url, &fingerprint, &laptop, "Mac Pro");
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
fn a_retry_after_a_lost_upload_response_does_not_duplicate_the_item() {
    use pastazzo_core::item::{ItemHeader, SealedItem};
    let (url, fingerprint, links) = start_server(1);
    let state = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let receiver = login_approved(&url, &fingerprint, &state, "Mac Pro");
    let remote = Remote::new(&url);
    let sealed = SealedItem::seal(
        &state.account_key,
        &state.account,
        ItemHeader {
            id: [17; 16],
            device: state.device.id(),
            epoch: state.account_key.epoch(),
            created_at: pastazzo_sync::now(),
        },
        &Content::Text("Retry without duplicates".into()),
        &mut OsRng,
    )
    .unwrap()
    .to_bytes();
    let cursor = remote.post_item(&state, &sealed).unwrap();
    assert_eq!(remote.post_item(&state, &sealed).unwrap(), cursor);
    let page = remote.items(&receiver, receiver.cursor, 0).unwrap();
    assert_eq!(page.items.len(), 1);
    let mut changed = sealed;
    *changed.last_mut().unwrap() ^= 1;
    assert!(
        remote
            .post_item(&state, &changed)
            .unwrap_err()
            .contains("409")
    );
}

#[test]
fn concurrent_outbox_retries_acknowledge_the_same_copy_once() {
    let (url, fingerprint, links) = start_server(1);
    let mut sender = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let receiver = login_approved(&url, &fingerprint, &sender, "Mac Pro");
    let sender_path = temp_state("concurrent-outbox");
    let receiver_path = temp_state("concurrent-receiver");
    sender.server_url = "http://127.0.0.1:1".into();
    let (offline, _) = device(sender, &sender_path, false);
    let content = Content::Text("one copy, concurrent retries".into());
    assert!(offline.send(&content).is_err());
    let mut restored = State::load_with(&sender_path, &NoKeychain).unwrap();
    restored.server_url = url;
    let (first, _) = device(restored, &sender_path, false);
    let (second, _) = device(
        State::load_with(&sender_path, &NoKeychain).unwrap(),
        &sender_path,
        false,
    );
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            first.flush_outbox()
        });
        let b = scope.spawn(|| {
            barrier.wait();
            second.flush_outbox()
        });
        a.join().unwrap().unwrap();
        b.join().unwrap().unwrap();
    });
    let (receiver, clipboard) = device(receiver, &receiver_path, false);
    receiver.receive_once(0).unwrap();
    assert_eq!(clipboard.applied(), [content]);
}

#[test]
fn failed_upload_survives_restart_and_is_retried_without_duplicates() {
    use std::os::unix::fs::PermissionsExt;
    let (url, fingerprint, links) = start_server(1);
    let mut sender = account::join(&links[0], "seb", PASSWORD, "offline laptop").unwrap();
    let receiver = login_approved(&url, &fingerprint, &sender, "Mac");
    let sender_path = temp_state("offline");
    let receiver_path = temp_state("online");
    sender.server_url = "http://127.0.0.1:1".into();
    let (offline, _) = device(sender, &sender_path, false);
    let content = Content::Text("a copy made without a network".into());
    assert!(offline.send(&content).is_err());
    let root = sender_path.with_file_name("outbox");
    let directory = std::fs::read_dir(&root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let file = std::fs::read_dir(&directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!String::from_utf8_lossy(&std::fs::read(&file).unwrap()).contains("a copy made"));
    drop(offline);
    let mut restored = State::load_with(&sender_path, &NoKeychain).unwrap();
    restored.server_url = url;
    let (restarted, _) = device(restored, &sender_path, false);
    let (receiver, clipboard) = device(receiver, &receiver_path, false);
    restarted.send_new_copies();
    receiver.receive_once(0).unwrap();
    assert_eq!(clipboard.applied(), [content]);
    assert!(!file.exists());
    restarted.flush_outbox().unwrap();
    receiver.receive_once(0).unwrap();
    assert_eq!(clipboard.applied().len(), 1);
    let mut pending = State::load_with(&sender_path, &NoKeychain).unwrap();
    pending.server_url = "http://127.0.0.1:1".into();
    let (pending, _) = device(pending, &sender_path, false);
    assert!(
        pending
            .send(&Content::Text("discard this pending copy".into()))
            .is_err()
    );
    pastazzo_sync::daemon::cancel_pending(&sender_path).unwrap();
    assert!(!root.exists());
    restarted.flush_outbox().unwrap();
    receiver.receive_once(0).unwrap();
    assert_eq!(clipboard.applied().len(), 1);
}

#[test]
fn wrong_password_and_reused_invite_are_refused() {
    let (url, fingerprint, links) = start_server(1);
    account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let error = account::login(
        &url,
        &fingerprint,
        "seb",
        "not the password",
        "x",
        &mut |_| {},
    )
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
    let error = account::login(&url, &[7; 32], "seb", PASSWORD, "x", &mut |_| {})
        .err()
        .unwrap();
    assert!(error.contains("fingerprint"), "{error}");
}

#[test]
fn revoked_devices_are_locked_out() {
    let (url, fingerprint, links) = start_server(1);
    let laptop = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let phone = login_approved(&url, &fingerprint, &laptop, "phone");
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
    let mac = login_approved(&url, &fingerprint, &laptop, "Mac Pro");
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
    let mac = login_approved(&url, &fingerprint, &laptop, "Mac Pro");
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
    let mac = login_approved(&url, &fingerprint, &laptop, "Mac Pro");
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
    let mac = login_approved(&url, &fingerprint, &laptop, "Mac Pro");
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

#[test]
fn a_device_waiting_for_approval_gets_nothing() {
    let (url, fingerprint, links) = start_server(1);
    let laptop = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let laptop_path = temp_state("waiting-laptop");
    let (laptop_daemon, _) = device(laptop, &laptop_path, false);
    laptop_daemon
        .send(&Content::Text("private".into()))
        .unwrap();
    let laptop = State::load_with(&laptop_path, &NoKeychain).unwrap();

    // Someone with the password logs in from another device and waits...
    let (url2, name) = (url.clone(), "stranger".to_owned());
    std::thread::spawn(move || {
        account::login(&url2, &fingerprint, "seb", PASSWORD, &name, &mut |_| {})
    });
    let pending = loop {
        let (_, pending) = account::devices_and_pending(&laptop).unwrap();
        if !pending.is_empty() {
            break pending;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    // ...and isn't listed as a device of the account: just as waiting.
    let names: Vec<String> = account::device_names(&laptop)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, ["laptop"]);
    assert_eq!(pending.len(), 1);

    // Rejecting it: its wait ends with an error, and it never had the key.
    Remote::new(&url)
        .revoke_device(&laptop, &pending[0].public.id)
        .unwrap();
}

#[test]
fn a_rejected_device_stops_waiting() {
    let (url, fingerprint, links) = start_server(1);
    let laptop = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let url2 = url.clone();
    let waiting = std::thread::spawn(move || {
        account::login(
            &url2,
            &fingerprint,
            "seb",
            PASSWORD,
            "stranger",
            &mut |_| {},
        )
    });
    let pending = loop {
        let (_, pending) = account::devices_and_pending(&laptop).unwrap();
        if !pending.is_empty() {
            break pending;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    Remote::new(&url)
        .revoke_device(&laptop, &pending[0].public.id)
        .unwrap();
    let error = waiting.join().unwrap().err().unwrap();
    assert!(error.contains("revoked"), "{error}");
}

#[test]
fn only_approved_devices_can_approve() {
    let (url, fingerprint, links) = start_server(1);
    let laptop = account::join(&links[0], "seb", PASSWORD, "laptop").unwrap();
    let mac = login_approved(&url, &fingerprint, &laptop, "Mac Pro");
    // The Mac, approved, can approve a third device in turn.
    let phone = login_approved(&url, &fingerprint, &mac, "phone");
    assert_eq!(
        phone.account_key.fingerprint(),
        laptop.account_key.fingerprint()
    );
    let names: Vec<String> = account::device_names(&phone)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, ["laptop", "Mac Pro", "phone"]);
}
