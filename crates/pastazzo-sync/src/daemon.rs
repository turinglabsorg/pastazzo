//! The sync loop: one thread sends local copies, another long-polls the
//! server for items from the other devices.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pastazzo_core::item::{Content, ItemHeader, SealedItem};
use pastazzo_core::request::MAX_CLOCK_SKEW_MS;
use pastazzo_core::{Id, api, random_id};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

use crate::clipboard::Clipboard;
use crate::remote::Remote;
use crate::state::State;
use crate::{Result, log, now};

/// How often local copies are looked for.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// After an item is received and applied, the same content showing up as a
/// "local copy" within this long is our own echo, not a new copy.
const ECHO_WINDOW: Duration = Duration::from_secs(15);
/// Item ids remembered to ignore duplicates and replays.
const SEEN_IDS: usize = 10_000;

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

#[derive(Clone)]
pub struct Daemon {
    state: Arc<State>,
    state_path: PathBuf,
    cursor: Arc<Mutex<u64>>,
    clipboard: Arc<Mutex<Box<dyn Clipboard>>>,
    shared: Arc<Mutex<Shared>>,
    remote: Arc<Remote>,
}

impl Daemon {
    pub fn new(state: State, state_path: &Path, clipboard: Box<dyn Clipboard>) -> Self {
        Self {
            remote: Arc::new(Remote::new(&state.server_url)),
            cursor: Arc::new(Mutex::new(state.cursor)),
            state: Arc::new(state),
            state_path: state_path.to_owned(),
            clipboard: Arc::new(Mutex::new(clipboard)),
            shared: Arc::default(),
        }
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

    /// Encrypts and uploads one item, retrying a few times.
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
        let mut delay = Duration::from_secs(1);
        for attempt in 1..=3 {
            match self.remote.post_item(state, &sealed) {
                Ok(_) => return Ok(()),
                Err(error) if attempt == 3 => return Err(error),
                Err(error) => log!("upload failed ({error}), retrying"),
            }
            thread::sleep(delay);
            delay *= 2;
        }
        unreachable!()
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

    /// One long poll: waits up to `wait` seconds for new items and handles them.
    pub fn receive_once(&self, wait: u64) -> Result<()> {
        let cursor = *self.cursor.lock().unwrap();
        let page = self.remote.items(&self.state, cursor, wait)?;
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
        let apply = {
            let mut shared = self.shared.lock().unwrap();
            if !shared.first_sight(sealed.header.id) {
                return Ok(());
            }
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
            clipboard.apply(&content)
        } else {
            clipboard.remember(&content)
        }
    }
}
