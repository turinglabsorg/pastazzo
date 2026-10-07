//! Clipboard items.
//!
//! An item is a header the server can see (ids, epoch, timestamp) and a
//! ciphertext it can't: the padded content, encrypted with
//! XChaCha20-Poly1305 under the `items` subkey, with the header and account
//! id bound in as associated data. Changing any header field, moving an item
//! to another account or replaying it under another id makes it fail to
//! decrypt.

use crate::account::AccountKey;
use crate::pad::{pad, unpad};
use crate::wire::{Reader, check_version};
use crate::{Error, Id, PROTOCOL_VERSION, Rng, aead_open, aead_seal, transcript};

pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;
const MAX_MIME_LEN: usize = 64;

const KIND_TEXT: u8 = 1;
const KIND_IMAGE: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Image { mime: String, data: Vec<u8> },
}

impl Content {
    /// `kind(1) || u32be(len(mime)) || mime || data`; text has an empty mime.
    fn encode(&self) -> Result<Vec<u8>, Error> {
        let (kind, mime, data): (u8, &str, &[u8]) = match self {
            Content::Text(text) if text.len() <= MAX_TEXT_BYTES => (KIND_TEXT, "", text.as_bytes()),
            Content::Image { mime, data } if data.len() <= MAX_IMAGE_BYTES => {
                validate_image_mime(mime)?;
                (KIND_IMAGE, mime, data)
            }
            _ => return Err(Error::TooLarge),
        };
        let mut out = Vec::with_capacity(5 + mime.len() + data.len());
        out.push(kind);
        crate::wire::push_bytes(&mut out, mime.as_bytes());
        out.extend_from_slice(data);
        Ok(out)
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes, "item content");
        let kind = reader.u8()?;
        let mime = reader.bytes()?;
        let data = reader.rest();
        match kind {
            KIND_TEXT if mime.is_empty() && data.len() <= MAX_TEXT_BYTES => {
                let text = std::str::from_utf8(data).map_err(|_| Error::Malformed("item text"))?;
                Ok(Content::Text(text.to_owned()))
            }
            KIND_IMAGE if data.len() <= MAX_IMAGE_BYTES => {
                let mime = std::str::from_utf8(mime).map_err(|_| Error::Malformed("item mime"))?;
                validate_image_mime(mime)?;
                Ok(Content::Image {
                    mime: mime.to_owned(),
                    data: data.to_vec(),
                })
            }
            _ => Err(Error::Malformed("item content")),
        }
    }
}

fn validate_image_mime(mime: &str) -> Result<(), Error> {
    let subtype = mime
        .strip_prefix("image/")
        .ok_or(Error::Malformed("item mime"))?;
    let valid = mime.len() <= MAX_MIME_LEN
        && !subtype.is_empty()
        && subtype
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b));
    if valid {
        Ok(())
    } else {
        Err(Error::Malformed("item mime"))
    }
}

