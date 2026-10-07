//! Where local copies come from and where received items go.
//!
//! - Linux (GNOME): background programs can't touch the Wayland clipboard,
//!   but the pastazzo GNOME extension can. It already saves every copy to
//!   the pastazzo history, so new local copies are new files there. Received
//!   items are added to the history with the `pastazzo` CLI and dropped in
//!   an inbox the extension puts on the clipboard.
//! - macOS: `pbpaste` and `pbcopy` for text, AppleScript for received PNGs.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use pastazzo_core::item::{Content, MAX_IMAGE_BYTES, MAX_TEXT_BYTES};

use crate::{Result, log};

pub trait Clipboard: Send {
    /// New local copies since the last call, oldest first.
    fn poll(&mut self) -> Vec<Content>;
    /// Puts a received item on the clipboard, and in the history if there is one.
    fn apply(&mut self, content: &Content) -> Result<()>;
    /// Only keeps a received item in the history, if there is one.
    fn remember(&mut self, content: &Content) -> Result<()>;
}

pub fn platform() -> Result<Box<dyn Clipboard>> {
    if cfg!(target_os = "macos") {
        Ok(Box::new(Pasteboard::new()))
    } else {
        Ok(Box::new(PastazzoStore::new()?))
    }
}

fn image_extension(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => return None,
    })
}

fn image_mime(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        _ => return None,
    })
}

/// The pastazzo history and the GNOME extension's inbox.
pub struct PastazzoStore {
    items: PathBuf,
    inbox: PathBuf,
    cli: PathBuf,
    known: HashSet<OsString>,
}

