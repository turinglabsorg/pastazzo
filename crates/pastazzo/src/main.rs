use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ITEMS: usize = 250;
const MAX_TEXT_BYTES: usize = 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;

#[derive(Debug, Clone)]
struct Item {
    id: String,
    timestamp: u128,
    kind: String,
    mime: String,
    text: String,
    path: String,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("pastazzo: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "search".to_string());

    match command.as_str() {
        "add" => add_from_stdin(),
        "add-image" => {
            let mime = args.next().unwrap_or_else(|| "image/png".to_string());
            add_image_from_stdin(&mime)
        }
        "search" => {
            let query = args.collect::<Vec<_>>().join(" ");
            let items = search_items(&query)?;
            print_items_json(&items)
        }
        "get" => {
            let id = args.next().ok_or("missing item id")?;
            let path = item_path(&id)?;
            let text = fs::read_to_string(path).map_err(|err| format!("read item: {err}"))?;
            print!("{text}");
            Ok(())
        }
        "touch" => {
            let id = args.next().ok_or("missing item id")?;
            let new_id = touch_item(&id)?;
            println!("{new_id}");
            Ok(())
        }
        "clear" => {
            let dir = items_dir()?;
            if dir.exists() {
                fs::remove_dir_all(&dir).map_err(|err| format!("clear history: {err}"))?;
            }
            Ok(())
        }
        "path" => {
            println!("{}", items_dir()?.display());
            Ok(())
        }
        _ => Err(format!("unknown command: {command}")),
    }
}

fn add_from_stdin() -> Result<(), String> {
    let mut bytes = Vec::new();
    io::stdin()
        .take((MAX_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("read stdin: {err}"))?;

    if bytes.len() > MAX_TEXT_BYTES {
        bytes.truncate(MAX_TEXT_BYTES);
    }

    let text = String::from_utf8_lossy(&bytes).to_string();
    if text.trim().is_empty() {
        return Ok(());
    }

    let dir = items_dir()?;
    fs::create_dir_all(&dir).map_err(|err| format!("create history dir: {err}"))?;

    let hash = fnv1a64(text.as_bytes());
    remove_existing_hash(&dir, hash)?;

    let timestamp = now_millis()?;
    let id = format!("{timestamp:020}-{hash:016x}");
    let path = dir.join(format!("{id}.txt"));

    let mut file = fs::File::create(&path).map_err(|err| format!("create item: {err}"))?;
    file.write_all(text.as_bytes())
        .map_err(|err| format!("write item: {err}"))?;

    prune_history(&dir)?;
    println!("{id}");
    Ok(())
}

fn add_image_from_stdin(mime: &str) -> Result<(), String> {
    let mut bytes = Vec::new();
    io::stdin()
        .take((MAX_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("read stdin: {err}"))?;

    if bytes.is_empty() {
        return Ok(());
    }

    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!("image is larger than {MAX_IMAGE_BYTES} bytes"));
    }

    let dir = items_dir()?;
    fs::create_dir_all(&dir).map_err(|err| format!("create history dir: {err}"))?;

    let hash = fnv1a64(&bytes);
    remove_existing_hash(&dir, hash)?;

    let timestamp = now_millis()?;
    let id = format!("{timestamp:020}-{hash:016x}");
    let extension = image_extension(mime);
    let path = dir.join(format!("{id}.{extension}"));

    let mut file = fs::File::create(&path).map_err(|err| format!("create image item: {err}"))?;
    file.write_all(&bytes)
        .map_err(|err| format!("write image item: {err}"))?;

    prune_history(&dir)?;
    println!("{id}");
    Ok(())
}

fn search_items(query: &str) -> Result<Vec<Item>, String> {
    let query = query.trim().to_lowercase();
    let terms = query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    let mut scored = read_items()?
        .into_iter()
        .filter_map(|item| {
            let haystack = searchable_text(&item).to_lowercase();
            let score = score(&haystack, &terms)?;
            Some((score, item))
        })
        .collect::<Vec<_>>();

    scored.sort_by(|(score_a, item_a), (score_b, item_b)| {
        score_b
            .cmp(score_a)
            .then_with(|| item_b.timestamp.cmp(&item_a.timestamp))
    });

    Ok(scored.into_iter().map(|(_, item)| item).take(20).collect())
}

fn score(haystack: &str, terms: &[String]) -> Option<i32> {
    if terms.is_empty() {
        return Some(0);
    }

    let mut score = 0;
    for term in terms {
        let pos = haystack.find(term)?;
        score += 1000;
        score -= pos.min(300) as i32;
        if pos == 0 {
            score += 200;
        }
    }

    Some(score)
}

fn read_items() -> Result<Vec<Item>, String> {
    let dir = items_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|err| format!("read history dir: {err}"))?;

