//! Test vectors, also listed in `docs/PROTOCOL.md`. They pin the wire format:
//! if one changes, the protocol changed.

use rand::SeedableRng;
use sha2::{Digest, Sha256};

use crate::account::{AccountKey, WrappedAccountKey};
use crate::device::DeviceKeys;
use crate::invite::InviteKey;
use crate::item::{Content, ItemHeader, SealedItem};
use crate::opaque;
use crate::request::{RequestSignature, Scope};
use crate::server::{ServerIdentity, ServerKeys};
use crate::session;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `start, start + 1, ...` as an array.
fn seq<const N: usize>(start: u8) -> [u8; N] {
    std::array::from_fn(|i| start.wrapping_add(i as u8))
}

const CREATED_AT: u64 = 1_791_379_434_274;

fn account_key() -> AccountKey {
    AccountKey::from_bytes(1, seq(0x00)).unwrap()
}

fn device_keys() -> DeviceKeys {
    let mut secret = vec![crate::PROTOCOL_VERSION];
    secret.extend_from_slice(&seq::<16>(0x20));
    secret.extend_from_slice(&seq::<32>(0x30));
    secret.extend_from_slice(&seq::<32>(0x50));
    DeviceKeys::from_secret_bytes(&secret).unwrap()
}

/// A full registration and login with a seeded RNG. Not reproducible by
/// another OPAQUE implementation (it depends on how opaque-ke draws random
/// numbers), but it pins the cipher suite, Argon2id parameters, identifiers
/// and context: any change there changes these values.
fn opaque_vectors() -> Vec<(&'static str, String)> {
    const PASSWORD: &str = "correct horse battery staple";
    let mut rng = rand_chacha::ChaCha20Rng::from_seed([0x42; 32]);
    let keys = ServerKeys::generate(&mut rng);
    let (request, state) = opaque::client_registration_start(&mut rng, "seb", PASSWORD).unwrap();
    let response = opaque::server_registration_start(&keys, &request, "seb").unwrap();
    let registration = state
        .finish(&mut rng, PASSWORD, &response, &keys.identity())
        .unwrap();
    let file = opaque::server_registration_finish(&registration.upload).unwrap();
    let (request, client) = opaque::client_login_start(&mut rng, PASSWORD).unwrap();
    let (response, server) =
        opaque::server_login_start(&mut rng, &keys, Some(&file), &request, "seb").unwrap();
    let login = client
        .finish(&mut rng, PASSWORD, &response, &keys.identity())
        .unwrap();
    assert_eq!(
        server.finish(&login.finalization).unwrap(),
        login.session_key
    );
    assert_eq!(login.export_key, registration.export_key);
    vec![
        (
            "opaque server fingerprint",
            hex(&keys.identity().fingerprint()),
        ),
        ("opaque password file sha256", hex(&Sha256::digest(&file))),
        ("opaque export key", hex(&login.export_key)),
        ("opaque session key", hex(&login.session_key)),
    ]
}

fn vectors() -> Vec<(&'static str, String)> {
    let key = account_key();
    let account = seq::<16>(0xa0);
    let header = ItemHeader {
        id: seq(0x10),
        device: seq(0x20),
        epoch: 1,
        created_at: CREATED_AT,
    };
    let item = SealedItem::seal_with_nonce(
        &key,
        &account,
        header,
        &Content::Text("hello".into()),
        seq(0xb0),
    )
    .to_bytes();
    let wrapped = WrappedAccountKey::wrap_with_nonce(&key, &seq::<64>(0x40), &account, seq(0xc0));
    let device = device_keys();
    let scope = Scope {
        server_fingerprint: seq(0x90),
        account,
    };
    let invite = InviteKey::from_secret(seq(0xe0), seq(0xd0));
    let server = ServerIdentity {
        opaque: [1; 32],
        transport: [2; 32],
        signing: [3; 32],
    };
    let mut vectors = vec![
        ("items subkey", hex(key.items_key().as_ref())),
        ("devices subkey", hex(key.devices_key().as_ref())),
        ("wrapped account key", hex(&wrapped.to_bytes())),
        ("sealed item length", item.len().to_string()),
        ("sealed item sha256", hex(&Sha256::digest(&item))),
        ("device signing public", hex(&device.public().signing)),
        ("device exchange public", hex(&device.public().exchange)),
        (
            "request signature",
            hex(&RequestSignature::sign_with_nonce(
                &device,
                &scope,
                "POST",
                "/v1/items",
                b"body",
                CREATED_AT,
                seq(0xf0),
            )
            .signature),
        ),
        ("invite verifier", hex(&invite.verifier().public)),
        (
            "invite start proof",
            hex(&invite.start_proof("seb", b"request")),
        ),
        (
            "invite finish proof",
            hex(&invite.finish_proof("seb", b"sealed")),
        ),
        (
            "device binding tag",
            hex(&session::device_binding_tag(
                &seq::<64>(0x60),
                "seb",
                &device.public(),
            )),
        ),
        (
            "login response tag",
            hex(&session::login_response_tag(
                &seq::<64>(0x60),
                "seb",
                &account,
                &wrapped,
            )),
        ),
        ("server fingerprint", hex(&server.fingerprint())),
    ];
    vectors.extend(opaque_vectors());
    vectors
}

