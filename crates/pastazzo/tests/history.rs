//! The history CLI end to end, in a temporary data directory.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn data_dir() -> PathBuf {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pastazzo-history-{}-{}-{}",
        std::process::id(),
        rand_suffix(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn pastazzo(data: &Path, args: &[&str], input: &[u8]) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pastazzo"))
        .args(args)
        .env("XDG_DATA_HOME", data)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "pastazzo {args:?} failed");
    String::from_utf8(output.stdout).unwrap()
}

fn origins(data: &Path, query: &str) -> Vec<(String, String)> {
    let json = pastazzo(data, &["search", query], b"");
    // Every item has "text":"...","path":"...","origin":"..." in this order.
    json.split("{\"id\":")
        .skip(1)
        .map(|item| {
            let field = |name: &str| {
                let start = item.find(&format!("\"{name}\":\"")).unwrap() + name.len() + 4;
                item[start..].split('"').next().unwrap().to_owned()
            };
            (field("text"), field("origin"))
        })
        .collect()
}

#[test]
fn items_remember_where_they_came_from() {
    let data = data_dir();
    pastazzo(&data, &["add"], b"local copy");
    pastazzo(&data, &["add", "--origin", "Mac Pro"], b"from the mac");
    pastazzo(
        &data,
        &["add-image", "image/png", "--origin", "iPhone"],
        b"\x89PNG fake",
    );

    let items = origins(&data, "");
    assert!(items.contains(&("from the mac".into(), "Mac Pro".into())));
    assert!(items.contains(&("local copy".into(), String::new())));
    assert!(pastazzo(&data, &["search", "image"], b"").contains("\"origin\":\"iPhone\""));

    // Searching finds items by device too.
    assert_eq!(
        origins(&data, "mac pro"),
        vec![("from the mac".into(), "Mac Pro".into())]
    );
}

#[test]
fn origin_follows_touch_and_goes_with_the_item() {
    let data = data_dir();
    let id = pastazzo(&data, &["add", "--origin", "XPS"], b"synced")
        .trim()
        .to_owned();
    let new_id = pastazzo(&data, &["touch", &id], b"").trim().to_owned();
    assert_ne!(id, new_id);
    assert_eq!(
        origins(&data, "synced"),
        vec![("synced".into(), "XPS".into())]
    );

    // Copying the same text locally makes it a local copy, with no stale origin left.
    pastazzo(&data, &["add"], b"synced");
    assert_eq!(
        origins(&data, "synced"),
        vec![("synced".into(), String::new())]
    );
    let items = data.join("pastazzo/items");
    let leftovers = std::fs::read_dir(&items)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|x| x == "origin")
        })
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn device_names_are_kept_to_one_short_line() {
    let data = data_dir();
    pastazzo(&data, &["add", "--origin", "evil\n\"name\"\u{7}"], b"x");
    let json = pastazzo(&data, &["search", ""], b"");
    assert!(json.contains("\"origin\":\"evil\\\"name\\\"\""), "{json}");
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect()
}

#[test]
fn the_history_is_readable_by_its_owner_only() {
    let data = data_dir();
    pastazzo(&data, &["add", "--origin", "XPS"], b"hunter2");
    pastazzo(&data, &["add-image", "image/png"], b"\x89PNG fake");

    let items = data.join("pastazzo/items");
    assert_eq!(mode(&data.join("pastazzo")), 0o700);
    assert_eq!(mode(&items), 0o700);
    let written = files(&items);
    assert_eq!(written.len(), 3, "{written:?}");
    for file in written {
        assert_eq!(mode(&file), 0o600, "{}", file.display());
    }
}

#[test]
fn an_older_open_history_is_closed_on_the_next_copy() {
    use std::os::unix::fs::PermissionsExt;
    let data = data_dir();
    let items = data.join("pastazzo/items");
    std::fs::create_dir_all(&items).unwrap();
    let old = items.join("00000000000000000001-0000000000000001.txt");
    std::fs::write(&old, "copied before the update").unwrap();
    for (path, mode) in [
        (&data.join("pastazzo"), 0o755),
        (&items, 0o755),
        (&old, 0o644),
    ] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    pastazzo(&data, &["add"], b"copied after");

    assert_eq!(mode(&data.join("pastazzo")), 0o700);
    assert_eq!(mode(&items), 0o700);
    for file in files(&items) {
        assert_eq!(mode(&file), 0o600, "{}", file.display());
    }
    assert_eq!(origins(&data, "before").len(), 1);
}

fn items(data: &Path, query: &str) -> Vec<serde_json::Value> {
    serde_json::from_str(&pastazzo(data, &["search", query], b"")).unwrap()
}

