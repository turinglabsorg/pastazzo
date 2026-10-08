mod history;
mod keychain;

use pastazzo_core::item::{Content, ItemHeader, MAX_IMAGE_BYTES, MAX_TEXT_BYTES, SealedItem};
use pastazzo_core::{api::B64, display_fingerprint, random_id};
use pastazzo_sync::remote::Remote;
use pastazzo_sync::secrets::SecretBackend;
use pastazzo_sync::state::State;
use pastazzo_sync::{Result, account, now};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use std::ffi::{CString, c_char};
use std::path::Path;
use zeroize::{Zeroize, Zeroizing};

fn string<'a>(request: &'a Value, key: &str) -> Result<&'a str> {
    request
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}"))
}

fn upload_queued(root: &Path, state: &State, path: &Path) -> Result<()> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let sealed = SealedItem::from_bytes(&bytes).map_err(|e| e.to_string())?;
    if sealed.header.device != state.device.id() {
        return Err("queued item belongs to another device".into());
    }
    let content = sealed
        .open(&state.account_key, &state.account)
        .map_err(|e| e.to_string())?;
    let id = B64::encode(&sealed.header.id);
    let mut entry = history::read(root, &id).unwrap_or_else(|_| {
        history::Entry::new(
            id,
            sealed.header.created_at,
            state.device_name.clone(),
            &content,
            true,
        )
    });
    history::save(root, &entry)?;
    Remote::new(&state.server_url).post_item(state, &bytes)?;
    entry.queued = false;
    history::save(root, &entry)?;
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