/// The part of an item the server sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemHeader {
    /// Random, chosen by the sender. Clients ignore ids they've already seen.
    pub id: Id,
    /// The sending device.
    pub device: Id,
    /// Epoch of the account key the item is encrypted under.
    pub epoch: u32,
    /// Milliseconds since the Unix epoch, sender's clock.
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedItem {
    pub header: ItemHeader,
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

/// Length of the binary header in [`SealedItem::to_bytes`].
pub const SEALED_HEADER_LEN: usize = 1 + 16 + 16 + 4 + 8 + 24;

impl SealedItem {
    pub fn seal(
        key: &AccountKey,
        account: &Id,
        header: ItemHeader,
        content: &Content,
        rng: &mut impl Rng,
    ) -> Result<Self, Error> {
        if header.epoch != key.epoch() {
            return Err(Error::Malformed("item epoch"));
        }
        let encoded = zeroize::Zeroizing::new(content.encode()?);
        let plaintext = zeroize::Zeroizing::new(pad(&encoded)?);
        let (nonce, ciphertext) = aead_seal(
            &key.items_key(),
            &plaintext,
            &item_aad(account, &header),
            rng,
        );
        Ok(Self {
            header,
            nonce,
            ciphertext,
        })
    }

    /// Decrypts with the account key for `self.header.epoch`.
    pub fn open(&self, key: &AccountKey, account: &Id) -> Result<Content, Error> {
        if self.header.epoch != key.epoch() {
            return Err(Error::Decrypt);
        }
        let plaintext = zeroize::Zeroizing::new(aead_open(
            &key.items_key(),
            &self.nonce,
            &self.ciphertext,
            &item_aad(account, &self.header),
        )?);
        Content::decode(unpad(&plaintext)?)
    }

    /// `version(1) || id(16) || device(16) || u32be(epoch) || u64be(created_at) || nonce(24) || ciphertext`
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SEALED_HEADER_LEN + self.ciphertext.len());
        out.push(PROTOCOL_VERSION);
        out.extend_from_slice(&self.header.id);
        out.extend_from_slice(&self.header.device);
        out.extend_from_slice(&self.header.epoch.to_be_bytes());
        out.extend_from_slice(&self.header.created_at.to_be_bytes());
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.ciphertext);
        out
    }

    /// Parses the layout of [`SealedItem::to_bytes`]. The server uses this to
    /// read the header; it can't check anything else.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes, "sealed item");
        check_version(reader.u8()?, "sealed item")?;
        let header = ItemHeader {
            id: reader.array()?,
            device: reader.array()?,
            epoch: reader.u32()?,
            created_at: reader.u64()?,
        };
        let nonce = reader.array()?;
        let ciphertext = reader.rest().to_vec();
        // The smallest valid ciphertext is one padded block plus the tag.
        if ciphertext.len() < crate::pad::MIN_PADDED_LEN + 16 {
            return Err(Error::Malformed("sealed item"));
        }
        Ok(Self {
            header,
            nonce,
            ciphertext,
        })
    }

    #[cfg(test)]
    pub(crate) fn seal_with_nonce(
        key: &AccountKey,
        account: &Id,
        header: ItemHeader,
        content: &Content,
        nonce: [u8; 24],
    ) -> Self {
        let plaintext = pad(&content.encode().unwrap()).unwrap();
        let ciphertext = crate::aead_seal_with_nonce(
            &key.items_key(),
            &nonce,
            &plaintext,
            &item_aad(account, &header),
        );
        Self {
            header,
            nonce,
            ciphertext,
        }
    }
}

