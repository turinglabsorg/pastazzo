use pastazzo_core::api::{B64, Registration};
use pastazzo_core::invite::{Invite, InviteKey};
use pastazzo_core::item::{Content, SealedItem};
use pastazzo_core::server::ServerKeys;
use pastazzo_mobile::execute;
use pastazzo_server::app::{App, router};
use pastazzo_server::store::Store;
use pastazzo_sync::{account, remote::Remote, secrets::MemoryKeychain};
use rand::rngs::OsRng;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

#[test]
fn mobile_joins_by_approval_and_syncs_with_desktop_without_exposing_keys() {
    mobile_flow(false);
}

#[test]
fn mobile_pairs_by_qr_and_syncs_with_desktop_without_a_password() {
    mobile_flow(true);
}

fn mobile_flow(qr: bool) {
    let keys = ServerKeys::generate(&mut OsRng);
    let fingerprint = keys.identity().fingerprint();
    let invite = InviteKey::generate(&mut OsRng);
    let verifier = invite.verifier();
    let store = Store::open(Path::new(":memory:")).unwrap();
    store
        .add_invite(&verifier.id, &verifier.public, 0, u64::MAX / 2)
        .unwrap();
    let app = Arc::new(App::new(keys, store, Registration::Invite));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                axum::serve(listener, router(app)).await.unwrap();
            })
    });
    let server = format!("http://{}", rx.recv().unwrap());
    let link = Invite {
        server_url: server.clone(),
        fingerprint,
        key: invite,
    }
    .to_link()
    .to_string();
    let password = "correct horse battery staple";
    let desktop = account::join(&link, "mobile-test", password, "Mac Pro").unwrap();
    let root = std::env::temp_dir().join(format!("pastazzo-mobile-{}", rand::random::<u64>()));
    let keychain = Arc::new(MemoryKeychain::default());
    assert!(execute(&json!({"operation":"login","root":root,"server":server,"fingerprint":B64::encode(&[0;32]),
        "username":"mobile-test","password":password,"name":"iPhone"}),keychain.as_ref()).is_err());
    assert!(!root.join("sync.json").exists());
    let pairing = if qr {
        Some(pastazzo_sync::pairing::create(&desktop).unwrap())
    } else {
        None
    };
    let request = if let Some((link, _)) = &pairing {
        json!({"operation":"pair","root":root,"link":link.to_link().as_str(),"name":"iPhone"})
    } else {
        json!({"operation":"login","root":root,"server":server,"fingerprint":B64::encode(&fingerprint),
            "username":"mobile-test","password":password,"name":"iPhone"})
    };
    let login_keychain = keychain.clone();
    let login = std::thread::spawn(move || execute(&request, login_keychain.as_ref()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Ok(code) = std::fs::read_to_string(root.join("approval-code")) {
            if let Some((_, offer)) = &pairing {
                if pastazzo_sync::pairing::status(&desktop, offer)
                    .unwrap()
                    .peer
                    .is_some()
                {
                    pastazzo_sync::pairing::approve(&desktop, offer, &code).unwrap();
                    break;
                }
            } else {
                let (_, waiting) = account::devices_and_pending(&desktop).unwrap();
                if let Some(pending) = waiting.first() {
                    assert_eq!(code, pending.code);
                    account::approve(&desktop, &pending.public.id).unwrap();
                    break;
                }
            }
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    login.join().unwrap().unwrap();
    let state = std::fs::read_to_string(root.join("sync.json")).unwrap();
    assert!(!state.contains("account_key"));
    assert!(!state.contains(password));
    assert!(state.contains("keychain"));
    assert!(!root.join("approval-code").exists());

    execute(
        &json!({"operation":"save","root":root,"name":"iPhone","text":"Hello from iPhone"}),
        keychain.as_ref(),
    )
    .unwrap();
    let page = Remote::new(&server)
        .items(&desktop, desktop.cursor, 0)
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(!String::from_utf8_lossy(&page.items[0].0).contains("Hello from iPhone"));
    let sealed = SealedItem::from_bytes(&page.items[0].0).unwrap();
    assert_eq!(
        sealed.open(&desktop.account_key, &desktop.account).unwrap(),
        Content::Text("Hello from iPhone".into())
    );

    let path = root.join("desktop.json");
    desktop.save(&path).unwrap();
    struct NoClipboard;
    impl pastazzo_sync::clipboard::Clipboard for NoClipboard {
        fn poll(&mut self) -> Vec<Content> {
            vec![]
        }
        fn apply(&mut self, _: &Content, _: &str) -> pastazzo_sync::Result<()> {
            Ok(())
        }
        fn remember(&mut self, _: &Content, _: &str) -> pastazzo_sync::Result<()> {
            Ok(())
        }
        fn clear_history(&mut self) -> pastazzo_sync::Result<()> {
            Ok(())
        }
    }
    let daemon = pastazzo_sync::daemon::Daemon::new(desktop, &path, Box::new(NoClipboard));
    daemon
        .send(&Content::Text("Hello from Mac Pro".into()))
        .unwrap();
    let image = vec![137, 80, 78, 71, 13, 10, 26, 10];
    daemon
        .send(&Content::Image {
            mime: "image/png".into(),
            data: image.clone(),
        })
        .unwrap();
    daemon
        .send(&Content::Text("Hello from Mac Pro".into()))
        .unwrap();
    daemon
        .send(&Content::Image {
            mime: "image/png".into(),
            data: image.clone(),
        })
        .unwrap();
    execute(
        &json!({"operation":"refresh","root":root}),
        keychain.as_ref(),
    )
    .unwrap();
    let first = execute(
        &json!({"operation":"history","root":root}),
        keychain.as_ref(),
    )
    .unwrap();
    let entries = first["items"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert!(
        entries
            .iter()
            .any(|e| e["origin"] == "Mac Pro" && e["preview"] == "Hello from Mac Pro")
    );
    let image_id = entries.iter().find(|e| e["kind"] == "image").unwrap()["id"]
        .as_str()
        .unwrap();
    let item = execute(
        &json!({"operation":"item","root":root,"id":image_id}),
        keychain.as_ref(),
    )
    .unwrap();
    assert_eq!(
        B64::decode(item["item"]["data"].as_str().unwrap()).unwrap(),
        image
    );
    execute(
        &json!({"operation":"refresh","root":root}),
        keychain.as_ref(),
    )
    .unwrap();
    assert_eq!(
        execute(
            &json!({"operation":"history","root":root}),
            keychain.as_ref()
        )
        .unwrap(),
        first
    );
    assert!(
        execute(
            &json!({"operation":"item","root":root,"id":"../../sync"}),
            keychain.as_ref()
        )
        .is_err()
    );
    let path = root.join("sync.json");
    let original = pastazzo_sync::state::State::load_with(&path, keychain.as_ref()).unwrap();
    let mut offline = pastazzo_sync::state::State::load_with(&path, keychain.as_ref()).unwrap();
    offline.server_url = "http://127.0.0.1:1".into();
    offline.save_new_with(&path, keychain.as_ref()).unwrap();
    let shared_id = B64::encode(&[81; 16]);
    let save = json!({"operation":"save","root":root,"name":"iPhone","id":shared_id,"text":"Saved offline from Share"});
    let queued = execute(&save, keychain.as_ref()).unwrap();
    assert_eq!(queued["queued"], true);
    let queued_dir = root
        .join("mobile-outbox")
        .join(B64::encode(&offline.device.id()));
    let queued_file = queued_dir.join(format!("{shared_id}.sealed"));
    let ciphertext = std::fs::read(&queued_file).unwrap();
    assert!(!String::from_utf8_lossy(&ciphertext).contains("Saved offline from Share"));
    assert_eq!(
        execute(&save, keychain.as_ref()).unwrap()["duplicate"],
        true
    );
    assert_eq!(std::fs::read(&queued_file).unwrap(), ciphertext);
    offline.forget_with(&path, keychain.as_ref()).unwrap();
    original.save(&path).unwrap();
    execute(
        &json!({"operation":"refresh","root":root}),
        keychain.as_ref(),
    )
    .unwrap();
    assert!(!queued_file.exists());
    let item = execute(
        &json!({"operation":"item","root":root,"id":shared_id}),
        keychain.as_ref(),
    )
    .unwrap();
    assert_eq!(item["item"]["queued"], false);
    assert_eq!(item["item"]["text"], "Saved offline from Share");
    execute(&save, keychain.as_ref()).unwrap();
    let receiver = pastazzo_sync::state::State::load_with(
        &root.join("desktop.json"),
        &pastazzo_sync::secrets::NoKeychain,
    )
    .unwrap();
    let page = Remote::new(&server).items(&receiver, 0, 0).unwrap();
    assert_eq!(page.items.len(), 2);
    std::fs::write(&queued_file, ciphertext).unwrap();
    execute(
        &json!({"operation":"clear_local","root":root}),
        keychain.as_ref(),
    )
    .unwrap();
    assert!(!root.join("mobile-outbox").exists());
    assert_eq!(
        execute(
            &json!({"operation":"history","root":root}),
            keychain.as_ref()
        )
        .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    execute(
        &json!({"operation":"logout","root":root}),
        keychain.as_ref(),
    )
    .unwrap();
    assert!(!root.join("sync.json").exists());
    assert!(keychain.entries.lock().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}