fn flush_outbox(root: &Path, state: &State) -> Result<()> {
    let dir = outbox(root, state);
    history::private_dir(&dir)?;
    let mut paths = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .map(|entry| entry.map(|entry| entry.path()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>>>()?;
    paths.sort();
    for path in paths {
        if path.extension().is_some_and(|ext| ext == "sealed") {
            upload_queued(root, state, &path)?;
        }
    }
    Ok(())
}

fn outbox(root: &Path, state: &State) -> std::path::PathBuf {
    root.join("mobile-outbox")
        .join(B64::encode(&state.device.id()))
}

pub fn execute(request: &Value, keychain: &dyn SecretBackend) -> Result<Value> {
    let root = Path::new(string(request, "root")?);
    if !root.is_absolute() {
        return Err("storage must be an absolute path".into());
    }
    history::private_dir(root)?;
    let path = root.join("sync.json");
    match string(request, "operation")? {
        "history" => Ok(
            json!({"items": history::entries(root)?.iter().map(history::Entry::summary).collect::<Vec<_>>()}),
        ),
        "item" => Ok(json!({"item": history::read(root, string(request, "id")?)?})),
        "clear_local" => {
            match std::fs::remove_dir_all(root.join("mobile-outbox")) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
            history::clear(root)?;
            Ok(json!({}))
        }
        "status" => {
            if !path.exists() {
                return Ok(json!({"logged_in":false}));
            }
            let state = State::load_with(&path, keychain)?;
            Ok(
                json!({"logged_in":true, "device_name":state.device_name, "username":state.username,
                "server_url":state.server_url, "account_fingerprint":display_fingerprint(&state.account_key.fingerprint()),
                "server_fingerprint":display_fingerprint(&state.identity.fingerprint())}),
            )
        }
        "login" | "pair" => {
            if path.exists() {
                return Err("this device is already connected".into());
            }
            if !keychain.available() {
                return Err("a secure keychain is required".into());
            }
            let code_path = root.join("approval-code");
            let mut on_code = |code: &str| {
                let _ = history::write(&code_path, code.as_bytes());
            };
            let result = if string(request, "operation")? == "pair" {
                pastazzo_sync::pairing::join(
                    string(request, "link")?,
                    string(request, "name")?,
                    &mut on_code,
                )
            } else {
                let fingerprint = B64::decode(string(request, "fingerprint")?)
                    .and_then(|b| b.try_into().ok())
                    .ok_or("invalid server fingerprint")?;
                account::login(
                    string(request, "server")?,
                    &fingerprint,
                    string(request, "username")?,
                    string(request, "password")?,
                    string(request, "name")?,
                    &mut on_code,
                )
            };
            let _ = std::fs::remove_file(code_path);
            let mut state = result?;
            state.save_new_with(&path, keychain)?;
            Ok(json!({}))
        }
        "save" => {
            let content = if request.get("kind").and_then(Value::as_str) == Some("image") {
                let data = B64::decode(string(request, "data")?).ok_or("invalid image")?;
                if data.len() > MAX_IMAGE_BYTES {
                    return Err("images must be at most 25 MB".into());
                }
                Content::Image {
                    mime: string(request, "mime")?.into(),
                    data,
                }
            } else {
                let text = string(request, "text")?;
                if text.len() > MAX_TEXT_BYTES || text.trim().is_empty() {
                    return Err("text must be nonempty and at most 1 MB".into());
                }
                Content::Text(text.into())
            };
            let id = match request.get("id").and_then(Value::as_str) {
                Some(id) => B64::decode(id)
                    .and_then(|bytes| bytes.try_into().ok())
                    .ok_or("invalid item id")?,
                None => random_id(&mut OsRng),
            };
            let encoded = B64::encode(&id);
            if let Ok(existing) = history::read(root, &encoded) {
                let matches = match &content {
                    Content::Text(text) => existing.kind == "text" && existing.text == *text,
                    Content::Image { mime, data } => {
                        existing.kind == "image"
                            && existing.mime == *mime
                            && existing.data.0 == *data
                    }
                    Content::ClearHistory => false,
                };
                if !matches {
                    return Err("item id already belongs to different content".into());
                }
                return Ok(json!({"queued":existing.queued,"duplicate":true}));
            }
            let state = if path.exists() {
                Some(State::load_with(&path, keychain)?)
            } else {
                None
            };
            let mut entry = history::Entry::new(
                encoded.clone(),
                now(),
                string(request, "name")?.into(),
                &content,
                false,
            );
            let mut error = None;
            if let Some(state) = state {
                entry.origin = state.device_name.clone();
                entry.queued = true;
                let queued = outbox(root, &state).join(format!("{encoded}.sealed"));
                if !queued.exists() {
                    let sealed = SealedItem::seal(
                        &state.account_key,
                        &state.account,
                        ItemHeader {
                            id,
                            device: state.device.id(),
                            epoch: state.account_key.epoch(),
                            created_at: entry.created_at,
                        },
                        &content,
                        &mut OsRng,
                    )
                    .map_err(|e| e.to_string())?;
                    history::write(&queued, &sealed.to_bytes())?;
                }
                history::save(root, &entry)?;
                match upload_queued(root, &state, &queued) {
                    Ok(()) => entry.queued = false,
                    Err(reason) => error = Some(reason),
                }
            } else {
                history::save(root, &entry)?;
            }
            history::prune(root)?;
            Ok(json!({"queued":entry.queued, "sync_error":error}))
        }
        "refresh" => {
            let mut state = State::load_with(&path, keychain)?;
            flush_outbox(root, &state)?;
            let devices = account::devices(&state)?;
            let page = Remote::new(&state.server_url).items(&state, state.cursor, 0)?;
            for bytes in page.items {
                let sealed = SealedItem::from_bytes(&bytes.0).map_err(|e| e.to_string())?;
                if sealed.header.device == state.device.id() {
                    continue;
                }
                let content = sealed
                    .open(&state.account_key, &state.account)
                    .map_err(|e| e.to_string())?;
                if content == Content::ClearHistory {
                    history::clear(root)?;
                    continue;
                }
                let id = B64::encode(&sealed.header.id);
                if history::read(root, &id).is_ok() {
                    continue;
                }
                let origin = devices
                    .iter()
                    .find(|d| d.id == sealed.header.device)
                    .map(|d| d.name.clone())
                    .unwrap_or("Another device".into());
                history::save(
                    root,
                    &history::Entry::new(id, sealed.header.created_at, origin, &content, false),
                )?;
            }
            state.cursor = page.cursor;
            state.save(&path)?;
            history::prune(root)?;
            Ok(json!({}))
        }
        "logout" => {
            let state = State::load_with(&path, keychain)?;
            Remote::new(&state.server_url).revoke_device(&state, &state.device.id())?;
            state.forget_with(&path, keychain)?;
            Ok(json!({}))
        }
        _ => Err("unknown operation".into()),
    }
}

/// # Safety
/// `bytes` must point to `len` readable bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pastazzo_mobile_call(bytes: *const u8, len: usize) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if bytes.is_null() || len > 40 * 1024 * 1024 {
            return Err("invalid request".to_owned());
        }
        let input = Zeroizing::new(unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec());
        let mut request: Value =
            serde_json::from_slice(&input).map_err(|_| "invalid JSON request")?;
        let result = execute(&request, &keychain::MobileKeychain);
        if let Some(Value::String(password)) = request.get_mut("password") {
            password.zeroize();
        }
        result
    })
    .unwrap_or_else(|_| Err("the mobile client couldn't complete the operation".into()));
    let response = match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    CString::new(response.to_string())
        .expect("JSON has no raw NUL bytes")
        .into_raw()
}

/// # Safety
/// `pointer` must be an unfreed result from `pastazzo_mobile_call`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pastazzo_mobile_free(pointer: *mut c_char) {
    if !pointer.is_null() {
        let string = unsafe { CString::from_raw(pointer) };
        let mut bytes = string.into_bytes_with_nul();
        bytes.zeroize();
    }
}
