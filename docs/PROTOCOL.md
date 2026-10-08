# Protocol

Version 1. This is the cryptography and wire format of pastazzo sync, implemented once in [`crates/pastazzo-core`](../crates/pastazzo-core) and shared by the server and every client. [SECURITY.md](SECURITY.md) explains what it protects against and what it doesn't.

## Conventions

- Integers are big-endian. `u32be(x)` is 4 bytes, `u64be(x)` 8 bytes.
- `||` is concatenation.
- Ids (accounts, devices, items, invites) are 16 random bytes.
- Timestamps are milliseconds since the Unix epoch.
- In JSON, binary values are base64url without padding.

### Transcripts

Everything that gets hashed, MAC'd, signed or used as AEAD associated data is a **transcript**: a label and a list of fields, each with its length.

```text
transcript(label, [f1, ..., fn]) = u32be(len(label)) || label || u32be(len(f1)) || f1 || ... || u32be(len(fn)) || fn
```

Labels are `pastazzo/v1/<purpose>`. Because each label names one purpose and every field has its length, a value made for one purpose can never be accepted for another.

### Primitives

| Use | Primitive |
| --- | --- |
| Key derivation | HKDF-SHA256, empty salt, 32-byte output: `kdf(ikm, info)` with `info` a transcript |
| Encryption | XChaCha20-Poly1305, random 24-byte nonce |
| MAC | HMAC-SHA256, verified in constant time |
| Signatures | Ed25519, strict verification |
| Sealed boxes | libsodium `crypto_box_seal` (X25519, XSalsa20-Poly1305) |
| Password authentication | OPAQUE, RFC 9807 (see below) |

## Keys

### Account key

The **account key** (AK) is 32 random bytes created by the first device. It has an **epoch**, starting at 1, which goes up on every rotation. Two subkeys derive from it:

```text
items_key   = kdf(AK, transcript("pastazzo/v1/subkey", ["items",   u32be(epoch)]))
devices_key = kdf(AK, transcript("pastazzo/v1/subkey", ["devices", u32be(epoch)]))
```

The first device also creates the **account secret** (AS), 32 random bytes. The account's devices keep it and hand it to each device they [approve](#approving-a-device); the server never sees it.

The server stores AK **wrapped** under a key that needs both the OPAQUE export key, so the password, and AS:

```text
wrapping_key = kdf(export_key || AS, transcript("pastazzo/v1/wrapping-key", []))
aad          = transcript("pastazzo/v1/wrapped-account-key", [account_id, u32be(epoch)])
wrapped      = u32be(epoch) || nonce(24) || XChaCha20-Poly1305(wrapping_key, nonce, AK, aad)   // 76 bytes, epoch ≥ 1
```

### Fingerprints

Settings show fingerprints so people can check, by eye, that every device holds the same account key and talks to the same server:

```text
account key fingerprint = first 16 bytes of kdf(AK, transcript("pastazzo/v1/subkey", ["fingerprint", u32be(epoch)]))
device fingerprint      = SHA-256(transcript("pastazzo/v1/device-fingerprint", [device_public]))
```

The account key fingerprint is a subkey of its own, so showing it reveals nothing about the key or the other subkeys, and the server can't compute it. The server fingerprint is defined under [Server identity](#server-identity). They're shown as the first 16 bytes in uppercase hex, in groups of four.

### Device keys

Every device generates an Ed25519 signing key and an X25519 exchange key and keeps them in the OS keychain:

```text
device_public = device_id(16) || ed25519_public(32) || x25519_public(32)                         // 80 bytes
device_secret = 0x01 || device_id(16) || ed25519_secret(32) || x25519_secret(32)                  // 81 bytes, never sent
```

### Server identity

A server has three key pairs:

- **OPAQUE** (Ristretto255): authenticates the server at every login.
- **Transport** (X25519): clients seal the registration record to it.
- **Signing** (Ed25519): signs the registration response.

Clients pin its fingerprint:

```text
server_identity = opaque_public(32) || transport_public(32) || signing_public(32)                 // 96 bytes
fingerprint     = SHA-256(transcript("pastazzo/v1/server-identity", [opaque_public, transport_public, signing_public]))
```

## OPAQUE

- Cipher suite: Ristretto255 OPRF; 3DH over Ristretto255 with SHA-512; Argon2id (version 0x13, 64 MiB, 3 passes, 4 lanes, the second recommended option of RFC 9106) as the key stretching function.
- Credential identifier: the username, 1 to 64 characters of `a-z 0-9 . _ -`.
- Client and server identities: the defaults (their public keys).
- Login context: `pastazzo/v1`.
- Passwords: UTF-8 in Unicode NFC.
- After every registration and login the client checks that the server's OPAQUE public key is the pinned one.
- For an unknown username the server runs the login with no password file, so the response looks like a real one.