#[test]
fn sync_links_survive_reuse_but_not_a_fresh_copy() {
    for (command, payload) in [
        (vec!["add"], b"a routed clipboard item".as_slice()),
        (
            vec!["add-image", "image/png"],
            b"\x89PNG routed image".as_slice(),
        ),
    ] {
        let data = data_dir();
        let mut args = command.clone();
        args.extend(["--origin", "Workstation", "--source-app", "firefox"]);
        let id = pastazzo(&data, &args, payload).trim().to_owned();
        let mime = if command[0] == "add" {
            "text/plain"
        } else {
            "image/png"
        };
        let wire_id = "AQEBAQEBAQEBAQEBAQEBAQ";
        pastazzo(&data, &["link-sync", wire_id, mime], payload);
        let snapshot = serde_json::json!({"supported":true,"peers":[],"items":[{
            "item_id":wire_id,"origin_name":"Workstation","origin_device":"sender", "created_at":100,
            "uploaded_at":101,"receivers":[{"device_id":"receiver","device_name":"iPhone","received_at":102}]
        }]});
        let sync = data.join("pastazzo/sync");
        std::fs::create_dir_all(&sync).unwrap();
        std::fs::write(sync.join("deliveries.json"), snapshot.to_string()).unwrap();
        let copied_at = items(&data, "")[0]["copied_at"].clone();
        let new_id = pastazzo(&data, &["touch", &id, "--replay"], b"")
            .trim()
            .to_owned();
        let mut capture = command.clone();
        capture.extend([
            "--origin",
            "Wrong device",
            "--source-app",
            "quickshell",
            "--ignore-replay",
        ]);
        assert_eq!(pastazzo(&data, &capture, payload).trim(), new_id);
        let found = items(&data, "iPhone");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["origin"], "Workstation");
        assert_eq!(found[0]["copied_at"], copied_at);
        assert_eq!(
            found[0]["delivery"]["receivers"][0]["device_name"],
            "iPhone"
        );
        assert!(!found[0].as_object().unwrap().contains_key("destinations"));
        assert!(
            files(&data.join("pastazzo/items"))
                .iter()
                .all(|path| mode(path) == 0o600)
        );
        // The same content copied afresh has a new sync identity. Old receipts
        // must never be presented as proof of delivery of this new copy.
        pastazzo(&data, &capture, payload);
        assert!(items(&data, "")[0]["delivery"].is_null());
        assert!(items(&data, "iPhone").is_empty());
    }
}

#[test]
fn legacy_app_destinations_are_never_mistaken_for_receiving_devices() {
    let data = data_dir();
    pastazzo(
        &data,
        &["add", "--origin", "Laptop → firefox ⇒ code"],
        b"legacy",
    );
    let legacy = items(&data, "firefox");
    assert_eq!(legacy[0]["origin"], "Laptop");
    assert_eq!(legacy[0]["source_app"], "firefox");
    assert!(legacy[0]["delivery"].is_null());
    assert!(items(&data, "code").is_empty());
}

#[test]
fn source_metadata_is_pruned_with_the_item() {
    let data = data_dir();
    let old_id = pastazzo(&data, &["add", "--source-app", "firefox"], b"old")
        .trim()
        .to_owned();
    for index in 0..251 {
        pastazzo(&data, &["add"], format!("item {index}").as_bytes());
    }
    assert!(!data.join(format!("pastazzo/items/{old_id}.meta")).exists());
    assert_eq!(files(&data.join("pastazzo/items")).len(), 250);
    pastazzo(&data, &["clear"], b"");
    assert!(!data.join("pastazzo/items").exists());
}

#[test]
fn missing_items_and_invalid_sync_ids_cannot_create_metadata() {
    let data = data_dir();
    for id in ["../escape", "AQEBAQEBAQEBAQEBAQEBAQ"] {
        let output = Command::new(env!("CARGO_BIN_EXE_pastazzo"))
            .args(["link-sync", id, "text/plain"])
            .env("XDG_DATA_HOME", &data)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
    assert!(items(&data, "").is_empty());
}

#[test]
fn linking_a_sync_event_checks_the_full_saved_payload() {
    let data = data_dir();
    let id = pastazzo(&data, &["add"], b"expected content")
        .trim()
        .to_owned();
    std::fs::write(
        data.join(format!("pastazzo/items/{id}.txt")),
        b"different saved content",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pastazzo"))
        .args(["link-sync", "AQEBAQEBAQEBAQEBAQEBAQ", "text/plain"])
        .env("XDG_DATA_HOME", &data)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"expected content")
        .unwrap();
    assert!(!child.wait().unwrap().success());
    assert!(!data.join(format!("pastazzo/items/{id}.meta")).exists());
}
