//! The history CLI end to end, in a temporary data directory.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn data_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pastazzo-history-{}-{}",
        std::process::id(),
        rand_suffix()
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
