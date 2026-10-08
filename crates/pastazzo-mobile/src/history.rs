use pastazzo_core::api::B64;
use pastazzo_core::item::Content;
use pastazzo_sync::Result;
use pastazzo_sync::daemon::fingerprint;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

#[derive(Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub created_at: u64,
    pub origin: String,
    pub kind: String,
    pub text: String,
    pub mime: String,
    pub data: B64,
    pub queued: bool,
    #[serde(default)]
    pub size: usize,
}

impl Entry {
    pub fn new(
        id: String,
        created_at: u64,
        origin: String,
        content: &Content,
        queued: bool,
    ) -> Self {
        let (kind, text, mime, data) = match content {
            Content::Text(text) => ("text", text.clone(), String::new(), Vec::new()),
            Content::Image { mime, data } => ("image", String::new(), mime.clone(), data.clone()),
            Content::ClearHistory => unreachable!("clear commands are not history entries"),
        };
        let size = text.len() + data.len();
        Self {
            id,
            created_at,
            origin,
            kind: kind.into(),
            text,
            mime,
            data: B64(data),
            queued,
            size,
        }
    }
    pub fn summary(&self) -> Value {
        json!({"id": self.id, "created_at": self.created_at, "origin": self.origin,
            "kind": self.kind, "preview": self.text.chars().take(400).collect::<String>(),
            "mime": self.mime, "size": self.size, "queued": self.queued})
    }
}

pub fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| e.to_string())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
}

pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    private_dir(path.parent().ok_or("invalid storage path")?)?;
    let temporary = path.with_extension(format!("{}.tmp", rand::random::<u64>()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

pub fn save(root: &Path, entry: &Entry) -> Result<()> {
    write(
        &root.join("history").join(format!("{}.json", entry.id)),
        &serde_json::to_vec(entry).map_err(|e| e.to_string())?,
    )
}

pub fn read(root: &Path, id: &str) -> Result<Entry> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("invalid item id".into());
    }
    let bytes =
        fs::read(root.join("history").join(format!("{id}.json"))).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn stored_entries(root: &Path) -> Result<Vec<(Entry, [u8; 32])>> {
    let dir = root.join("history");
    private_dir(&dir)?;
    let mut entries = Vec::new();
    for item in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            let mut entry = serde_json::from_slice::<Entry>(&bytes).map_err(|e| e.to_string())?;
            entry.size = entry.text.len() + entry.data.0.len();
            let content = match entry.kind.as_str() {
                "text" => Content::Text(std::mem::take(&mut entry.text)),
                "image" => Content::Image {
                    mime: entry.mime.clone(),
                    data: std::mem::take(&mut entry.data.0),
                },
                _ => return Err("invalid history item kind".into()),
            };
            let content_fingerprint = fingerprint(&content);
            if let Content::Text(text) = content {
                entry.text = text.chars().take(400).collect();
            }
            entries.push((entry, content_fingerprint));
        }
    }
    entries.sort_by(|(a, _), (b, _)| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(entries)
}

pub fn entries(root: &Path) -> Result<Vec<Entry>> {
    let mut visible: Vec<Entry> = Vec::new();
    let mut positions: HashMap<[u8; 32], usize> = HashMap::new();
    for (entry, content_fingerprint) in stored_entries(root)? {
        if let Some(&index) = positions.get(&content_fingerprint) {
            visible[index].queued |= entry.queued;
        } else {
            positions.insert(content_fingerprint, visible.len());
            visible.push(entry);
        }
    }
    Ok(visible)
}