Changing any of these parameters changes every password file, so they're fixed for protocol version 1.

## Registration

OPAQUE assumes registration runs over an authenticated channel. A relay isn't one: it could answer the registration request with an OPRF evaluation under its own key, and then test password guesses offline against the record it causes the client to create. So with an invite every step below is authenticated in both directions: the client with the invite, the server with its signing key. Without one (open registration) the client isn't authenticated, so a relay could substitute its own record and take the username, which step 6 detects; and the server's signature is only as good as the trust-on-first-use of its fingerprint.

1. The client fetches the server identity and checks it against the fingerprint in the invite link. Without an invite (open registration) it trusts it on first use and shows the fingerprint.
2. The client starts OPAQUE registration and sends the username, the registration request, the invite id and

   ```text
   invite_key  = Ed25519 key with seed kdf(invite_secret, transcript("pastazzo/v1/invite-key", [invite_id]))
   start_proof = Ed25519(invite_key, transcript("pastazzo/v1/invite/registration-start", [invite_id, username, request]))
   ```

3. The server checks the registration mode, that the invite exists, is unused and unexpired, and the proof against the invite's public key. It answers with the OPAQUE registration response and

   ```text
   signature = Ed25519(server_signing, transcript("pastazzo/v1/registration-response", [username, request, response]))
   ```

4. The client checks the signature with the pinned signing key, and only then finishes OPAQUE (getting the export key). It creates AK at epoch 1, AS and a random account id, wraps AK, and seals the **registration record** to the server's transport key:

   ```text
   record       = 0x01 || u32be(len) || username || account_id(16) || u32be(len) || opaque_upload || u32be(len) || wrapped
   sealed       = crypto_box_seal(transport_public, record)
   finish_proof = Ed25519(invite_key, transcript("pastazzo/v1/invite/registration-finish", [invite_id, username, sealed]))
   ```

5. The server checks the proof, opens the record, checks that the username matches step 2 and is still free and that the account id isn't taken, and stores the password file, the account id and the wrapped AK. The invite is used up.
6. The client logs in right away. If that fails, the registration was tampered with. As the account's first device it's approved at once: it already has AS.

## Login

A new device needs the server URL and fingerprint first: from a device already logged in, or from the invite link. It checks the server identity against the fingerprint like registration does.

1. The client starts OPAQUE login and sends the username and the credential request.
2. The server answers with the credential response and keeps the login state in memory, briefly and for one use.
3. The client finishes OPAQUE, getting the session key and the export key, and binds its device keys to the session:

   ```text
   mac_key(purpose) = kdf(session_key, transcript("pastazzo/v1/session-mac", [purpose]))
   binding = HMAC(mac_key("device-binding"), transcript("pastazzo/v1/device-binding", [username, device_public]))
   ```

   It sends the credential finalization, `device_public` and `binding`.
4. The server finishes OPAQUE, checks the binding and registers the device's public keys. From now on it accepts requests signed by that device. It answers with the account id, the wrapped AK and

   ```text
   tag = HMAC(mac_key("login-response"), transcript("pastazzo/v1/login-response", [username, account_id, wrapped]))
   ```

5. The client checks the tag. The account's first device is approved at once; any other device now waits to be [approved](#approving-a-device), which gives it AS. With both the export key and AS, the client unwraps AK, stores AK, AS and its device keys in the keychain, and publishes its device record.

## Approving a device

A device that logged in isn't in the account yet: the server only accepts its requests to collect its grant, and the password alone doesn't open AK.

