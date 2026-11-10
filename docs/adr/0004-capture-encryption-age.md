# 0004. 0004 Encrypt captures with the age format (rage crate)

## Context
Every image, thumbnail and metadata record must be encrypted with a fresh random key wrapped with a public key, so that the agent never holds a secret. The spec forbids home-made crypto.

## Options
- **Build X25519 + HKDF + ChaCha20-Poly1305 ourselves from RustCrypto parts:** this is home-made protocol design. Rejected.
- **HPKE (`hpke` crate):** a standard, but low-level; we would still design the framing and streaming.
- **libsodium sealed boxes (`sodiumoxide`):** the crate is deprecated and unmaintained.
- **age (`age` crate by str4d):** a widely used, audited-by-community format with X25519 recipients, a fresh file key per file, and authenticated streaming (ChaCha20-Poly1305 STREAM).

## Decision
age with X25519 recipients. Each of the three parts of a capture is a separate age file, so each has its own file key. The parts are framed in one container file (ADR 0006).

## Consequences
The agent needs only the `age1...` public key. Tampering and truncation are detected by age. Interop is a bonus: in an emergency, the parts can be opened with the `rage` CLI and the private key. Decryption cost is one X25519 operation per part, well under 1 ms.
