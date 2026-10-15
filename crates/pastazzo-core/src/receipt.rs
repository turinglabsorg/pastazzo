//! Signed, encrypted device delivery acknowledgements, on a separate feed.
use crate::account::AccountKey;
use crate::device::{DeviceKeys, DevicePublic};
use crate::item::{ItemHeader, SealedItem};
use crate::pad::{pad, unpad};
use crate::{Error, Id, Rng, aead_open, aead_seal, transcript};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedReceipt {
    pub header: ItemHeader,
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

impl SealedReceipt {
    pub fn seal(
        key: &AccountKey,
        account: &Id,
        header: ItemHeader,
        item: &Id,
        device: &DeviceKeys,
        rng: &mut impl Rng,
    ) -> Result<Self, Error> {
        if header.epoch != key.epoch() || header.device != device.id() {
            return Err(Error::Malformed("receipt sender"));
        }
        let mut body = item.to_vec();
        body.extend_from_slice(&device.sign(&signature_message(account, &header, item)));
        let (nonce, ciphertext) =
            aead_seal(&key.items_key(), &pad(&body)?, &aad(account, &header), rng);
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
        item: &Id,
        device: &DeviceKeys,
        nonce: [u8; 24],
    ) -> Self {
        let mut body = item.to_vec();
        body.extend_from_slice(&device.sign(&signature_message(account, &header, item)));
        let ciphertext = crate::aead_seal_with_nonce(
            &key.items_key(),
            &nonce,
            &pad(&body).unwrap(),
            &aad(account, &header),
        );
        Self {
            header,
            nonce,
            ciphertext,
        }
    }

    pub fn open(&self, key: &AccountKey, account: &Id, device: &DevicePublic) -> Result<Id, Error> {
        if self.header.epoch != key.epoch() || self.header.device != device.id {
            return Err(Error::Decrypt);
        }
        let padded = aead_open(
            &key.items_key(),
            &self.nonce,
            &self.ciphertext,
            &aad(account, &self.header),
        )?;
        let body = unpad(&padded)?;
        if body.len() != 80 {
            return Err(Error::Malformed("receipt content"));
        }
        let item: Id = body[..16]
            .try_into()
            .map_err(|_| Error::Malformed("receipt item"))?;
        let signature =
            ed25519_dalek::Signature::from_slice(&body[16..]).map_err(|_| Error::Decrypt)?;
        device
            .verifying_key()?
            .verify_strict(&signature_message(account, &self.header, &item), &signature)
            .map_err(|_| Error::Decrypt)?;
        Ok(item)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        SealedItem {
            header: self.header,
            nonce: self.nonce,
            ciphertext: self.ciphertext.clone(),
        }
        .to_bytes()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        // Fixed 256-byte padded body plus authentication tag and envelope.
        if bytes.len() != crate::item::SEALED_HEADER_LEN + 256 + 16 {
            return Err(Error::Malformed("sealed receipt"));
        }
        let item = SealedItem::from_bytes(bytes)?;
        Ok(Self {
            header: item.header,
            nonce: item.nonce,
            ciphertext: item.ciphertext,
        })
    }
}

fn aad(account: &Id, header: &ItemHeader) -> Vec<u8> {
    transcript(
        "pastazzo/v1/receipt",
        &[
            account,
            &header.id,
            &header.device,
            &header.epoch.to_be_bytes(),
            &header.created_at.to_be_bytes(),
        ],
    )
}

fn signature_message(account: &Id, header: &ItemHeader, item: &Id) -> Vec<u8> {
    transcript(
        "pastazzo/v1/receipt-signature",
        &[
            account,
            &header.id,
            &header.device,
            &header.epoch.to_be_bytes(),
            &header.created_at.to_be_bytes(),
            item,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;
    #[test]
    fn receipts_bind_item_account_receiver_and_time() {
        let key = AccountKey::generate(&mut OsRng);
        let receiver = DeviceKeys::generate(&mut OsRng);
        let other = DeviceKeys::generate(&mut OsRng);
        let header = ItemHeader {
            id: [1; 16],
            device: receiver.id(),
            epoch: key.epoch(),
            created_at: 1234,
        };
        let receipt =
            SealedReceipt::seal(&key, &[2; 16], header, &[3; 16], &receiver, &mut OsRng).unwrap();
        let decoded = SealedReceipt::from_bytes(&receipt.to_bytes()).unwrap();
        assert_eq!(
            decoded.open(&key, &[2; 16], &receiver.public()).unwrap(),
            [3; 16]
        );
        assert!(decoded.open(&key, &[4; 16], &receiver.public()).is_err());
        assert!(decoded.open(&key, &[2; 16], &other.public()).is_err());
        let mut modified = decoded.clone();
        modified.header.created_at += 1;
        assert!(modified.open(&key, &[2; 16], &receiver.public()).is_err());
        assert!(
            SealedItem::from_bytes(&receipt.to_bytes())
                .unwrap()
                .open(&key, &[2; 16])
                .is_err()
        );
        // A holder of the account key cannot change the receiver or item and
        // authenticate it using another device's signature.
        let body = aead_open(
            &key.items_key(),
            &decoded.nonce,
            &decoded.ciphertext,
            &aad(&[2; 16], &decoded.header),
        )
        .unwrap();
        let mut forged_body = unpad(&body).unwrap().to_vec();
        forged_body[0] ^= 1;
        let (nonce, ciphertext) = aead_seal(
            &key.items_key(),
            &pad(&forged_body).unwrap(),
            &aad(&[2; 16], &decoded.header),
            &mut OsRng,
        );
        let forged = SealedReceipt {
            header: decoded.header,
            nonce,
            ciphertext,
        };
        assert!(forged.open(&key, &[2; 16], &receiver.public()).is_err());
    }
}
