# 0006. 0006 Store one container file per capture, with the time in plain text in the file name

## Context
Retention runs in the agent without the private key. The viewer needs a fast time index and fast thumbnail reads. Writes must be atomic. Plain timestamps reveal when the owner worked.

## Options
- **Three files per capture:** three renames, so it is not atomic as a unit.
- **SQLite database with encrypted blobs:** one big file; the WAL and journal hold data and are harder to reason about; retention needs VACUUM to give space back.
- **Encrypted index with no plain times:** retention would need the private key, which the agent must never have.
- **One `.trc` container per capture, named by its UTC time, in UTC day folders:** the thumbnail comes first in the file.

## Decision
The container file approach. It is written to `.<name>.partial` in the same folder, fsynced and renamed. Listing ignores `.partial` files, and the agent removes them at start. Capture times are deliberately left unencrypted; file times would show them anyway. All other metadata is encrypted.

## Consequences
Retention is a simple delete based on the file name. The index costs one directory listing per day. The architecture doc states which data is not encrypted. If the owner later wants hidden times, that needs a new design (for example, coarse buckets).
