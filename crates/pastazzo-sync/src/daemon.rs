//! The sync loop: one thread sends local copies, another long-polls the
//! server for items from the other devices. Both report what they're doing
//! in `<data dir>/sync/`, which the GNOME extension and the Mac app read:
//!
//! - `status.json`: this device, its account and server;
//! - `transfers.json`: uploads and downloads in progress, for a progress
//!   indicator on both sides of a transfer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pastazzo_core::api::B64;
use pastazzo_core::item::{Content, ItemHeader, SealedItem};
use pastazzo_core::request::MAX_CLOCK_SKEW_MS;
use pastazzo_core::{Id, api, random_id};
use rand::rngs::OsRng;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::clipboard::Clipboard;
use crate::remote::Remote;
use crate::state::State;
use crate::{Result, account, log, now};

/// How often local copies are looked for.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// After an item is received and applied, the same content showing up as a
/// "local copy" within this long is our own echo, not a new copy.
const ECHO_WINDOW: Duration = Duration::from_secs(15);
/// Item ids remembered to ignore duplicates and replays.
const SEEN_IDS: usize = 10_000;
/// Items at least this big are announced before they're uploaded, so the
/// other devices can show them coming, and report progress.
const ANNOUNCE_BYTES: usize = 64 * 1024;
/// An unknown device makes the names be fetched again, at most this often.
const NAMES_REFRESH: Duration = Duration::from_secs(30);
/// `transfers.json` is rewritten at most this often while things move.
const STATUS_INTERVAL: Duration = Duration::from_millis(150);

/// A content hash, local to this device: never sent anywhere.
pub fn fingerprint(content: &Content) -> [u8; 32] {
    let mut hash = Sha256::new();
    match content {
        Content::Text(text) => {
            hash.update([1]);
            hash.update(text.as_bytes());
        }
        Content::Image { mime, data } => {
            hash.update([2]);
            hash.update((mime.len() as u32).to_be_bytes());
            hash.update(mime.as_bytes());
            hash.update(data);
        }
        Content::ClearHistory => hash.update([3]),
    }
    hash.finalize().into()
}

#[derive(Default)]
struct Shared {
    /// Content received recently, by fingerprint.
    received: HashMap<[u8; 32], Instant>,
    seen: HashSet<Id>,
    seen_order: VecDeque<Id>,
    /// `created_at` of the last item put on the clipboard.
    last_applied: u64,
}

impl Shared {
    fn is_echo(&mut self, content: &Content) -> bool {
        self.received.retain(|_, at| at.elapsed() < ECHO_WINDOW);
        self.received.contains_key(&fingerprint(content))
    }

    /// Records an item id; false if it was already seen.
    fn first_sight(&mut self, id: Id) -> bool {
        if !self.seen.insert(id) {
            return false;
        }
        self.seen_order.push_back(id);
        if self.seen_order.len() > SEEN_IDS
            && let Some(old) = self.seen_order.pop_front()
        {
            self.seen.remove(&old);
        }
        true
    }
}

/// A transfer in progress, as the UIs show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Transfer {
    /// `send` or `receive`.
    pub direction: &'static str,
    /// For a receive, the sending device's name.
    pub device: String,
    pub size: u64,
    pub done: u64,
}

#[derive(Default)]
struct Status {
    /// Where to write the status files; `None` for one-off commands and tests.
    dir: Option<PathBuf>,
    sending: Option<Transfer>,
    incoming: Vec<Transfer>,
    downloading: Option<Transfer>,
    published: Vec<Transfer>,
    published_at: Option<Instant>,
}

impl Status {
    fn current(&self) -> Vec<Transfer> {
        self.sending
            .iter()
            .chain(&self.incoming)
            .chain(&self.downloading)
            .cloned()
            .collect()
    }

    /// Writes `transfers.json` if something changed. Starts and ends are
    /// written right away; progress at most every [`STATUS_INTERVAL`].
    fn publish(&mut self) {
        let current = self.current();
        if current == self.published {
            return;
        }
        let edge = current.is_empty() || current.len() != self.published.len();
        if !edge
            && self
                .published_at
                .is_some_and(|at| at.elapsed() < STATUS_INTERVAL)
        {
            return;
        }
        self.write(current);
    }

    fn write(&mut self, current: Vec<Transfer>) {
        let Some(dir) = &self.dir else { return };
        #[derive(Serialize)]
        struct File<'a> {
            updated: u64,
            transfers: &'a [Transfer],
        }
        let file = File {
            updated: now(),
            transfers: &current,
        };
        if let Err(error) = write_json(dir, "transfers.json", &file) {
            log!("couldn't write the transfer status: {error}");
        }
        self.published = current;
        self.published_at = Some(Instant::now());
    }
}