    for entry in entries {
        let entry = entry.map_err(|err| format!("read history entry: {err}"))?;
        let path = entry.path();
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default();
        if extension != "txt" && mime_from_image_extension(extension).is_none() {
            continue;
        }

        let id = match path.file_stem().and_then(|name| name.to_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        let timestamp = match id.split_once('-').and_then(|(ts, _)| ts.parse::<u128>().ok()) {
            Some(timestamp) => timestamp,
            None => continue,
        };

        if extension == "txt" {
            let text = fs::read_to_string(&path).unwrap_or_default();
            if !text.trim().is_empty() {
                items.push(Item {
                    id,
                    timestamp,
                    kind: "text".to_string(),
                    mime: "text/plain".to_string(),
                    text,
                    path: path.to_string_lossy().to_string(),
                });
            }
        } else if let Some(mime) = mime_from_image_extension(extension) {
            items.push(Item {
                id,
                timestamp,
                kind: "image".to_string(),
                mime: mime.to_string(),
                text: String::new(),
                path: path.to_string_lossy().to_string(),
            });
        }
    }

    items.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(items)
}

fn print_items_json(items: &[Item]) -> Result<(), String> {
    print!("[");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            print!(",");
        }

        print!(
            "{{\"id\":\"{}\",\"timestamp\":{},\"kind\":\"{}\",\"mime\":\"{}\",\"preview\":\"{}\",\"text\":\"{}\",\"path\":\"{}\"}}",
            json_escape(&item.id),
            item.timestamp,
            json_escape(&item.kind),
            json_escape(&item.mime),
            json_escape(&item_preview(item)),
            json_escape(&item.text),
            json_escape(&item.path),
        );
    }
    println!("]");
    Ok(())
}

fn preview(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_space = false;

    for ch in text.chars() {
        let ch = if ch.is_whitespace() { ' ' } else { ch };
        if ch == ' ' {
            if last_was_space {
                continue;
            }
            last_was_space = true;
        } else {
            last_was_space = false;
        }

        if out.chars().count() >= 180 {
            out.push_str("...");
            break;
        }
        out.push(ch);
    }

    out.trim().to_string()
}

fn item_preview(item: &Item) -> String {
    if item.kind == "image" {
        return format!("{} image", item.mime.strip_prefix("image/").unwrap_or("clipboard"));
    }

    preview(&item.text)
}

fn searchable_text(item: &Item) -> String {
    if item.kind == "image" {
        return format!("image {} {}", item.mime, item.id);
    }

    item.text.clone()
}

fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

fn remove_existing_hash(dir: &Path, hash: u64) -> Result<(), String> {
    let suffix = format!("-{hash:016x}.");
    for entry in fs::read_dir(dir).map_err(|err| format!("read history dir: {err}"))? {
        let path = entry
            .map_err(|err| format!("read history entry: {err}"))?
            .path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(&suffix))
        {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

fn prune_history(dir: &Path) -> Result<(), String> {
    let mut files = fs::read_dir(dir)
        .map_err(|err| format!("read history dir: {err}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default();
            extension == "txt" || mime_from_image_extension(extension).is_some()
        })
        .collect::<Vec<_>>();

    files.sort_by(|a, b| {
        let a = a.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let b = b.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        b.cmp(a)
    });

    for path in files.into_iter().skip(MAX_ITEMS) {
        let _ = fs::remove_file(path);
    }

    Ok(())
}

fn item_path(id: &str) -> Result<PathBuf, String> {
    if !id
        .chars()
        .all(|ch| ch.is_ascii_hexdigit() || ch == '-')
    {
        return Err("invalid item id".to_string());
    }

    Ok(items_dir()?.join(format!("{id}.txt")))
}

fn touch_item(id: &str) -> Result<String, String> {
    if !id
        .chars()
        .all(|ch| ch.is_ascii_hexdigit() || ch == '-')
    {
        return Err("invalid item id".to_string());
    }

    let dir = items_dir()?;
    let current_path = fs::read_dir(&dir)
        .map_err(|err| format!("read history dir: {err}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.file_stem().and_then(|name| name.to_str()) == Some(id))
        .ok_or("item not found")?;

    let (_, hash) = id.split_once('-').ok_or("invalid item id")?;
    let extension = current_path
        .extension()
        .and_then(|ext| ext.to_str())
        .ok_or("item has no extension")?;
    let timestamp = now_millis()?;
    let new_id = format!("{timestamp:020}-{hash}");
    let new_path = dir.join(format!("{new_id}.{extension}"));

    fs::rename(&current_path, &new_path).map_err(|err| format!("touch item: {err}"))?;
    prune_history(&dir)?;
    Ok(new_id)
}

fn items_dir() -> Result<PathBuf, String> {
    if let Ok(data_home) = env::var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(data_home).join("pastazzo").join("items"));
    }

    let home = env::var("HOME").map_err(|_| "HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("pastazzo")
        .join("items"))
}

fn now_millis() -> Result<u128, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system clock before epoch: {err}"))?
        .as_millis())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn image_extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => "png",
    }
}

fn mime_from_image_extension(extension: &str) -> Option<&'static str> {
    match extension {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        _ => None,
    }
}
