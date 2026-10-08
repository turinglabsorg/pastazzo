//! Short-lived, passwordless device pairing. The QR carries an invite
//! capability and the trusted owner's public keys, never account keys.

use crate::account::{AccountKey, AccountSecret};
use crate::api::B64;
use crate::device::{DeviceKeys, DevicePublic};
use crate::invite::{Invite, InviteVerifier};
use crate::wire::{Reader, check_version};
use crate::{Error, Id, PROTOCOL_VERSION, Rng, transcript};
use ed25519_dalek::{Signature, VerifyingKey};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

pub const TTL_MS: u64 = 5 * 60 * 1000;

pub struct PairingLink {
    pub invite: Invite,
    pub account: Id,
    pub owner: DevicePublic,
    pub username: String,
    pub expires_at: u64,
}

impl PairingLink {
    pub fn to_link(&self) -> Zeroizing<String> {
        let base = Zeroizing::new(
            self.invite
                .to_link()
                .replace("pastazzo://join?", "pastazzo://pair?"),
        );
        Zeroizing::new(format!(
            "{}&account={}&owner={}&username={}&expires={}",
            base.as_str(),
            B64::encode(&self.account),
            B64::encode(&self.owner.to_bytes()),
            B64::encode(self.username.as_bytes()),
            self.expires_at
        ))
    }

    pub fn parse(link: &str) -> Result<Self, Error> {
        let malformed = Error::Malformed("pairing QR");
        if link.len() > 4096 {
            return Err(malformed);
        }
        let query = link
            .trim()
            .strip_prefix("pastazzo://pair?")
            .ok_or(malformed.clone())?;
        let mut fields = BTreeMap::new();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').ok_or(malformed.clone())?;
            if ![
                "v", "server", "fp", "id", "key", "account", "owner", "username", "expires",
            ]
            .contains(&key)
                || fields.insert(key, value).is_some()
            {
                return Err(malformed);
            }
        }
        let get = |key| fields.get(key).copied().ok_or(malformed.clone());
        let base = Zeroizing::new(format!(
            "pastazzo://join?v={}&server={}&fp={}&id={}&key={}",
            get("v")?,
            get("server")?,
            get("fp")?,
            get("id")?,
            get("key")?
        ));
        let invite = Invite::parse(&base)?;
        let account = B64::decode(get("account")?)
            .and_then(|x| x.try_into().ok())
            .ok_or(malformed.clone())?;
        let owner =
            DevicePublic::from_bytes(&B64::decode(get("owner")?).ok_or(malformed.clone())?)?;
        let username = String::from_utf8(B64::decode(get("username")?).ok_or(malformed.clone())?)
            .map_err(|_| malformed.clone())?;
        crate::opaque::validate_username(&username)?;
        let expires_at = get("expires")?.parse().map_err(|_| malformed)?;
        Ok(Self {
            invite,
            account,
            owner,
            username,
            expires_at,
        })
    }
}

pub fn peer_message(device: &DevicePublic, name: &str) -> Vec<u8> {
    transcript(
        "pastazzo/v1/pairing-peer",
        &[&device.to_bytes(), name.as_bytes()],
    )
}

pub fn verify_peer(
    verifier: &InviteVerifier,
    username: &str,
    device: &DevicePublic,
    name: &str,
    proof: &[u8],
) -> Result<(), Error> {
    if name.trim().is_empty() || name.len() > 128 {
        return Err(Error::Malformed("device name"));
    }
    verifier.verify_start_proof(username, &peer_message(device, name), proof)
}

fn grant_message(id: &Id, account: &Id, recipient: &DevicePublic, sealed: &[u8]) -> Vec<u8> {
    transcript(
        "pastazzo/v1/pairing-grant",
        &[id, account, &recipient.to_bytes(), sealed],
    )
}

