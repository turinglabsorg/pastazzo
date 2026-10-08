# Encrypted recovery backups

An account password alone cannot recover the account key. Keep an encrypted recovery backup of a trusted device's account key, account secret, device keys, and connection metadata. Anyone who can decrypt that backup can act as the backed-up device: protect it as carefully as the device itself.

`pastazzo-sync backup` sends the recovery document directly to hush's encryption process. It writes only an encrypted envelope to standard output; it does not write plaintext keys to a temporary file or print them.

```sh
pastazzo-sync backup --hush-public-key ~/.hush/box.pub \
  | hush box open PASTAZZO_DEVICE_RECOVERY
```

Use `--hush /absolute/path/to/hush` if hush is outside the process's PATH. The backup must run in a session authorized to read the device's keychain. An SSH session on macOS may not have that access; run it in the logged-in user's session instead.

Generate and retain a new account password without printing it:

```sh
hush generate PASTAZZO_ACCOUNT_PASSWORD --json
```

Automation should consume it through `hush run --redact`, passing it to the client's `--password-file` through a pipe. Invitations can likewise be passed through `join --invite-file`; keep secrets out of process arguments and logs.

Back up the server's keys separately through `hush box seal --file`, and retain a consistent SQLite backup of its database. Server keys alone do not recover account registrations. Device recovery documents use the client's existing private state format, including the original device identity; do not install the same device identity on two active machines.

There is no in-place account password reset or coordinated key rotation endpoint. To replace every credential, prepare a fresh server data directory and invite-only account, verify its fingerprint out of band, register the trusted devices again, and retire the previous server state. Preserve local history and the old encrypted recovery material before switching. Old clients must refuse the new server fingerprint until deliberately re-associated. The regular revoke command does not erase keys already held by a revoked device; see [SECURITY.md](SECURITY.md).