1. The new device shows its **approval code**, the first 64 bits of its [device fingerprint](#fingerprints) (`4A4E 0279 A9F5 C72F`), and asks to be approved.
2. The devices already in the account see it waiting (`GET /v1/devices`), with the same code computed from the public keys the server lists. The person approving compares the two codes. If the server had slipped in keys of its own, they wouldn't match, and with 64 bits it can't search for keys that happen to give the same code in the time an approval takes.
3. The approving device seals a **grant** to the new device's X25519 key:

   ```text
   grant = crypto_box_seal(x25519_public, 0x01 || account_id(16) || device_id(16) || AS(32))
   ```

   and uploads it (`PUT /v1/devices/{id}/grant`). That approves the device on the server.
4. The new device collects it (`GET /v1/devices/{id}/grant`), checks it names this account and device, unwraps AK, and publishes its record.

Grants aren't signed: a forged one carries a wrong AS, and AK then doesn't unwrap. Rejecting a waiting device is revoking it. Requests nobody approves within a day are dropped.


## Signed requests

Every request after login carries the device id, a timestamp, a random 16-byte nonce and

```text
signature = Ed25519(device_secret, transcript("pastazzo/v1/request", [server_fingerprint, account_id, device_id, method, path_and_query, u64be(timestamp), nonce, SHA-256(body)]))
```

The server rejects the request if the signature doesn't verify with that device's key, if the device was revoked, if the timestamp is more than 5 minutes from its clock, or if it has seen the `(device_id, nonce)` pair before. It remembers nonces for at least 10 minutes.

## Device records

Each device publishes a record that other devices can check. The name is UTF-8, at most 128 bytes, and padded to a single 256-byte block so its length doesn't show:

```text
aad    = transcript("pastazzo/v1/device-record", [account_id, device_public, u32be(epoch)])
record = 0x01 || device_public(80) || u32be(epoch) || nonce(24) || u32be(272) || XChaCha20-Poly1305(devices_key, nonce, padded_name, aad)
```

Every record is 385 bytes. A device only trusts records that decrypt, which proves they were made by a holder of the account key.

## Items

An item's content is

```text
content = kind(1) || u32be(len(mime)) || mime || data
```

- Text: kind `1`, empty mime, UTF-8 data up to 1 MiB.
- Image: kind `2`, mime `image/<subtype>` (subtype of `A-Z a-z 0-9 + - .`, mime at most 64 bytes), data up to 25 MiB.
- Clear history: kind `3`, empty mime and data. Not a copy: every device that receives it empties its clipboard history. Like any item, only a holder of the account key can make one.

It's padded before encryption, to hide its length:

```text
padded = u32be(len(content)) || content || zeros up to padded_len(4 + len(content))
```

`padded_len` is Padmé applied to `max(L, 256)`: with `E = ⌊log2 L⌋` and `S = ⌊log2 E⌋ + 1`, round `L` up to a multiple of `2^(E-S)`. Anything up to 252 bytes of content pads to 256 bytes, and the overhead is at most 6.25%. Receivers reject padding that isn't canonical.

Then it's encrypted, with the header the server can see bound in:

```text
aad    = transcript("pastazzo/v1/item", [account_id, item_id, device_id, u32be(epoch), u64be(created_at)])
sealed = 0x01 || item_id(16) || device_id(16) || u32be(epoch) || u64be(created_at) || nonce(24) || XChaCha20-Poly1305(items_key, nonce, padded, aad)
```

The header is 69 bytes, and the smallest ciphertext is 272 bytes (256 padded plus the 16-byte tag).

Clients:

- pick a random item id, and use their own clock for `created_at`;
- ignore item ids they have already seen;
- put an item on the clipboard only if it's newer than the last one they applied and no more than 5 minutes ahead of their own clock; other unseen items only go to the history;
- detect echoes and duplicates after decryption, on the device. Content hashes are never sent to the server, not even keyed ones.
- keep failed uploads as sealed items in a private on-device outbox. A retry sends the same sealed bytes with a fresh signed request nonce and timestamp, including after a restart. Identical uploads by the same device return the original cursor; reusing an item id with different bytes is refused with HTTP 409. Item encryption and its wire format do not change on retry.

## Invite links

```text
pastazzo://join?v=1&server=<url>&fp=<fingerprint>&id=<invite id>&key=<invite secret>
```

`server`, `fp`, `id` and `key` are base64url without padding; the server URL starts with `https://` or `http://`. The secret is 32 bytes. It's never sent anywhere, and the server doesn't keep it: the admin CLI stores only the invite id and the public half of `invite_key`, then prints the link. Deliver it out of band.

## Passwordless QR pairing

A trusted, approved device creates an in-memory pairing session using a fresh invite id and seed. The server stores only its `InviteVerifier`, account, owner, username, and five-minute expiry. Creating another session on that owner invalidates the previous one. Restart, cancellation, expiry, or owner revocation invalidates a session.

The physical QR uses `pastazzo://pair?v=1&server=<url>&fp=<fingerprint>&id=<id>&key=<seed>&account=<account>&owner=<DevicePublic>&username=<username>&expires=<milliseconds>`. All values except version and expiry are base64url without padding. The parser rejects unknown or duplicate fields and links over 4096 bytes. The URL carries public trust data and a temporary invite capability, never account keys or a password.

The iPhone generates its own device keys. Its proof is the existing invite start proof for the username, with the request set to `T("pastazzo/v1/pairing-peer", [DevicePublic, UTF8(name)])`. Device names are nonempty and at most 128 UTF-8 bytes. The server binds the first valid device and name; retries for that peer are idempotent, other peers get 409. The Mac independently verifies this proof and compares the approval code before approving.

The grant plaintext is `version(1) || pairing_id(16) || account_id(16) || recipient_DevicePublic(80) || epoch_be(4) || AK(32) || account_secret(32)`. It is sealed to the recipient using the existing device sealed-box format. The owner signs `T("pastazzo/v1/pairing-grant", [pairing_id, account_id, recipient_DevicePublic, sealed_box])`. The wire grant is `Ed25519_signature(64) || sealed_box`. The iPhone verifies the signature against the owner's signing key in the physical QR, then decrypts and checks every embedded binding. It publishes its normal encrypted device record and begins at the latest cursor. No account password is used.

## HTTP API

JSON bodies unless noted. Endpoints marked *signed* need a [signed request] from an approved device(#signed-requests), sent in the headers `pastazzo-device`, `pastazzo-timestamp` (decimal milliseconds), `pastazzo-nonce` and `pastazzo-signature`; the signed path is the request's path and query exactly as sent. Errors are `{"error": "..."}` with a 4xx or 5xx status.

| Endpoint | Purpose |
| --- | --- |
| `GET /v1/server` | Protocol version, server identity, registration mode |
| `POST /v1/register/start` | Registration steps 2–3 |
| `POST /v1/register/finish` | Registration steps 4–5 |
| `POST /v1/login/start` | Login steps 1–2 |
| `POST /v1/login/finish` | Login steps 3–4 |
| `POST /v1/pairings` | *Signed.* Create a QR session: `{id, verifier, username}`; returns `{expires_at, peer, completed}` |
| `GET /v1/pairings/{id}` | *Signed, by the owner.* Status including the peer's `{device, name, proof}` |
| `DELETE /v1/pairings/{id}` | *Signed, by the owner.* Cancel the session |
| `POST /v1/pairings/{id}/request` | Invite proof authenticated. Submit `{device, name, proof}`; returns `{account, grant}` with null grant until confirmation |
| `PUT /v1/pairings/{id}/grant` | *Signed, by the owner.* Submit the binary signed sealed grant and approve the peer. Identical retries succeed; changed grants get 409 |
| `PUT /v1/devices/{id}` | *Signed.* Publish this device's record (binary body) |
| `GET /v1/devices` | *Signed.* The account's device records, and the public keys of the devices waiting for approval |
| `DELETE /v1/devices/{id}` | *Signed.* Revoke a device of the account, or reject one waiting |
| `PUT /v1/devices/{id}/grant` | *Signed.* Approve a waiting device with its grant (binary body) |
| `GET /v1/devices/{id}/grant` | *Signed, by that device, even while it waits.* Its grant, or 404 until it's approved |
| `POST /v1/items/announce` | *Signed.* `{"id", "size"}`: an upload of that sealed item is about to start |
| `POST /v1/items` | *Signed.* Upload a sealed item (binary body, possibly chunked); answers with its cursor |
| `GET /v1/items?after=<cursor>&wait=<seconds>&pending=<version>` | *Signed.* Items after a cursor, oldest first, with the announced uploads in progress. With nothing new, the server holds the request up to `wait` seconds (at most 30) and answers as soon as an item arrives or, if `pending` is given, as soon as the uploads change from that version. `after=latest` returns only the current cursor, for a device that only wants what comes next. |
| `DELETE /v1/items` | *Signed.* Delete every item of the account |

Receiving is long polling rather than WebSockets so it works through any proxy or tunnel.

### Progress of big items

Clients announce items of 64 KiB or more before uploading them. The server keeps the announcement for that device and item, and counts the bytes of the upload as they stream in. Items pages carry the account's uploads in progress (`pending`: uploading device, item id, size, bytes received) and a `pending_version` that changes with them, so the other devices show an item coming while it's still being uploaded, and then follow their own download. Nothing about an upload is stored, or passed on as an item, until the whole request and its signature check out.

## Not yet specified

- **Account key rotation and revocation (M6).** Device records are only authenticated with the account key: they prove a record was made by a key holder, not that the device is still trusted, and a revoked device still holds the key. Rotation needs an authenticated, versioned device list signed by trusted devices, with revocation entries, built from what each device has confirmed itself; and clients that remember the newest epoch they've seen, so a server can't hand a new device an old wrapped key.
- **Per-device item signatures,** so items prove which device sent them.
- Streaming encryption for items larger than the current limits.

## Test vectors

The tests in `crates/pastazzo-core/src/vectors.rs` check these values. `seq(x, n)` is the `n` bytes `x, x+1, ...`.

Inputs:

- AK = `seq(0x00, 32)`, epoch 1; account id = `seq(0xa0, 16)`
- export key = `seq(0x40, 64)`; account secret = `seq(0x70, 32)`; wrapping nonce = `seq(0xc0, 24)`
- item: id `seq(0x10, 16)`, device `seq(0x20, 16)`, epoch 1, created_at `1791379434274`, text `hello`, nonce `seq(0xb0, 24)`
- device: id `seq(0x20, 16)`, Ed25519 secret `seq(0x30, 32)`, X25519 secret `seq(0x50, 32)`
- request: server fingerprint `seq(0x90, 32)`, the account id above, `POST /v1/items`, body `body`, timestamp `1791379434274`, nonce `seq(0xf0, 16)`
- request retry: the same scope, device, path, and body, timestamp `1791379434275`, nonce `seq(0xa0, 16)`
- invite: id `seq(0xe0, 16)`, secret `seq(0xd0, 32)`, username `seb`; start request `request`, sealed record `sealed`
- session key = `seq(0x60, 64)`, username `seb`, the device and wrapped key above
- QR pairing: owner is the device above, account and invite ids and account secret as above; recipient generation followed by sealed grant uses a fresh ChaCha20 RNG seeded with `24` × 32; peer name `iPhone`.
- server identity: OPAQUE public `01` × 32, transport public `02` × 32, signing public `03` × 32

Outputs (hex):

```text
items subkey            618cc388ad00a0b9a5e23751d98726cf2a3f9b5eeb6553c2f0372abc9f4c575b
devices subkey          adea2ef7ba33f0dfe4c0e1d59414a2d976e2dbd11006aff29a7c4bc1219e13a6
wrapped account key     00000001c0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d727fad6fee9f14bfba715bcddc302c0038ccd76d6b29e5e44b991a40148a7d3471f30ef4d39c97d8d86d56d1a066dc95e
sealed item length      341
sealed item sha256      3229c379d25d2d04482546c64d06adfa82bab837bc1ad654795453b9ee0c5507
device signing public   8bb04e1c1b83dddf311f5bcddf7c50ede3c0802f47ec796e2a131cf41298d9f3
device exchange public  392d174a38b3b1beafaf1fe824870841c5fa531bc6eafdb6402c124664488c1c
request signature       ce199fd2e07212767c8fa317564c1d657e645eb926e274ab95b56c07b333f9c87f195f6a9e5c43792c0f778a3307c803df8be07b92fda3d80c8a1cfdab065c0c
request retry signature aa8b9bf920e32856d45122c808a99cb48c7326c3712fc52c0ebe5bf0b058d4be2109b21425970c3e7ed95ef0ddc8b2dc390a5b0a62572b945b648a0be13f840d
invite verifier         577577ed2fe0cea0d9181cad7db6ff8fc33a8c54b63c1d03d89e0e50312b1ee4
invite start proof      f12edf5623e5a024f87b63a1ed13a927f2850a0bf4aff9d5a419676a1f704f2156e47e852de2fa596a754c7d14df57c3bd3a9dff31966b359106945efa4e5e0a
invite finish proof     3b010f76ff7919433b08230e6dfe98f96bbf3104a5375c718c58bb152bfafe8e55d273511209e8bf00aa9dc29071d1d436114d31d334a86b5901870c32215606
device binding tag      61f1f33bd6249fe7a1daf2555e8be71a7e31d443d6c7f44f6d2e85e70e5f4ef2
login response tag      45150e97f4c3a2fefd47c3a3c23c68a7730183f068bc5799e30aaf51cf4e3d87
server fingerprint      f69c9f26c4c1190ee7f3488ec1747cc838188379dda29cf9ca1ace9e6f403d9b
account key fingerprint 6b02d833142fb0cab9369ae46e2e09a9
device fingerprint      4a4e0279a9f5c72f651881210fad3c1bb987a210771c5f59ce4baa19b382e0d7
approval code           4A4E 0279 A9F5 C72F
pairing peer proof      633da9d50894a69d902cd2b4bc7518b29369522f438bbb59a8bc7fe1d692fdbeb7d545b798a7bf247d5c0c281925360f8a6a60fe4126400ecbe228f8e5699601
pairing grant length    293
pairing grant sha256    adf592192d392feae3002513957f8b7e946ce450ce6d8d0ad0f541a58cfa2038
```

The tests also run a full registration and login with a ChaCha20 RNG seeded with `42` × 32 and pin the resulting export key, session key and password file. Those values depend on how opaque-ke draws random numbers, so they're a regression check for this implementation, not vectors for another one: what they pin is the cipher suite, the Argon2id parameters, the identifiers and the context.