pub fn seal_grant(
    owner: &DeviceKeys,
    id: &Id,
    account: &Id,
    recipient: &DevicePublic,
    key: &AccountKey,
    secret: &AccountSecret,
    rng: &mut impl Rng,
) -> Vec<u8> {
    let mut plain = Zeroizing::new(vec![PROTOCOL_VERSION]);
    plain.extend_from_slice(id);
    plain.extend_from_slice(account);
    plain.extend_from_slice(&recipient.to_bytes());
    plain.extend_from_slice(&key.epoch().to_be_bytes());
    plain.extend_from_slice(key.expose_secret());
    plain.extend_from_slice(secret.expose_secret());
    let sealed = recipient.seal_to(&plain, rng);
    let mut grant = owner
        .sign(&grant_message(id, account, recipient, &sealed))
        .to_vec();
    grant.extend_from_slice(&sealed);
    grant
}

pub fn open_grant(
    link: &PairingLink,
    recipient: &DeviceKeys,
    grant: &[u8],
) -> Result<(AccountKey, AccountSecret), Error> {
    let malformed = Error::Malformed("pairing grant");
    let signature: [u8; 64] = grant
        .get(..64)
        .ok_or(malformed.clone())?
        .try_into()
        .map_err(|_| malformed.clone())?;
    let sealed = &grant[64..];
    VerifyingKey::from_bytes(&link.owner.signing)
        .map_err(|_| malformed.clone())?
        .verify_strict(
            &grant_message(
                &link.invite.key.id,
                &link.account,
                &recipient.public(),
                sealed,
            ),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| Error::BadSignature)?;
    let plain = recipient.unseal(sealed)?;
    let mut reader = Reader::new(&plain, "pairing grant");
    check_version(reader.u8()?, "pairing grant")?;
    if reader.array::<16>()? != link.invite.key.id
        || reader.array::<16>()? != link.account
        || reader.array::<80>()? != recipient.public().to_bytes()
    {
        return Err(malformed);
    }
    let epoch = reader.u32()?;
    let key = Zeroizing::new(reader.array::<32>()?);
    let secret = Zeroizing::new(reader.array::<32>()?);
    reader.finish()?;
    Ok((
        AccountKey::from_bytes(epoch, *key)?,
        AccountSecret::from_bytes(*secret),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invite::InviteKey;
    use rand::rngs::OsRng;

    #[test]
    fn qr_grants_are_bound_to_the_owner_session_account_and_recipient() {
        let owner = DeviceKeys::generate(&mut OsRng);
        let phone = DeviceKeys::generate(&mut OsRng);
        let key = AccountKey::generate(&mut OsRng);
        let secret = AccountSecret::generate(&mut OsRng);
        let mut link = PairingLink {
            invite: Invite {
                server_url: "https://clip.example.org".into(),
                fingerprint: [2; 32],
                key: InviteKey::generate(&mut OsRng),
            },
            account: [1; 16],
            owner: owner.public(),
            username: "seb".into(),
            expires_at: 99,
        };
        let parsed = PairingLink::parse(&link.to_link()).unwrap();
        assert_eq!(parsed.owner, link.owner);
        assert!(PairingLink::parse(&format!("{}&expires=10", link.to_link().as_str())).is_err());
        let grant = seal_grant(
            &owner,
            &link.invite.key.id,
            &link.account,
            &phone.public(),
            &key,
            &secret,
            &mut OsRng,
        );
        assert_eq!(
            open_grant(&link, &phone, &grant).unwrap().0.fingerprint(),
            key.fingerprint()
        );
        assert!(open_grant(&link, &DeviceKeys::generate(&mut OsRng), &grant).is_err());
        let mut tampered = grant.clone();
        tampered[70] ^= 1;
        assert!(open_grant(&link, &phone, &tampered).is_err());
        link.owner = DeviceKeys::generate(&mut OsRng).public();
        assert!(open_grant(&link, &phone, &grant).is_err());
        link.owner = owner.public();
        link.account[0] ^= 1;
        assert!(open_grant(&link, &phone, &grant).is_err());
    }
}
