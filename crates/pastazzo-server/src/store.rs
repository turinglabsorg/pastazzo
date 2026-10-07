//! SQLite storage. Everything about clipboard contents in here is
//! ciphertext: the server keeps password files, wrapped account keys, device
//! public keys and records, and sealed items, none of which it can open.

use std::path::Path;

use pastazzo_core::Id;
use rusqlite::{Connection, OptionalExtension, params};

/// Items kept per account, newest first; older ones are dropped.
pub const MAX_ITEMS_PER_ACCOUNT: i64 = 500;
/// Items older than this are dropped too.
pub const ITEM_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1000;

pub struct Store {
    db: Connection,
}

pub struct Account {
    pub id: Id,
    pub password_file: Vec<u8>,
    pub wrapped_key: Vec<u8>,
}

pub struct Device {
    pub account: Id,
    pub public: Vec<u8>,
    pub revoked: bool,
}

pub struct Invite {
    pub public: [u8; 32],
    pub expires_at: u64,
    pub used: bool,
}

#[derive(Debug)]
pub enum Insert {
    Done,
    Conflict,
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS invites (
                 id BLOB PRIMARY KEY,
                 public BLOB NOT NULL,
                 created_at INTEGER NOT NULL,
                 expires_at INTEGER NOT NULL,
                 used_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS accounts (
                 id BLOB PRIMARY KEY,
                 username TEXT NOT NULL UNIQUE,
                 password_file BLOB NOT NULL,
                 wrapped_key BLOB NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS devices (
                 id BLOB PRIMARY KEY,
                 account BLOB NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 public BLOB NOT NULL,
                 record BLOB,
                 created_at INTEGER NOT NULL,
                 revoked_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS items (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 account BLOB NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 item_id BLOB NOT NULL,
                 device BLOB NOT NULL,
                 data BLOB NOT NULL,
                 received_at INTEGER NOT NULL,
                 UNIQUE (account, item_id)
             );
             CREATE INDEX IF NOT EXISTS items_by_account ON items (account, seq);",
        )?;
        Ok(Self { db })
    }

    // Invites.

    pub fn add_invite(
        &self,
        id: &Id,
        public: &[u8; 32],
        now: u64,
        expires_at: u64,
    ) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO invites (id, public, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![&id[..], &public[..], now as i64, expires_at as i64],
        )?;
        Ok(())
    }

    pub fn invite(&self, id: &Id) -> rusqlite::Result<Option<Invite>> {
        self.db
            .query_row(
                "SELECT public, expires_at, used_at FROM invites WHERE id = ?1",
                params![&id[..]],
                |row| {
                    let public: Vec<u8> = row.get(0)?;
                    Ok((public, row.get::<_, i64>(1)?, row.get::<_, Option<i64>>(2)?))
                },
            )
            .optional()
            .map(|row| {
                row.and_then(|(public, expires_at, used_at)| {
                    Some(Invite {
                        public: public.try_into().ok()?,
                        expires_at: expires_at as u64,
                        used: used_at.is_some(),
                    })
                })
            })
    }

    // Accounts.