fn item_aad(account: &Id, header: &ItemHeader) -> Vec<u8> {
    transcript(
        "pastazzo/v1/item",
        &[
            account,
            &header.id,
            &header.device,
            &header.epoch.to_be_bytes(),
            &header.created_at.to_be_bytes(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn header() -> ItemHeader {
        ItemHeader {
            id: [1; 16],
            device: [2; 16],
            epoch: 1,
            created_at: 1_791_379_434_274,
        }
    }

    #[test]
    fn text_and_image_roundtrip() {
        let key = AccountKey::generate(&mut OsRng);
        let account = [7; 16];
        for content in [
            Content::Text("hunter2".into()),
            Content::Text(String::new()),
            Content::Text("é".repeat(1000)),
            Content::Image {
                mime: "image/png".into(),
                data: vec![0x89, b'P', b'N', b'G'],
            },
        ] {
            let sealed = SealedItem::seal(&key, &account, header(), &content, &mut OsRng).unwrap();
            let decoded = SealedItem::from_bytes(&sealed.to_bytes()).unwrap();
            assert_eq!(decoded, sealed);
            assert_eq!(decoded.open(&key, &account).unwrap(), content);
        }
    }

    #[test]
    fn short_secrets_all_look_the_same() {
        let key = AccountKey::generate(&mut OsRng);
        let short = SealedItem::seal(
            &key,
            &[7; 16],
            header(),
            &Content::Text("a".into()),
            &mut OsRng,
        )
        .unwrap();
        let longer = SealedItem::seal(
            &key,
            &[7; 16],
            header(),
            &Content::Text("x".repeat(200)),
            &mut OsRng,
        )
        .unwrap();
        assert_eq!(short.ciphertext.len(), longer.ciphertext.len());
    }

    #[test]
    fn tampering_is_detected() {
        let key = AccountKey::generate(&mut OsRng);
        let account = [7; 16];
        let sealed = SealedItem::seal(
            &key,
            &account,
            header(),
            &Content::Text("secret".into()),
            &mut OsRng,
        )
        .unwrap();

        assert_eq!(sealed.open(&key, &[8; 16]).err(), Some(Error::Decrypt));
        assert_eq!(
            sealed
                .open(&AccountKey::generate(&mut OsRng), &account)
                .err(),
            Some(Error::Decrypt)
        );

        let mut moved = sealed.clone();
        moved.header.id = [9; 16];
        assert_eq!(moved.open(&key, &account).err(), Some(Error::Decrypt));

        let mut moved = sealed.clone();
        moved.header.device = [9; 16];
        assert_eq!(moved.open(&key, &account).err(), Some(Error::Decrypt));

        let mut moved = sealed.clone();
        moved.header.created_at += 1;
        assert_eq!(moved.open(&key, &account).err(), Some(Error::Decrypt));

        let mut flipped = sealed.clone();
        flipped.ciphertext[10] ^= 1;
        assert_eq!(flipped.open(&key, &account).err(), Some(Error::Decrypt));
    }

    #[test]
    fn epoch_must_match_the_key() {
        let key = AccountKey::generate(&mut OsRng);
        let mut wrong = header();
        wrong.epoch = 2;
        assert!(
            SealedItem::seal(
                &key,
                &[7; 16],
                wrong,
                &Content::Text("x".into()),
                &mut OsRng
            )
            .is_err()
        );

        let sealed = SealedItem::seal(
            &key,
            &[7; 16],
            header(),
            &Content::Text("x".into()),
            &mut OsRng,
        )
        .unwrap();
        assert_eq!(
            sealed.open(&key.rotate(&mut OsRng), &[7; 16]).err(),
            Some(Error::Decrypt)
        );
    }

    #[test]
    fn limits_and_mimes() {
        let key = AccountKey::generate(&mut OsRng);
        let too_long = Content::Text("x".repeat(MAX_TEXT_BYTES + 1));
        assert_eq!(
            SealedItem::seal(&key, &[7; 16], header(), &too_long, &mut OsRng).err(),
            Some(Error::TooLarge)
        );
        for mime in ["text/plain", "image/", "image/png; x=1", "image/../x"] {
            let image = Content::Image {
                mime: mime.into(),
                data: vec![1],
            };
            assert!(
                SealedItem::seal(&key, &[7; 16], header(), &image, &mut OsRng).is_err(),
                "{mime}"
            );
        }
        let svg = Content::Image {
            mime: "image/svg+xml".into(),
            data: vec![1],
        };
        assert!(SealedItem::seal(&key, &[7; 16], header(), &svg, &mut OsRng).is_ok());
    }

    #[test]
    fn truncated_or_wrong_version_is_rejected() {
        let key = AccountKey::generate(&mut OsRng);
        let bytes = SealedItem::seal(
            &key,
            &[7; 16],
            header(),
            &Content::Text("x".into()),
            &mut OsRng,
        )
        .unwrap()
        .to_bytes();
        assert!(SealedItem::from_bytes(&bytes[..SEALED_HEADER_LEN + 10]).is_err());
        let mut wrong_version = bytes.clone();
        wrong_version[0] = 2;
        assert!(SealedItem::from_bytes(&wrong_version).is_err());
    }
}
