//! Private local delivery ledger and durable encrypted acknowledgement outbox.
use crate::remote::Remote;
use crate::state::State;
use crate::{Result, now};
use pastazzo_core::account::AccountKey;
use pastazzo_core::api::B64;
use pastazzo_core::device::{DevicePublic, SealedDeviceRecord};
use pastazzo_core::item::{Content, ItemHeader};
use pastazzo_core::receipt::SealedReceipt;
use pastazzo_core::{Id, random_id};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receiver {
    pub device_id: String,
    pub device_name: String,
    pub received_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delivery {
    pub item_id: String,
    pub fingerprint: String,
    pub origin_device: String,
    pub origin_name: String,
    pub created_at: u64,
    pub uploaded_at: Option<u64>,
    pub receivers: Vec<Receiver>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Peer {
    pub device_id: String,
    pub device_name: String,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub cursor: u64,
    pub supported: bool,
    pub peers: Vec<Peer>,
    pub items: Vec<Delivery>,
}

pub struct Tracker {
    root: PathBuf,
    publish: Option<PathBuf>,
}

pub fn fingerprint_hex(content: &Content) -> String {
    crate::daemon::fingerprint(content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Writes private snapshots atomically, including retries after process death.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("invalid delivery path")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(|e| e.to_string())?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
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

impl Tracker {
    pub fn new(state_path: &Path, device: &Id) -> Self {
        Self {
            root: state_path
                .with_extension("deliveries")
                .join(B64::encode(device)),
            publish: None,
        }
    }

    pub fn with_published(&mut self, dir: &Path) -> Result<()> {
        self.publish = Some(dir.join("deliveries.json"));
        self.save(&self.snapshot()?)
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        match fs::read(self.root.join("ledger.json")) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| format!("invalid delivery ledger: {e}"))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Snapshot::default()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn save(&self, snapshot: &Snapshot) -> Result<()> {
        let bytes = serde_json::to_vec(snapshot).map_err(|e| e.to_string())?;
        write_private(&self.root.join("ledger.json"), &bytes)?;
        if let Some(path) = &self.publish {
            write_private(path, &bytes)?;
        }
        Ok(())
    }

    pub fn track(&self, header: &ItemHeader, content: &Content, origin: &str) -> Result<()> {
        if matches!(content, Content::ClearHistory) {
            return Ok(());
        }
        let mut snapshot = self.snapshot()?;
        let item_id = B64::encode(&header.id);
        if snapshot.items.iter().any(|entry| entry.item_id == item_id) {
            return Ok(());
        }
        snapshot.items.push(Delivery {
            item_id,
            fingerprint: fingerprint_hex(content),
            origin_device: B64::encode(&header.device),
            origin_name: origin.into(),
            created_at: header.created_at,
            uploaded_at: None,
            receivers: Vec::new(),
        });
        snapshot.items.sort_by_key(|item| item.created_at);
        let excess = snapshot.items.len().saturating_sub(2500);
        snapshot.items.drain(..excess);
        self.save(&snapshot)
    }

    pub fn uploaded(&self, item: &Id) -> Result<()> {
        let mut snapshot = self.snapshot()?;
        if let Some(entry) = snapshot
            .items
            .iter_mut()
            .find(|entry| entry.item_id == B64::encode(item))
        {
            entry.uploaded_at.get_or_insert(now());
        }
        self.save(&snapshot)
    }

    /// Call only after the receiving device saved the actual content locally.
    pub fn received(
        &self,
        state: &State,
        header: &ItemHeader,
        content: &Content,
        origin: &str,
    ) -> Result<()> {
        self.track(header, content, origin)?;
        let mut snapshot = self.snapshot()?;
        let entry = snapshot
            .items
            .iter_mut()
            .find(|entry| entry.item_id == B64::encode(&header.id))
            .ok_or("missing received item")?;
        let device_id = B64::encode(&state.device.id());
        if entry
            .receivers
            .iter()
            .any(|receiver| receiver.device_id == device_id)
        {
            return Ok(());
        }
        let queued = self
            .root
            .join("outbox")
            .join(format!("{}.receipt", B64::encode(&header.id)));
        let receipt = if queued.exists() {
            let bytes = fs::read(&queued).map_err(|e| e.to_string())?;
            let receipt = SealedReceipt::from_bytes(&bytes).map_err(|e| e.to_string())?;
            let item = receipt
                .open(&state.account_key, &state.account, &state.device.public())
                .map_err(|e| e.to_string())?;
            if item != header.id {
                return Err("queued receipt belongs to another item".into());
            }
            receipt
        } else {
            let receipt_header = ItemHeader {
                id: random_id(&mut OsRng),
                device: state.device.id(),
                epoch: state.account_key.epoch(),
                created_at: now(),
            };
            let receipt = SealedReceipt::seal(
                &state.account_key,
                &state.account,
                receipt_header,
                &header.id,
                &state.device,
                &mut OsRng,
            )
            .map_err(|e| e.to_string())?;
            // Queue before the cursor advances. Retries use the same sealed bytes.
            write_private(&queued, &receipt.to_bytes())?;
            receipt
        };
        let received_at = receipt.header.created_at;
        entry.uploaded_at.get_or_insert(received_at);
        entry.receivers.push(Receiver {
            device_id,
            device_name: state.device_name.clone(),
            received_at,
        });
        self.save(&snapshot)
    }

    pub fn sync(&self, state: &State, remote: &Remote) -> Result<()> {
        let mut snapshot = self.snapshot()?;
        let page = remote.receipts(state, snapshot.cursor)?;
        let Some(page) = page else {
            snapshot.supported = false;
            return self.save(&snapshot);
        };
        snapshot.supported = true;
        let devices = remote
            .devices(state)?
            .records
            .into_iter()
            .filter_map(|bytes| {
                let record = SealedDeviceRecord::from_bytes(&bytes.0).ok()?;
                let name = record.open(&state.account_key, &state.account).ok()?;
                Some((record.device, name))
            })
            .collect::<Vec<_>>();
        snapshot.peers = devices
            .iter()
            .map(|(public, name)| Peer {
                device_id: B64::encode(&public.id),
                device_name: name.clone(),
            })
            .collect();
        for bytes in page.receipts {
            accept_receipt(
                &mut snapshot,
                &state.account_key,
                &state.account,
                &devices,
                &bytes.0,
                now(),
            );
        }
        snapshot.cursor = snapshot.cursor.max(page.cursor);
        self.save(&snapshot)?;
        let outbox = self.root.join("outbox");
        let paths = match fs::read_dir(outbox) {
            Ok(paths) => paths,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        for entry in paths {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "receipt")
            {
                let bytes = fs::read(&path).map_err(|e| e.to_string())?;
                let receipt = SealedReceipt::from_bytes(&bytes).map_err(|e| e.to_string())?;
                receipt
                    .open(&state.account_key, &state.account, &state.device.public())
                    .map_err(|e| e.to_string())?;
                remote.post_receipt(state, &bytes)?;
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        let mut snapshot = self.snapshot()?;
        snapshot.items.clear();
        match fs::remove_dir_all(self.root.join("outbox")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        self.save(&snapshot)
    }
}

fn accept_receipt(
    snapshot: &mut Snapshot,
    key: &AccountKey,
    account: &Id,
    devices: &[(DevicePublic, String)],
    bytes: &[u8],
    current_time: u64,
) -> bool {
    let Ok(receipt) = SealedReceipt::from_bytes(bytes) else {
        return false;
    };
    let Some((public, name)) = devices
        .iter()
        .find(|(public, _)| public.id == receipt.header.device)
    else {
        return false;
    };
    let Ok(item) = receipt.open(key, account, public) else {
        return false;
    };
    if receipt.header.created_at
        > current_time.saturating_add(pastazzo_core::request::MAX_CLOCK_SKEW_MS)
    {
        return false;
    }
    let Some(entry) = snapshot
        .items
        .iter_mut()
        .find(|entry| entry.item_id == B64::encode(&item))
    else {
        return false;
    };
    let device_id = B64::encode(&receipt.header.device);
    if device_id == entry.origin_device
        || entry
            .receivers
            .iter()
            .any(|receiver| receiver.device_id == device_id)
    {
        return false;
    }
    entry.receivers.push(Receiver {
        device_id,
        device_name: name.clone(),
        received_at: receipt.header.created_at,
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::KeyStorage;
    use pastazzo_core::device::DeviceKeys;
    use pastazzo_core::server::ServerIdentity;
    use std::os::unix::fs::PermissionsExt;

    fn state(name: &str) -> State {
        State {
            server_url: "http://127.0.0.1:1".into(),
            identity: ServerIdentity {
                opaque: [1; 32],
                transport: [2; 32],
                signing: [3; 32],
            },
            username: "synthetic".into(),
            account: [7; 16],
            account_key: AccountKey::from_bytes(1, [9; 32]).unwrap(),
            account_secret: None,
            device: DeviceKeys::generate(&mut OsRng),
            device_name: name.into(),
            cursor: 0,
            storage: KeyStorage::File,
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("pastazzo-delivery-{}", rand::random::<u64>())))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn upload_is_not_delivery_and_receipts_survive_restarts() {
        let fixture = Fixture::new();
        let sender = state("MacBook");
        let receiver = state("iPhone");
        let header = ItemHeader {
            id: [1; 16],
            device: sender.device.id(),
            epoch: 1,
            created_at: now(),
        };
        let content = Content::Text("synthetic copy".into());
        let source = Tracker::new(&fixture.0.join("sender.json"), &sender.device.id());
        source
            .track(&header, &content, &sender.device_name)
            .unwrap();
        source.uploaded(&header.id).unwrap();
        assert!(source.snapshot().unwrap().items[0].receivers.is_empty());
        let destination = Tracker::new(&fixture.0.join("receiver.json"), &receiver.device.id());
        destination
            .received(&receiver, &header, &content, &sender.device_name)
            .unwrap();
        let queued = destination
            .root
            .join("outbox")
            .join(format!("{}.receipt", B64::encode(&header.id)));
        let bytes = fs::read(&queued).unwrap();
        let restarted = Tracker::new(&fixture.0.join("receiver.json"), &receiver.device.id());
        restarted
            .received(&receiver, &header, &content, &sender.device_name)
            .unwrap();
        assert_eq!(fs::read(&queued).unwrap(), bytes);
        assert_eq!(restarted.snapshot().unwrap().items[0].receivers.len(), 1);
        assert_eq!(
            fs::metadata(&queued).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&destination.root)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let mut published = source.snapshot().unwrap();
        let known = vec![(receiver.device.public(), receiver.device_name.clone())];
        assert!(accept_receipt(
            &mut published,
            &sender.account_key,
            &sender.account,
            &known,
            &bytes,
            now()
        ));
        assert!(!accept_receipt(
            &mut published,
            &sender.account_key,
            &sender.account,
            &known,
            &bytes,
            now()
        ));
        assert_eq!(published.items[0].receivers.len(), 1);
        assert_eq!(published.items[0].receivers[0].device_name, "iPhone");
        source.save(&published).unwrap();
        assert_eq!(
            Tracker::new(&fixture.0.join("sender.json"), &sender.device.id())
                .snapshot()
                .unwrap()
                .items[0]
                .receivers
                .len(),
            1
        );
        restarted.clear().unwrap();
        assert!(!queued.exists());
        assert!(restarted.snapshot().unwrap().items.is_empty());
    }

    #[test]
    fn invalid_unknown_and_foreign_receipts_do_not_confirm_delivery() {
        let fixture = Fixture::new();
        let sender = state("MacBook");
        let receiver = state("iPhone");
        let original = ItemHeader {
            id: [1; 16],
            device: sender.device.id(),
            epoch: 1,
            created_at: now(),
        };
        let tracker = Tracker::new(&fixture.0.join("sender.json"), &sender.device.id());
        tracker
            .track(&original, &Content::Text("synthetic".into()), "MacBook")
            .unwrap();
        let mut snapshot = tracker.snapshot().unwrap();
        let header = ItemHeader {
            id: [2; 16],
            device: receiver.device.id(),
            epoch: 1,
            created_at: now(),
        };
        let bytes = SealedReceipt::seal(
            &receiver.account_key,
            &receiver.account,
            header,
            &original.id,
            &receiver.device,
            &mut OsRng,
        )
        .unwrap()
        .to_bytes();
        let known = vec![(receiver.device.public(), "iPhone".into())];
        assert!(!accept_receipt(
            &mut snapshot,
            &sender.account_key,
            &sender.account,
            &[],
            &bytes,
            now()
        ));
        assert!(!accept_receipt(
            &mut snapshot,
            &sender.account_key,
            &[8; 16],
            &known,
            &bytes,
            now()
        ));
        let wrong = SealedReceipt::seal(
            &receiver.account_key,
            &receiver.account,
            header,
            &[3; 16],
            &receiver.device,
            &mut OsRng,
        )
        .unwrap()
        .to_bytes();
        assert!(!accept_receipt(
            &mut snapshot,
            &sender.account_key,
            &sender.account,
            &known,
            &wrong,
            now()
        ));
        let mut tampered = bytes.clone();
        tampered[100] ^= 1;
        assert!(!accept_receipt(
            &mut snapshot,
            &sender.account_key,
            &sender.account,
            &known,
            &tampered,
            now()
        ));
        let future_header = ItemHeader {
            created_at: now() + 1000000,
            ..header
        };
        let future = SealedReceipt::seal(
            &receiver.account_key,
            &receiver.account,
            future_header,
            &original.id,
            &receiver.device,
            &mut OsRng,
        )
        .unwrap()
        .to_bytes();
        assert!(!accept_receipt(
            &mut snapshot,
            &sender.account_key,
            &sender.account,
            &known,
            &future,
            now()
        ));
        assert!(snapshot.items[0].receivers.is_empty());
    }

    #[test]
    fn receipt_storage_failure_is_reported_before_a_confirmation() {
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.0).unwrap();
        let sender = state("MacBook");
        let receiver = state("iPhone");
        fs::write(
            fixture.0.join("receiver.deliveries"),
            b"blocks the ledger directory",
        )
        .unwrap();
        let tracker = Tracker::new(&fixture.0.join("receiver.json"), &receiver.device.id());
        let header = ItemHeader {
            id: [1; 16],
            device: sender.device.id(),
            epoch: 1,
            created_at: now(),
        };
        assert!(
            tracker
                .received(
                    &receiver,
                    &header,
                    &Content::Text("synthetic".into()),
                    "MacBook"
                )
                .is_err()
        );
    }
}