    pub fn account_by_username(&self, username: &str) -> rusqlite::Result<Option<Account>> {
        self.db
            .query_row(
                "SELECT id, password_file, wrapped_key FROM accounts WHERE username = ?1",
                params![username],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map(|row| {
                row.and_then(|(id, password_file, wrapped_key)| {
                    Some(Account {
                        id: id.try_into().ok()?,
                        password_file,
                        wrapped_key,
                    })
                })
            })
    }

    pub fn username_taken(&self, username: &str) -> rusqlite::Result<bool> {
        Ok(self.account_by_username(username)?.is_some())
    }

    /// Creates the account and uses up the invite, atomically: if the
    /// username or account id is taken, or the invite was used meanwhile,
    /// nothing changes.
    pub fn create_account(
        &mut self,
        id: &Id,
        username: &str,
        password_file: &[u8],
        wrapped_key: &[u8],
        invite: Option<&Id>,
        now: u64,
    ) -> rusqlite::Result<Insert> {
        let tx = self.db.transaction()?;
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = ?1 OR username = ?2)",
            params![&id[..], username],
            |row| row.get(0),
        )?;
        if taken {
            return Ok(Insert::Conflict);
        }
        if let Some(invite) = invite {
            let used = tx.execute(
                "UPDATE invites SET used_at = ?2 WHERE id = ?1 AND used_at IS NULL AND expires_at > ?2",
                params![&invite[..], now as i64],
            )?;
            if used != 1 {
                return Ok(Insert::Conflict);
            }
        }
        tx.execute(
            "INSERT INTO accounts (id, username, password_file, wrapped_key, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![&id[..], username, password_file, wrapped_key, now as i64],
        )?;
        tx.commit()?;
        Ok(Insert::Done)
    }

    pub fn usernames(&self) -> rusqlite::Result<Vec<(String, u64)>> {
        let mut statement = self.db.prepare(
            "SELECT username, (SELECT COUNT(*) FROM devices WHERE devices.account = accounts.id AND revoked_at IS NULL)
             FROM accounts ORDER BY username",
        )?;
        let rows =
            statement.query_map([], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)))?;
        rows.collect()
    }

    pub fn delete_account(&self, username: &str) -> rusqlite::Result<bool> {
        Ok(self.db.execute(
            "DELETE FROM accounts WHERE username = ?1",
            params![username],
        )? == 1)
    }

    // Devices.

    pub fn device(&self, id: &Id) -> rusqlite::Result<Option<Device>> {
        self.db
            .query_row(
                "SELECT account, public, revoked_at FROM devices WHERE id = ?1",
                params![&id[..]],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .optional()
            .map(|row| {
                row.and_then(|(account, public, revoked_at)| {
                    Some(Device {
                        account: account.try_into().ok()?,
                        public,
                        revoked: revoked_at.is_some(),
                    })
                })
            })
    }

    /// Registers a device's public keys. Logging in again with the same keys
    /// is fine; reusing a device id with other keys or for another account,
    /// or a revoked one, is a conflict.
    pub fn add_device(
        &self,
        id: &Id,
        account: &Id,
        public: &[u8],
        now: u64,
    ) -> rusqlite::Result<Insert> {
        if let Some(existing) = self.device(id)? {
            let same =
                existing.account == *account && existing.public == public && !existing.revoked;
            return Ok(if same { Insert::Done } else { Insert::Conflict });
        }
        self.db.execute(
            "INSERT INTO devices (id, account, public, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![&id[..], &account[..], public, now as i64],
        )?;
        Ok(Insert::Done)
    }

    pub fn set_device_record(&self, id: &Id, record: &[u8]) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE devices SET record = ?2 WHERE id = ?1",
            params![&id[..], record],
        )?;
        Ok(())
    }

    pub fn device_records(&self, account: &Id) -> rusqlite::Result<Vec<Vec<u8>>> {
        let mut statement = self.db.prepare(
            "SELECT record FROM devices WHERE account = ?1 AND revoked_at IS NULL AND record IS NOT NULL ORDER BY created_at",
        )?;
        let rows = statement.query_map(params![&account[..]], |row| row.get(0))?;
        rows.collect()
    }

    pub fn revoke_device(&self, id: &Id, account: &Id, now: u64) -> rusqlite::Result<bool> {
        Ok(self.db.execute(
            "UPDATE devices SET revoked_at = ?3 WHERE id = ?1 AND account = ?2 AND revoked_at IS NULL",
            params![&id[..], &account[..], now as i64],
        )? == 1)
    }

    // Items.

    /// Stores a sealed item and returns its cursor.
    pub fn add_item(
        &mut self,
        account: &Id,
        item: &Id,
        device: &Id,
        data: &[u8],
        now: u64,
    ) -> rusqlite::Result<Option<u64>> {
        let tx = self.db.transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO items (account, item_id, device, data, received_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![&account[..], &item[..], &device[..], data, now as i64],
        )?;
        if inserted == 0 {
            return Ok(None);
        }
        let seq = tx.last_insert_rowid() as u64;
        tx.execute(
            "DELETE FROM items WHERE account = ?1 AND (received_at < ?2 OR seq <= (
                 SELECT seq FROM items WHERE account = ?1 ORDER BY seq DESC LIMIT 1 OFFSET ?3))",
            params![
                &account[..],
                now.saturating_sub(ITEM_TTL_MS) as i64,
                MAX_ITEMS_PER_ACCOUNT
            ],
        )?;
        tx.commit()?;
        Ok(Some(seq))
    }

    /// Items after `cursor`, oldest first, at most `limit`.
    pub fn items_after(
        &self,
        account: &Id,
        cursor: u64,
        limit: usize,
    ) -> rusqlite::Result<Vec<(u64, Vec<u8>)>> {
        let mut statement = self.db.prepare(
            "SELECT seq, data FROM items WHERE account = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = statement
            .query_map(params![&account[..], cursor as i64, limit as i64], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get(1)?))
            })?;
        rows.collect()
    }

    /// The newest cursor of an account, for a device that only wants what
    /// comes next.
    pub fn latest_cursor(&self, account: &Id) -> rusqlite::Result<u64> {
        self.db.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM items WHERE account = ?1",
            params![&account[..]],
            |row| Ok(row.get::<_, i64>(0)? as u64),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn account_creation_uses_up_the_invite_once() {
        let mut store = store();
        store.add_invite(&[1; 16], &[2; 32], 0, 100).unwrap();
        assert!(matches!(
            store
                .create_account(&[3; 16], "seb", b"file", b"key", Some(&[1; 16]), 10)
                .unwrap(),
            Insert::Done
        ));
        assert!(store.invite(&[1; 16]).unwrap().unwrap().used);
        // Same invite again, for another account: refused.
        assert!(matches!(
            store
                .create_account(&[4; 16], "other", b"file", b"key", Some(&[1; 16]), 10)
                .unwrap(),
            Insert::Conflict
        ));
        // Taken username or account id: refused.
        assert!(matches!(
            store
                .create_account(&[5; 16], "seb", b"file", b"key", None, 10)
                .unwrap(),
            Insert::Conflict
        ));
        assert!(matches!(
            store
                .create_account(&[3; 16], "new", b"file", b"key", None, 10)
                .unwrap(),
            Insert::Conflict
        ));
        assert_eq!(
            store.account_by_username("seb").unwrap().unwrap().id,
            [3; 16]
        );
    }

    #[test]
    fn expired_invites_are_refused() {
        let mut store = store();
        store.add_invite(&[1; 16], &[2; 32], 0, 100).unwrap();
        assert!(matches!(
            store
                .create_account(&[3; 16], "seb", b"f", b"k", Some(&[1; 16]), 100)
                .unwrap(),
            Insert::Conflict
        ));
    }

    #[test]
    fn devices_and_items() {
        let mut store = store();
        store
            .create_account(&[3; 16], "seb", b"f", b"k", None, 0)
            .unwrap();
        assert!(matches!(
            store.add_device(&[7; 16], &[3; 16], b"pub", 0).unwrap(),
            Insert::Done
        ));
        assert!(matches!(
            store.add_device(&[7; 16], &[3; 16], b"pub", 0).unwrap(),
            Insert::Done
        ));
        assert!(matches!(
            store.add_device(&[7; 16], &[3; 16], b"other", 0).unwrap(),
            Insert::Conflict
        ));
        store.set_device_record(&[7; 16], b"record").unwrap();
        assert_eq!(
            store.device_records(&[3; 16]).unwrap(),
            vec![b"record".to_vec()]
        );

        let first = store
            .add_item(&[3; 16], &[1; 16], &[7; 16], b"a", 1)
            .unwrap()
            .unwrap();
        assert_eq!(
            store
                .add_item(&[3; 16], &[1; 16], &[7; 16], b"a", 1)
                .unwrap(),
            None
        );
        let second = store
            .add_item(&[3; 16], &[2; 16], &[7; 16], b"b", 2)
            .unwrap()
            .unwrap();
        assert_eq!(
            store.items_after(&[3; 16], first, 10).unwrap(),
            vec![(second, b"b".to_vec())]
        );
        assert_eq!(store.latest_cursor(&[3; 16]).unwrap(), second);

        assert!(store.revoke_device(&[7; 16], &[3; 16], 5).unwrap());
        assert!(store.device(&[7; 16]).unwrap().unwrap().revoked);
        assert!(store.device_records(&[3; 16]).unwrap().is_empty());
        assert!(matches!(
            store.add_device(&[7; 16], &[3; 16], b"pub", 0).unwrap(),
            Insert::Conflict
        ));
    }

    #[test]
    fn old_items_are_pruned() {
        let mut store = store();
        store
            .create_account(&[3; 16], "seb", b"f", b"k", None, 0)
            .unwrap();
        for i in 0..(MAX_ITEMS_PER_ACCOUNT as u64 + 10) {
            let mut id = [0u8; 16];
            id[..8].copy_from_slice(&i.to_be_bytes());
            store
                .add_item(&[3; 16], &id, &[7; 16], b"x", 1_000)
                .unwrap();
        }
        assert_eq!(
            store.items_after(&[3; 16], 0, 10_000).unwrap().len(),
            MAX_ITEMS_PER_ACCOUNT as usize
        );
        // An item past its TTL goes as soon as the next one arrives.
        store
            .add_item(
                &[3; 16],
                &[0xff; 16],
                &[7; 16],
                b"new",
                1_000 + ITEM_TTL_MS + 1,
            )
            .unwrap();
        assert_eq!(store.items_after(&[3; 16], 0, 10_000).unwrap().len(), 1);
    }

    #[test]
    fn deleting_an_account_removes_everything() {
        let mut store = store();
        store
            .create_account(&[3; 16], "seb", b"f", b"k", None, 0)
            .unwrap();
        store.add_device(&[7; 16], &[3; 16], b"pub", 0).unwrap();
        store
            .add_item(&[3; 16], &[1; 16], &[7; 16], b"a", 1)
            .unwrap();
        assert!(store.delete_account("seb").unwrap());
        assert!(store.device(&[7; 16]).unwrap().is_none());
        assert!(store.items_after(&[3; 16], 0, 10).unwrap().is_empty());
    }
}
