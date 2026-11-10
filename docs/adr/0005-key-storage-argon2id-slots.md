# 0005. 0005 Store the private key in password and recovery slots (Argon2id, XChaCha20-Poly1305)

## Context
The private key must be protected by a password with Argon2id. A recovery key must also unlock it. Changing the password must not touch the captures. age's own passphrase mode uses scrypt, not Argon2id.

## Options
- **age scrypt recipient:** not Argon2id, so the spec rejects it.
- **Encrypt captures to two keys (password and recovery):** more files would have to change on a password change, and the agent would need two keys.
- **One key pair, wrapped twice (slots), similar to LUKS keyslots:**
  - password slot = Argon2id key derivation + XChaCha20-Poly1305 (`argon2`, `chacha20poly1305` crates);
  - recovery slot = HKDF-SHA256 from a 256-bit random recovery key + XChaCha20-Poly1305.

## Decision
The slot design, stored in `keyring.json` with versioned fields.

- Argon2id parameters: m = 256 MiB, t = 3, p = 4. They are stored in the slot, so they can be raised later. This takes about 1 s on a laptop, once per unlock.
- The recovery key is shown once as a Bech32 string (built-in checksum against typos).
- The associated data binds the slot name and the public key.
- A password change rewrites only the password slot, with an atomic replace.

## Consequences
These are standard primitives from maintained crates, used in a common, simple construction (a password-wrapped key). Old copies of the key file in backups still open with the old password; this is documented. Tests use low Argon2 parameters, which are passed in explicitly and are never used in production.