pub fn prune(root: &Path) -> Result<()> {
    for (entry, _) in stored_entries(root)?.into_iter().skip(200) {
        if !entry.queued {
            fs::remove_file(root.join("history").join(format!("{}.json", entry.id)))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn clear(root: &Path) -> Result<()> {
    for (entry, _) in stored_entries(root)? {
        fs::remove_file(root.join("history").join(format!("{}.json", entry.id)))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("pastazzo-history-{}", rand::random::<u64>())))
        }
        fn save(&self, id: &str, at: u64, origin: &str, content: &Content, queued: bool) {
            save(
                &self.0,
                &Entry::new(id.into(), at, origin.into(), content, queued),
            )
            .unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn identical_images_keep_the_latest_origin_and_any_pending_upload() {
        let fixture = Fixture::new();
        let image = Content::Image {
            mime: "image/png".into(),
            data: vec![137, 80, 78, 71, 1],
        };
        fixture.save("older", 100, "iPhone", &image, true);
        fixture.save("newer", 200, "MacBook", &image, false);
        let items = entries(&fixture.0).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "newer");
        assert_eq!(items[0].origin, "MacBook");
        assert_eq!(items[0].created_at, 200);
        assert!(items[0].queued);
        assert_eq!(
            read(&fixture.0, "older").unwrap().data.0,
            vec![137, 80, 78, 71, 1]
        );
        assert!(!read(&fixture.0, "newer").unwrap().queued);
    }

    #[test]
    fn equal_previews_do_not_merge_different_text_or_image_payloads() {
        let fixture = Fixture::new();
        let prefix = "🍊".repeat(400);
        fixture.save(
            "text-a",
            1,
            "MacBook",
            &Content::Text(format!("{prefix} first ending")),
            false,
        );
        fixture.save(
            "text-a-copy",
            2,
            "Mac Pro",
            &Content::Text(format!("{prefix} first ending")),
            false,
        );
        fixture.save(
            "text-b",
            3,
            "MacBook",
            &Content::Text(format!("{prefix} second ending")),
            false,
        );
        for (id, mime, data) in [
            ("image-a", "image/png", vec![137, 80, 78, 71, 1]),
            ("image-a-copy", "image/png", vec![137, 80, 78, 71, 1]),
            ("image-b", "image/png", vec![137, 80, 78, 71, 2]),
            ("image-mime", "image/jpeg", vec![137, 80, 78, 71, 1]),
        ] {
            fixture.save(
                id,
                4,
                "MacBook",
                &Content::Image {
                    mime: mime.into(),
                    data,
                },
                false,
            );
        }
        let items = entries(&fixture.0).unwrap();
        assert_eq!(items.len(), 5);
        assert_eq!(items.iter().filter(|item| item.kind == "text").count(), 2);
        assert_eq!(items.iter().filter(|item| item.kind == "image").count(), 3);
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == "text-a-copy")
                .unwrap()
                .text,
            prefix
        );
        assert_eq!(
            entries(&fixture.0)
                .unwrap()
                .iter()
                .map(|item| &item.id)
                .collect::<Vec<_>>(),
            items.iter().map(|item| &item.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn clearing_history_removes_every_stored_copy() {
        let fixture = Fixture::new();
        let content = Content::Text("Repeated clip".into());
        fixture.save("old", 1, "MacBook", &content, false);
        fixture.save("new", 2, "Mac Pro", &content, false);
        assert_eq!(entries(&fixture.0).unwrap().len(), 1);
        clear(&fixture.0).unwrap();
        assert_eq!(fs::read_dir(fixture.0.join("history")).unwrap().count(), 0);
    }

    #[test]
    fn pruning_still_protects_hidden_pending_copies() {
        let fixture = Fixture::new();
        for n in 0..200 {
            fixture.save(
                &format!("clip-{n}"),
                n + 10,
                "MacBook",
                &Content::Text(format!("Clip {n}")),
                false,
            );
        }
        fixture.save(
            "pending",
            2,
            "iPhone",
            &Content::Text("Clip 199".into()),
            true,
        );
        fixture.save(
            "expired",
            1,
            "Mac Pro",
            &Content::Text("Clip 198".into()),
            false,
        );
        prune(&fixture.0).unwrap();
        assert!(read(&fixture.0, "pending").unwrap().queued);
        assert!(read(&fixture.0, "expired").is_err());
        assert_eq!(entries(&fixture.0).unwrap().len(), 200);
        assert!(
            entries(&fixture.0)
                .unwrap()
                .iter()
                .find(|item| item.id == "clip-199")
                .unwrap()
                .queued
        );
    }
}
