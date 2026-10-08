use pastazzo_core::api::B64;
use pastazzo_core::item::Content;
use pastazzo_sync::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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

pub fn entries(root: &Path) -> Result<Vec<Entry>> {
    let dir = root.join("history");
    private_dir(&dir)?;
    let mut entries = Vec::new();
    for item in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            let mut entry = serde_json::from_slice::<Entry>(&bytes).map_err(|e| e.to_string())?;
            entry.size = entry.text.len() + entry.data.0.len();
            entry.text = entry.text.chars().take(400).collect();
            entry.data.0 = Vec::new();
            entries.push(entry);
        }
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.created_at));
    Ok(entries)
}

pub fn prune(root: &Path) -> Result<()> {
    for entry in entries(root)?.into_iter().skip(200) {
        if !entry.queued {
            fs::remove_file(root.join("history").join(format!("{}.json", entry.id)))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn clear(root: &Path) -> Result<()> {
    for entry in entries(root)? {
        fs::remove_file(root.join("history").join(format!("{}.json", entry.id)))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