const EXPECTED: &[(&str, &str)] = &[
    (
        "items subkey",
        "618cc388ad00a0b9a5e23751d98726cf2a3f9b5eeb6553c2f0372abc9f4c575b",
    ),
    (
        "devices subkey",
        "adea2ef7ba33f0dfe4c0e1d59414a2d976e2dbd11006aff29a7c4bc1219e13a6",
    ),
    (
        "wrapped account key",
        "00000001c0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d2264b7da1ab3d562d2e36493053de88c792b4a9473fc6a2681caf8c7f82a0cba48f97894fe23911cf080a2c9a90c2fc",
    ),
    ("sealed item length", "341"),
    (
        "sealed item sha256",
        "3229c379d25d2d04482546c64d06adfa82bab837bc1ad654795453b9ee0c5507",
    ),
    (
        "device signing public",
        "8bb04e1c1b83dddf311f5bcddf7c50ede3c0802f47ec796e2a131cf41298d9f3",
    ),
    (
        "device exchange public",
        "392d174a38b3b1beafaf1fe824870841c5fa531bc6eafdb6402c124664488c1c",
    ),
    (
        "request signature",
        "ce199fd2e07212767c8fa317564c1d657e645eb926e274ab95b56c07b333f9c87f195f6a9e5c43792c0f778a3307c803df8be07b92fda3d80c8a1cfdab065c0c",
    ),
    (
        "invite verifier",
        "577577ed2fe0cea0d9181cad7db6ff8fc33a8c54b63c1d03d89e0e50312b1ee4",
    ),
    (
        "invite start proof",
        "f12edf5623e5a024f87b63a1ed13a927f2850a0bf4aff9d5a419676a1f704f2156e47e852de2fa596a754c7d14df57c3bd3a9dff31966b359106945efa4e5e0a",
    ),
    (
        "invite finish proof",
        "3b010f76ff7919433b08230e6dfe98f96bbf3104a5375c718c58bb152bfafe8e55d273511209e8bf00aa9dc29071d1d436114d31d334a86b5901870c32215606",
    ),
    (
        "device binding tag",
        "61f1f33bd6249fe7a1daf2555e8be71a7e31d443d6c7f44f6d2e85e70e5f4ef2",
    ),
    (
        "login response tag",
        "cf8ea8ebbe3c33ab88a5c0448365e868032fb30ad34703c4c1294b911e613eb7",
    ),
    (
        "server fingerprint",
        "f69c9f26c4c1190ee7f3488ec1747cc838188379dda29cf9ca1ace9e6f403d9b",
    ),
    (
        "opaque server fingerprint",
        "e89a3768d5bde88ae9e5cda6b0e8104f2306d92aeed356e261cbd54404d8a26a",
    ),
    (
        "opaque password file sha256",
        "439149940391933c31faa17f9953e9ddfa0bbe227255e234ef5ced4aacd234a5",
    ),
    (
        "opaque export key",
        "c07d8760739a593b101c4d7167a0f3f433ce9b282179f25c053ca8783f3fe8d5750784d54d0b0b49995bfb4782dddd07f2e4dd5e91ec5c76f0eddf2a03e3d409",
    ),
    (
        "opaque session key",
        "a9402e9290eb9d155c192e2694538c8c3d2da59ea12efc28a0b17a507130d0a2c1395036e3932e3b66ac8132cfb0f223a3dc159096f999a495a02962dade2e05",
    ),
];

/// Run with `--nocapture` to print the current values.
#[test]
fn vectors_match() {
    let actual = vectors();
    for (name, value) in &actual {
        println!("{name}: {value}");
    }
    let actual: Vec<(&str, &str)> = actual
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    assert_eq!(actual, EXPECTED);
}