impl PastazzoStore {
    pub fn new() -> Result<Self> {
        let data = match std::env::var("XDG_DATA_HOME") {
            Ok(dir) => PathBuf::from(dir),
            Err(_) => PathBuf::from(std::env::var("HOME").map_err(|_| "HOME is not set")?)
                .join(".local/share"),
        };
        let root = data.join("pastazzo");
        // The pastazzo CLI next to this binary, else on the PATH.
        let cli = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("pastazzo")))
            .filter(|cli| cli.exists())
            .unwrap_or_else(|| PathBuf::from("pastazzo"));
        Ok(Self::at(root.join("items"), root.join("inbox"), cli))
    }

    pub fn at(items: PathBuf, inbox: PathBuf, cli: PathBuf) -> Self {
        let mut store = Self {
            items,
            inbox,
            cli,
            known: HashSet::new(),
        };
        // What's already in the history was copied before we started.
        store.known = store.listing();
        store
    }

    fn listing(&self) -> HashSet<OsString> {
        fs::read_dir(&self.items)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn read(path: &Path) -> Option<Content> {
        let extension = path.extension()?.to_str()?;
        let size = fs::metadata(path).ok()?.len() as usize;
        if extension == "txt" {
            if size > MAX_TEXT_BYTES {
                return None;
            }
            return fs::read_to_string(path).ok().map(Content::Text);
        }
        let mime = image_mime(extension)?;
        if size > MAX_IMAGE_BYTES {
            return None;
        }
        Some(Content::Image {
            mime: mime.to_owned(),
            data: fs::read(path).ok()?,
        })
    }

    fn add_to_history(&self, content: &Content) -> Result<()> {
        let (args, input): (Vec<&str>, &[u8]) = match content {
            Content::Text(text) => (vec!["add"], text.as_bytes()),
            Content::Image { mime, data } => (vec!["add-image", mime], data),
        };
        let mut child = Command::new(&self.cli)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("run {}: {e}", self.cli.display()))?;
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(input)
            .map_err(|e| e.to_string())?;
        let status = child.wait().map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{} {} failed", self.cli.display(), args[0]))
        }
    }

    /// Drops the item where the extension picks it up and puts it on the
    /// clipboard. Written under a temporary name first, so the extension
    /// never reads a half-written file.
    fn send_to_extension(&self, content: &Content) -> Result<()> {
        fs::create_dir_all(&self.inbox)
            .map_err(|e| format!("create {}: {e}", self.inbox.display()))?;
        self.drop_stale_inbox_files();
        let (extension, bytes): (&str, &[u8]) = match content {
            Content::Text(text) => ("txt", text.as_bytes()),
            Content::Image { mime, data } => (image_extension(mime).unwrap_or("png"), data),
        };
        let name = format!("{:020}-{:08x}", crate::now(), rand::random::<u32>());
        let tmp = self.inbox.join(format!(".{name}.tmp"));
        fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        fs::rename(&tmp, self.inbox.join(format!("{name}.{extension}"))).map_err(|e| e.to_string())
    }

    /// If the extension isn't running, don't let the inbox pile up.
    fn drop_stale_inbox_files(&self) {
        let Ok(entries) = fs::read_dir(&self.inbox) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > 60);
            if stale {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

impl Clipboard for PastazzoStore {
    fn poll(&mut self) -> Vec<Content> {
        let listing = self.listing();
        let mut new: Vec<&OsString> = listing
            .iter()
            .filter(|name| !self.known.contains(*name))
            .collect();
        // File names start with a timestamp.
        new.sort();
        let contents = new
            .into_iter()
            .filter_map(|name| Self::read(&self.items.join(name)))
            .collect();
        self.known = listing;
        contents
    }

    fn apply(&mut self, content: &Content) -> Result<()> {
        self.add_to_history(content)?;
        self.send_to_extension(content)
    }

    fn remember(&mut self, content: &Content) -> Result<()> {
        self.add_to_history(content)
    }
}

/// The macOS pasteboard.
pub struct Pasteboard {
    last_text: Option<String>,
}

impl Pasteboard {
    pub fn new() -> Self {
        // Whatever is on the clipboard now was copied before we started.
        Self {
            last_text: Self::paste(),
        }
    }

    /// pbcopy and pbpaste pick the encoding from the locale, which launchd
    /// doesn't set: without this, anything beyond ASCII gets mangled.
    fn command(program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8");
        command
    }

    fn paste() -> Option<String> {
        let output = Self::command("pbpaste")
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() || output.stdout.len() > MAX_TEXT_BYTES {
            return None;
        }
        String::from_utf8(output.stdout).ok()
    }

    fn copy(text: &str) -> Result<()> {
        let mut child = Self::command("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("run pbcopy: {e}"))?;
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string())?;
        child.wait().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Images go through a private temporary file and AppleScript.
    fn copy_png(data: &[u8]) -> Result<()> {
        let dir = std::env::temp_dir().join(format!("pastazzo-{}", rand::random::<u64>()));
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("clip.png");
        let result = fs::write(&path, data)
            .map_err(|e| e.to_string())
            .and_then(|_| {
                let script = format!(
                    "set the clipboard to (read (POSIX file \"{}\") as «class PNGf»)",
                    path.display()
                );
                let status = Command::new("osascript")
                    .arg("-e")
                    .arg(script)
                    .status()
                    .map_err(|e| e.to_string())?;
                if status.success() {
                    Ok(())
                } else {
                    Err("osascript couldn't set the clipboard".to_owned())
                }
            });
        let _ = fs::remove_dir_all(&dir);
        result
    }
}

impl Default for Pasteboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard for Pasteboard {
    fn poll(&mut self) -> Vec<Content> {
        let Some(text) = Self::paste() else {
            return Vec::new();
        };
        if text.is_empty() || self.last_text.as_deref() == Some(&text) {
            return Vec::new();
        }
        self.last_text = Some(text.clone());
        vec![Content::Text(text)]
    }

    fn apply(&mut self, content: &Content) -> Result<()> {
        match content {
            Content::Text(text) => {
                Self::copy(text)?;
                self.last_text = Some(text.clone());
                Ok(())
            }
            Content::Image { mime, data } if mime == "image/png" => Self::copy_png(data),
            Content::Image { mime, .. } => {
                log!(
                    "received a {mime} image: only PNG images can go on the macOS clipboard for now"
                );
                Ok(())
            }
        }
    }

    fn remember(&mut self, _content: &Content) -> Result<()> {
        Ok(())
    }
}