/// Writes a JSON file atomically, so readers never see half of it.
fn write_json(dir: &Path, name: &str, value: &impl Serialize) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{name}.tmp"));
    fs::write(&tmp, serde_json::to_vec(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    fs::rename(&tmp, dir.join(name)).map_err(|e| e.to_string())
}

#[derive(Default)]
struct Names {
    by_id: HashMap<Id, String>,
    fetched: Option<Instant>,
}

#[derive(Clone)]
pub struct Daemon {
    state: Arc<State>,
    state_path: PathBuf,
    cursor: Arc<Mutex<u64>>,
    pending_version: Arc<Mutex<u64>>,
    clipboard: Arc<Mutex<Box<dyn Clipboard>>>,
    shared: Arc<Mutex<Shared>>,
    remote: Arc<Remote>,
    status: Arc<Mutex<Status>>,
    names: Arc<Mutex<Names>>,
    /// The device whose upload was last seen coming, for labelling the download.
    last_sender: Arc<Mutex<String>>,
}

impl Daemon {
    pub fn new(state: State, state_path: &Path, clipboard: Box<dyn Clipboard>) -> Self {
        Self {
            remote: Arc::new(Remote::new(&state.server_url)),
            cursor: Arc::new(Mutex::new(state.cursor)),
            state: Arc::new(state),
            state_path: state_path.to_owned(),
            pending_version: Arc::default(),
            clipboard: Arc::new(Mutex::new(clipboard)),
            shared: Arc::default(),
            status: Arc::default(),
            names: Arc::default(),
            last_sender: Arc::default(),
        }
    }

    /// Reports this device and its transfers in `dir` for the UIs.
    pub fn with_status_dir(self, dir: PathBuf) -> Result<Self> {
        #[derive(Serialize)]
        struct Info<'a> {
            username: &'a str,
            device_name: &'a str,
            device_id: String,
            server_url: &'a str,
        }
        write_json(
            &dir,
            "status.json",
            &Info {
                username: &self.state.username,
                device_name: &self.state.device_name,
                device_id: B64::encode(&self.state.device.id()),
                server_url: &self.state.server_url,
            },
        )?;
        {
            let mut status = self.status.lock().unwrap();
            status.dir = Some(dir);
            // Nothing from a previous run is still moving.
            status.write(Vec::new());
        }
        Ok(self)
    }

    /// Runs until the process is stopped.
    pub fn run(self) -> Result<()> {
        log!(
            "syncing as {} ({}) with {}",
            self.state.username,
            self.state.device_name,
            self.state.server_url
        );
        let receiver = {
            let daemon = self.clone();
            thread::spawn(move || daemon.receive_forever())
        };
        loop {
            if receiver.is_finished() {
                return Err("the receiving thread stopped".to_owned());
            }
            self.send_new_copies();
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// Sends every local copy made since the last call, except echoes of
    /// what was just received.
    pub fn send_new_copies(&self) {
        let copies = self.clipboard.lock().unwrap().poll();
        for content in copies {
            if self.shared.lock().unwrap().is_echo(&content) {
                continue;
            }
            if let Err(error) = self.send(&content) {
                log!("couldn't send a copy: {error}");
            }
        }
    }

    fn set_sending(&self, transfer: Option<Transfer>) {
        let mut status = self.status.lock().unwrap();
        status.sending = transfer;
        status.publish();
    }

    /// Encrypts and uploads one item, retrying a few times. Big items are
    /// announced first and report their progress.
    pub fn send(&self, content: &Content) -> Result<()> {
        let state = &self.state;
        let header = ItemHeader {
            id: random_id(&mut OsRng),
            device: state.device.id(),
            epoch: state.account_key.epoch(),
            created_at: now(),
        };
        let sealed = SealedItem::seal(
            &state.account_key,
            &state.account,
            header,
            content,
            &mut OsRng,
        )
        .map_err(|e| e.to_string())?
        .to_bytes();
        self.shared.lock().unwrap().first_sight(header.id);

        let big = sealed.len() >= ANNOUNCE_BYTES;
        if big {
            if let Err(error) = self.remote.announce(state, &header.id, sealed.len() as u64) {
                log!("couldn't announce a big item ({error}), sending it anyway");
            }
            self.set_sending(Some(Transfer {
                direction: "send",
                device: String::new(),
                size: sealed.len() as u64,
                done: 0,
            }));
        }
        let mut delay = Duration::from_secs(1);
        let mut result = Err(String::new());
        for attempt in 1..=3 {
            result = if big {
                self.remote.upload_item(state, &sealed, &mut |done, total| {
                    self.set_sending(Some(Transfer {
                        direction: "send",
                        device: String::new(),
                        size: total,
                        done,
                    }));
                })
            } else {
                self.remote.post_item(state, &sealed)
            };
            match &result {
                Ok(_) => break,
                Err(error) if attempt < 3 => log!("upload failed ({error}), retrying"),
                Err(_) => {}
            }
            thread::sleep(delay);
            delay *= 2;
        }
        if big {
            self.set_sending(None);
        }
        result.map(|_| ())
    }

    fn receive_forever(self) {
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.receive_once(api::MAX_WAIT_SECONDS - 5) {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(error) => {
                    log!("{error}; retrying in {}s", backoff.as_secs());
                    thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
    }

    /// The name another device gave itself, from its encrypted record.
    pub fn device_name(&self, id: &Id) -> String {
        let stale = {
            let names = self.names.lock().unwrap();
            if let Some(name) = names.by_id.get(id) {
                return name.clone();
            }
            names.fetched.is_none_or(|at| at.elapsed() >= NAMES_REFRESH)
        };
        if stale {
            self.names.lock().unwrap().fetched = Some(Instant::now());
            match account::devices(&self.state) {
                Ok(devices) => {
                    let mut names = self.names.lock().unwrap();
                    for device in devices {
                        names.by_id.insert(device.id, device.name);
                    }
                }
                Err(error) => log!("couldn't fetch the device names: {error}"),
            }
        }
        self.names
            .lock()
            .unwrap()
            .by_id
            .get(id)
            .cloned()
            .unwrap_or_else(|| "another device".to_owned())
    }

    /// One long poll: waits up to `wait` seconds for new items, or for
    /// progress of other devices' uploads, and handles what comes.
    pub fn receive_once(&self, wait: u64) -> Result<()> {
        let cursor = *self.cursor.lock().unwrap();
        let pending_version = *self.pending_version.lock().unwrap();
        let sender = self.last_sender.lock().unwrap().clone();
        let page = self.remote.items_with_progress(
            &self.state,
            cursor,
            wait,
            Some(pending_version),
            &mut |done, total| {
                if total >= ANNOUNCE_BYTES as u64 {
                    let mut status = self.status.lock().unwrap();
                    status.downloading = Some(Transfer {
                        direction: "receive",
                        device: sender.clone(),
                        size: total,
                        done,
                    });
                    status.publish();
                }
            },
        );
        {
            let mut status = self.status.lock().unwrap();
            status.downloading = None;
            status.publish();
        }
        let page = page?;
        *self.pending_version.lock().unwrap() = page.pending_version;

        let own = self.state.device.id();
        let incoming: Vec<Transfer> = page
            .pending
            .iter()
            .filter_map(|p| {
                let device: Id = p.device.array()?;
                (device != own).then(|| Transfer {
                    direction: "receive",
                    device: self.device_name(&device),
                    size: p.size,
                    done: p.received,
                })
            })
            .collect();
        if let Some(last) = incoming.last() {
            *self.last_sender.lock().unwrap() = last.device.clone();
        }
        {
            let mut status = self.status.lock().unwrap();
            status.incoming = incoming;
            status.publish();
        }

        for bytes in &page.items {
            if let Err(error) = self.handle(&bytes.0) {
                log!("skipped an item: {error}");
            }
        }
        if page.cursor != cursor {
            *self.cursor.lock().unwrap() = page.cursor;
            self.state.save_with_cursor(&self.state_path, page.cursor)?;
        }
        Ok(())
    }

    fn handle(&self, bytes: &[u8]) -> Result<()> {
        let sealed = SealedItem::from_bytes(bytes).map_err(|e| e.to_string())?;
        if sealed.header.device == self.state.device.id() {
            return Ok(());
        }
        let content = sealed
            .open(&self.state.account_key, &self.state.account)
            .map_err(|e| format!("can't open it ({e})"))?;
        if !self.shared.lock().unwrap().first_sight(sealed.header.id) {
            return Ok(());
        }
        let origin = self.device_name(&sealed.header.device);
        if content == Content::ClearHistory {
            log!("{origin} cleared the history on every device");
            return self.clipboard.lock().unwrap().clear_history();
        }
        let apply = {
            let mut shared = self.shared.lock().unwrap();
            shared
                .received
                .insert(fingerprint(&content), Instant::now());
            let created_at = sealed.header.created_at;
            let fresh = created_at > shared.last_applied && created_at <= now() + MAX_CLOCK_SKEW_MS;
            if fresh {
                shared.last_applied = created_at;
            }
            fresh
        };
        let mut clipboard = self.clipboard.lock().unwrap();
        if apply {
            clipboard.apply(&content, &origin)
        } else {
            clipboard.remember(&content, &origin)
        }
    }
}
