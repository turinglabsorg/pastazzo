# Security

Pastazzo syncs your clipboard between your devices through a server you or someone else runs. Clipboards hold passwords, tokens, private messages and screenshots, so the sync is designed around one rule: **only your devices can read your clipboard.** Not the server, not whoever runs it, not a relay or proxy in between, not the network.

This document is the threat model. [PROTOCOL.md](PROTOCOL.md) has the exact cryptography and wire format.

## Trust

| Trusted | Not trusted |
| --- | --- |
| Your devices, while they aren't compromised | The network |
| Your password | TLS, and anything that terminates it: reverse proxies, tunnels, relays, CDNs |
| | The server, its operator and its database |

TLS is still used, but no guarantee below depends on it. Treat every server as if it were run by someone curious, and every connection as if a relay could read and change it.

## What is guaranteed

Against a relay, or anyone else on the network, that can see and modify all traffic:

- **Contents stay secret.** Every clipboard item is encrypted on the device that copied it (XChaCha20-Poly1305) with a key only your devices hold. Sizes are padded (Padmé, at least 256 bytes), so a short password looks like any other short text. Content hashes aren't sent either: duplicate detection happens on the devices, after decryption.
- **Your password can't be attacked from the traffic.** Registration and login use OPAQUE (RFC 9807): the password never leaves the device, and nothing on the wire can be used to test guesses offline. OPAQUE needs the registration to be authenticated, so the server signs its registration response and the client checks the signature against the pinned server identity before going on.
- **The account key never travels in the clear.** It's created on your first device and stored on the server only encrypted under a key that needs two things: your password (through the OPAQUE export key) and the account secret, which only your devices have.
- **A new device needs your approval.** Password login waits for a trusted device's approval. Passwordless QR pairing requires a five-minute capability displayed on a trusted Mac and its explicit confirmation. In both flows, account secrets are sealed to the new device's own keys. A QR approval is additionally signed by the owner key physically transferred in the QR; the server cannot substitute that key or the recipient's proof. Someone with your password alone gets nothing.
- **Items can't be forged, altered or moved.** Each item is authenticated together with its account, id, sending device id, key epoch and timestamp, under a key only holders of the account key have.
- **Offline uploads stay encrypted.** Pending uploads survive a restart in an owner-only outbox (0700 directories, 0600 files) containing sealed items only. Retrying does not decrypt them on the server, add another copy, or reuse a signed request nonce. An identical item is acknowledged at its original cursor; a changed body under the same item id is rejected.
- **Devices can't be impersonated.** There are no bearer tokens. Every request is signed with the device's own Ed25519 key, covering the server, the account, the method, path, timestamp, a nonce and the body hash. The device's public key is bound to the OPAQUE login it was registered in, so a relay can't swap in its own.
- **Invites can't be stolen in transit.** The invite secret is never sent: the client signs each registration message with a key derived from it, so a relay can neither reuse an invite nor replace the registration with its own. The server keeps only the public half, so its database holds no usable invites.
- **With an invite, you can't be tricked into using another server.** The invite link carries the server's identity fingerprint (its OPAQUE, transport and signing public keys), and registration checks it. A new device gets the fingerprint from one already logged in (or from the invite link) and checks it too; independently of that, the OPAQUE login itself only succeeds with the server key your registration was made with.
- **You can check it yourself.** Settings show the account key's fingerprint, which must be the same on every device, and each device's key fingerprint, which must match what that device shows for itself. The server can't compute the first and can't fake the second.
- **Clearing the history everywhere needs the account key.** The request travels as an encrypted item like any copy, so neither the server nor a relay can make devices wipe their history.
- **The registration record is sealed to the server.** OPAQUE's registration record is about as sensitive as a password hash, so it's encrypted to the server's transport key and a relay only sees ciphertext.

Against the server itself, all of the above except where noted below: it doesn't see contents, the password or the account key, and it can't forge items or device keys, unless it guesses your password offline (see below).

## What isn't

- **Your password against the server.** The server necessarily holds what it needs to check logins, and with it, it can test password guesses offline, at an Argon2id computation (64 MiB, 3 passes) per guess. A guessed password lets it log in as you, but not open anything: that also takes the account secret. **Use a strong, unique password** anyway.
- **Losing every device without a recovery backup.** The account secret lives on your devices: with all of them and any encrypted recovery backups gone, the account can't be opened again, and you create a new one. Recovery backups contain account and device secrets; protect them accordingly (see [RECOVERY.md](RECOVERY.md)).
- **Registering without an invite.** On an open server the client trusts the server's identity on first use and shows its fingerprint. Until you've compared it with one you got another way, a relay could be posing as the server.
- **Availability.** The server, or a relay, can drop, delay or reorder items, or refuse service.
- **A photographed pairing QR.** Its temporary capability lets the holder request pairing, but cannot decrypt history without the Mac's confirmation. The first requester binds the session; an unexpected requester requires cancellation and a new QR. Scan and confirm only your own device. Expiry, cancellation, and owner revocation are enforced by an honest server; a malicious server can withhold expiry or cancellation, so the Mac also checks its local expiry before approval. Never approve an unexpected device or share the QR.
- **Metadata.** The server sees when items are sent, by which device, how many there are, their padded size (as soon as a big one is announced), your username and your devices' public keys.
- **Which device sent an item.** Items are authenticated as coming from a holder of the account key. The sending device's id is bound to the item but not signed by that device, so a device holding the key could claim to be another one.
- **Replays of old items.** A server can send an old, authentic item again. Clients ignore item ids they've already seen and never put an item on the clipboard if it's older than the last one they applied, but an item older than what a device remembers could reappear in its history, and an old "clear history" could empty it again.
- **Revoked devices, until key rotation exists.** Revoking a device makes the server refuse its requests, but the device still has the account key and the account secret: with the password it could log in again as a new device and open the account key without anyone's approval, and a server could keep presenting its record to your other devices. Cutting a device off for good needs a rotation of the account key and secret and an authenticated, versioned device list; both are planned and not built yet.
- **A compromised device.** It has the account key and can read everything.
- **Your clipboard history on each device.** It is stored on the device as it is today, unencrypted. The history and the inbox of received items are readable by your user only (directories 0700, files 0600), so other accounts on the same machine can't read them, but root, backups and anything running as you can. Protecting it beyond that is up to the device (disk encryption, screen lock).
- **iOS sharing is deferred.** The share extension saves text or images to a private, file-protected App Group inbox, excluded from backup. The foreground app imports them and sends them through the same encrypted Rust client. The extension does not send plaintext to the network. iOS device keys use the local protected keychain with `WhenUnlockedThisDeviceOnly`; there is no file fallback. Received history never changes the system clipboard automatically, and explicit Copy uses a local-only pasteboard item.
- **Home Screen widget actions open the app.** Widgets hold no keys, show no clipboard content, read no history, and make no network requests. The Paste link requests a single foreground clipboard import, subject to the iOS paste permission. Ordinary launch, History, and background refresh do not read the clipboard. Widget links contain only a fixed action, never clipboard content or credentials.

## For operators

- Registration defaults to invite-only. `open` lets anyone create an account (rate-limited); `closed` refuses new accounts even with an invite.
- Invite links are printed by the admin CLI on the server. Hand them over through a channel that doesn't go through your relay: a link shown by a web page behind the relay could have its fingerprint rewritten.
- A server behind a TLS-terminating relay is a supported setup: the relay is outside the trust boundary by design.

## Reporting a vulnerability

Please report security issues privately through [GitHub security advisories](https://github.com/turinglabsorg/pastazzo/security/advisories/new), not in public issues.
