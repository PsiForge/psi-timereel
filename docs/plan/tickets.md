# Ticket plan

The machine-readable version is `tickets.json`; edit that file if you change tickets by hand.

| Id | Title | Size | Risk | Depends on | Stories |
|---|---|---|---|---|---|
| T01 | Set up the Rust workspace, lint, tests and CI | S | low | — |  |
| T02 | Add config schema, XDG paths and atomic locked updates | M | low | T01 | #8 #23 |
| T03 | Create the key file and unlock with password or recovery key | L | high | T02 | #3 #6 |
| T04 | Seal and open captures with age in a container format | M | high | T03 | #4 |
| T05 | Change the password by rewriting only the password slot | S | high | T03, T04 | #7 |
| T06 | Store encrypted captures with atomic writes and time listing | M | high | T02, T04 | #5 |
| T07 | Implement retention that deletes old captures inside the data dir | S | high | T06 | #14 |
| T08 | Build the in-memory image pipeline: stitch, scale, encode, decode | M | low | T01 | #9 |
| T09 | Implement the agent capture cycle with skip and error logic | M | high | T02, T06, T08 | #12 #13 #15 #8 #11 |
| T10 | Run the agent as a D-Bus service with live config, scheduling and retention | M | high | T07, T09 | #8 #12 #14 #15 |
| T11 | Capture all screens and the active window on X11 | M | low | T09, T10 | #9 #11 |
| T12 | Detect a locked screen through logind and ScreenSaver D-Bus | S | low | T10 | #12 |
| T13 | Capture screens on Wayland through the ScreenCast portal with a restore token | L | high | T11 | #10 |
| T14 | Read the active window on GNOME Wayland through a bundled extension | M | high | T13, T12 | #11 #13 |
| T15 | Add the tray process with state icon and controls | M | low | T10 | #16 #15 |
| T16 | Build the viewer shell and the first-run set-up | M | high | T03, T15 | #3 #8 |
| T17 | Add the lock screen, unlock, auto-lock and the Session object | M | high | T05, T12, T16 | #17 #6 #7 |
| T18 | Implement the timeline index, queries, jumps and gaps | M | low | T06 | #18 #19 |
| T19 | Add the background image loader and byte-limited memory caches | M | high | T08, T17, T18 | #21 |
| T20 | Build the timeline view with thumbnail strip and large view | M | low | T19 | #18 #21 |
| T21 | Add jump to date/time and quick-jump buttons | S | low | T20 | #19 |
| T22 | Add in-memory search by app name or window title | M | high | T20 | #20 |
| T23 | Build the Settings screen with validation, apply, password change and retention confirmation | M | high | T17, T10, T07 | #23 #7 #13 #14 |
| T24 | Add the decrypt and decode benchmark to CI | S | low | T19 | #22 |
| T25 | Package as a .deb and add the purge-data command | M | high | T14, T15, T16 | #24 |
| T26 | Write the README for install, set-up, data and removal | S | low | T25 | #25 |

## T01 — Set up the Rust workspace, lint, tests and CI

Create the Cargo workspace from docs/architecture.md section 13:
- library crates `timereel-config`, `-crypto`, `-store`, `-image`, `-capture`, `-ipc`, `-timeline` and `-bench`, each with a placeholder test;
- binaries `apps/timereel-agent`, `apps/timereel-tray` and `apps/timereel`, which print their version;
- `rust-toolchain.toml` pinning stable Rust, `rustfmt.toml`, workspace clippy lints, and `deny.toml` for cargo-deny (advisories, licences, bans).

Add `.github/workflows/ci.yml` for ubuntu-latest. It installs the system packages (pkg-config, libclang-dev, libpipewire-0.3-dev, libxkbcommon-dev, xvfb, dbus), caches cargo, and runs `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` and `cargo deny check`. Add a README stub with build and test commands.

**Acceptance criteria**
- CI runs on push and pull_request on ubuntu-latest and is green.
- CI fails when formatting, clippy, a test or cargo-deny fails (shown by the workflow steps).
- `cargo build --workspace` builds all crates and three binaries.
- README stub explains how to build and run the tests.

## T02 — Add config schema, XDG paths and atomic locked updates

In `timereel-config`, implement:
- XDG data, config and state paths, with overrides `TIMEREEL_DATA_DIR`, `TIMEREEL_CONFIG_DIR` and `TIMEREEL_STATE_DIR`;
- directory creation with mode 0700 and files with mode 0600;
- the `Config` struct with the defaults in architecture section 3.2;
- `validate()`, which returns per-field errors with clear messages. Ranges: interval 1-60 min, width 320-7680, quality 1-100, thumbnail width 120-640, retention 1-3650 days, format webp|jpeg, auto-lock 1-240 min, excluded apps trimmed and non-empty, at most 50;
- `load()`, which returns the defaults with no public key when the file is missing;
- `update(|cfg| ...)`, which takes a flock on `config.lock` (fs4 crate), reads, modifies, validates, writes a temp file, fsyncs, renames and fsyncs the directory.

**Acceptance criteria**
- Unit test: defaults are 5 min, 1280 px, webp, quality 50, thumbnail 240, 30 days, empty excluded list, capture_when_app_unknown false, paused false, auto-lock 10 min.
- Unit tests: each out-of-range value gives a field error with a readable message.
- Unit test: two threads doing 100 updates each to different fields lose no change.
- Unit test: a failure injected before the rename leaves the old config intact.
- Unit test: created directories are 0700 and the config file is 0600.
- Environment variable path overrides are used by the tests.

## T03 — Create the key file and unlock with password or recovery key

In `timereel-crypto`, implement the key file from ADR 0005:
- generate an age X25519 identity;
- password slot: Argon2id (m = 256 MiB, t = 3, p = 4, stored in the slot), wrapping with XChaCha20-Poly1305, associated data binding the slot name and public key;
- recovery slot: 32 random OS bytes, HKDF-SHA256, XChaCha20-Poly1305. The recovery key is shown as a Bech32 string in groups;
- `create(password, params) -> (Keyring, RecoveryKey, UnlockedKey)`, `unlock_with_password`, `unlock_with_recovery_key`, `load`, and `save` (atomic, mode 0600);
- after unlock, check that the identity matches the stored public key;
- one error `WrongSecret` for every unlock failure and a separate `Corrupt` error for a malformed file;
- secrets held in `secrecy`/`zeroize` types;
- a password policy helper: shorter than 12 characters gives a warning, not a block.

Crates: age, argon2, chacha20poly1305, hkdf, sha2, bech32, zeroize, secrecy, getrandom. Tests pass in low Argon2 parameters explicitly.

**Acceptance criteria**
- Unit test: unlock with the right password returns an identity matching the public key.
- Unit test: a wrong password returns WrongSecret.
- Unit test: the recovery key unlocks; a wrong or typo'd recovery key returns WrongSecret (or a checksum error with the same user-facing message).
- Unit test: the key file contains no `AGE-SECRET-KEY` text and no recovery key; the recovery key is never written to disk.
- Unit test: the Argon2 parameters and salt are stored in the file, and production defaults are m = 262144 KiB, t = 3, p = 4.
- Unit test: Argon2 runs exactly once per unlock (counting hook).
- Unit test: the key file is 0600; a corrupt file gives Corrupt, not a panic.
- cargo-deny passes with the new crates.

## T04 — Seal and open captures with age in a container format

In `timereel-crypto`, implement `seal_capture(public_key, thumb, meta, image) -> Vec<u8>` and `open_part(identity, bytes, Part)` using the `.trc` container in architecture section 4.1.
- Three separate age encryptions to the X25519 recipient, all in memory.
- Each plaintext part has a part tag and a 16-byte capture id prefix.
- The parser checks the magic, the version and the length limits (thumb 1 MB, meta 64 KB, image 32 MB) before allocating.
- The reader checks the tag and that the capture id is the same in all parts.
- Clear errors: `Truncated`, `Tampered`, `WrongKey`, `BadFormat`. The parser never panics.
- `open_part` reads only the bytes it needs, so a thumbnail can be read without reading the image.

**Acceptance criteria**
- Unit test: the round trip returns identical thumb, meta and image bytes.
- Unit test: opening with a different identity returns WrongKey.
- Unit test: flipping any byte in each part returns an error, not a panic.
- Unit test: truncating at many offsets returns Truncated or BadFormat, not a panic.
- Property test (proptest): random bytes never panic the parser.
- Unit test: a thumb part swapped in from another capture is detected (capture id mismatch).
- Unit test: sealing the same input twice gives different ciphertexts (fresh file keys).
- Sealing needs only the public key.

## T05 — Change the password by rewriting only the password slot

In `timereel-crypto`, add `change_password(keyring_path, unlocked, new_password)`.
- It builds a new password slot with a new salt and nonce, keeps the recovery slot and the public key unchanged, and replaces `keyring.json` atomically (temp file in the same directory, fsync, rename, fsync of the directory).
- Leftover temp files are ignored on load and cleaned up.
- Unlocking with either the current password or the recovery key is done by the caller using T03.

**Acceptance criteria**
- Unit test: after a change, the new password unlocks and the old password returns WrongSecret.
- Unit test: the recovery key still unlocks after the change.
- Unit test: a capture sealed with T04 before the change still opens after the change.
- Unit test: a failure injected before the rename leaves the old key file working; after the rename, the new file works.
- Unit test: the recovery slot bytes and the public key are byte-identical before and after the change.

## T06 — Store encrypted captures with atomic writes and time listing

In `timereel-store`:
- the layout in architecture section 4: `captures/YYYY-MM-DD/<UTC time>.trc`;
- `CaptureMeta` serialised as JSON, sealed with T04;
- `Store::write`: write `.<name>.partial`, fsync, rename, fsync the directory;
- `list(range)` sorted by time from file names only, ignoring partial files and unknown names;
- `read_thumb`, `read_meta` and `read_image`, each reading only what is needed;
- `cleanup_partials()` for agent start;
- the reader checks that the metadata timestamp matches the file name;
- no temp files outside the data directory and nothing unencrypted written anywhere.

**Acceptance criteria**
- Unit tests: write, then read meta, thumb and image round trip; list returns captures in time order across day folders and filters by range.
- Unit test: a write interrupted before the rename is not listed, and cleanup_partials removes it.
- Unit test: while 50 captures are written, a `notify` watcher on the data dir and TMPDIR sees only `.partial` and `.trc` files. Every file starts with `TRC1` and contains no WebP `RIFF`/`WEBP` header and no plaintext metadata strings.
- Unit test: a renamed file (timestamp mismatch) gives a clear error.
- Unit test: created folders are 0700 and files are 0600.
- The data location is documented in the crate docs and in architecture section 4.

## T07 — Implement retention that deletes old captures inside the data dir

In `timereel-store`, add `retention::run(store, now, days) -> Report{deleted, failed}` and `count_older_than(now, days)`.
- Retention uses file-name timestamps only (no key needed).
- A capture is deleted when its time is earlier than now minus D x 24 h; a capture exactly at the boundary is kept.
- Only regular `.trc` and `.partial` files under the canonical captures root are deleted. Symlinks are never followed (`symlink_metadata`), and the path is checked to be inside the root.
- Empty day folders are removed.
- Unparsable names are left alone and logged.
- A failed delete is logged with the path kind but no content, counted in the report, and retried at the next run.

**Acceptance criteria**
- Unit tests: boundary cases (just older, exactly at the boundary, newer) with a fixed clock.
- Unit test: a symlink in the captures dir pointing outside the dir is not followed, and the target survives.
- Unit test: files with unknown names are not deleted.
- Unit test: a read-only file gives failed = 1 and no panic; a later run after fixing the permissions deletes it.
- Unit test: count_older_than matches what run deletes.
- Unit test: empty day folders are removed.

## T08 — Build the in-memory image pipeline: stitch, scale, encode, decode

In `timereel-image`:
- `stitch(frames with positions)`: the canvas is the bounding box of the monitors, uncovered areas are black, and the image is scaled into the target width while stitching;
- scaling keeps the aspect ratio and never upscales (`fast_image_resize`);
- thumbnail of the configured width (default 240);
- encode WebP lossy through the `webp` crate (libwebp) or JPEG through `jpeg-encoder`;
- decode to RGBA (libwebp; `zune-jpeg` for JPEG).

The whole API works on in-memory buffers. Add 3-5 committed fixture screenshots of a clean desktop with open-source apps (no personal data) for the size check.

**Acceptance criteria**
- Unit tests: stitching of side-by-side, stacked, offset and different-height layouts gives the right canvas size and pixel placement.
- Unit test: output width is 1280 and the aspect ratio is kept within 1 px; a narrower source is not upscaled.
- Unit test: encode then decode returns the expected dimensions for WebP and JPEG.
- Fixture test: at defaults, the mean image is 45 KB or less and the mean thumbnail is 6 KB or less (sizes printed).
- The API takes and returns buffers only; no file I/O in the crate (checked by review and a test with a watched TMPDIR).

## T09 — Implement the agent capture cycle with skip and error logic

In `timereel-capture`, define these traits and fakes for tests:
- `ScreenSource` (returns monitor frames with positions);
- `ActiveWindowSource` (returns Known{app, title}, NoWindow or Unknown);
- `LockSource`.

In the agent, implement `CycleRunner::run_once(config, now) -> Outcome`, following architecture section 2.2:
- order: paused, then locked, then the excluded or unknown check (case-insensitive whole-name match; unknown is skipped unless `capture_when_app_unknown`), then capture, then the excluded check again, then the image pipeline, seal and store;
- map errors to kinds (NoDisplay, NoPermission, Portal, DiskFull on ENOSPC, Encryption, Storage, NotSetUp) with short user-facing reasons;
- a state machine: recording, paused, error, needs_permission, not_set_up. The state returns to recording after the next success;
- logging with `tracing`: skip reasons and error kinds only.

**Acceptance criteria**
- Unit tests with fakes: paused, locked, excluded and unknown (default) each skip and store nothing.
- Unit test: unknown with capture_when_app_unknown = true captures.
- Unit test: NoWindow captures with app '(none)'.
- Unit test: excluded-app matching is case-insensitive on the whole name.
- Unit test: when an excluded app becomes active during the capture, nothing is stored.
- Unit tests: each error kind sets the error state with the right reason; a following success returns the state to recording.
- Unit test: a captured log of a full run contains no window title or app name strings.
- Unit test: the stored capture's metadata contains the timestamp, app and title from the fake.

## T10 — Run the agent as a D-Bus service with live config, scheduling and retention

Agent `main` on a tokio current-thread runtime:
- serve `org.timereel.Agent1` with zbus (properties and methods in architecture section 3.1, including a stub `RetryPermission`);
- Pause and Resume save `paused` through the config update and take effect at once, cancelling a pending capture;
- ReloadConfig validates the config and reschedules;
- the config is also re-read at every tick;
- wall-clock scheduling with a reschedule on logind `PrepareForSleep`;
- retention at start and every 24 h; cleanup of partial files at start;
- logging to the journal with `tracing-journald`, falling back to stderr;
- `prctl(PR_SET_DUMPABLE, 0)`;
- state not_set_up when there is no public key;
- the ScreenSource and LockSource are injected; for now the real ones report 'unsupported session'.

Add `packaging/timereel-agent.service`: a user unit with PartOf/After/WantedBy graphical-session.target, `Restart=on-failure`, `NoNewPrivileges=yes`, `MemoryHigh=64M`, and no User= line.

**Acceptance criteria**
- Integration test under dbus-run-session with fake sources: State changes to paused within 1 s after Pause, and back after Resume; paused is saved in config.toml and survives an agent restart.
- Integration test: ReloadConfig with a changed interval reschedules (fake clock); an invalid config returns an error text.
- Integration test: RunRetention returns the number deleted.
- Unit test: with no public key, the state is not_set_up and no capture happens.
- Unit test: no ticks happen between scheduled times (fake clock counts wakeups).
- `systemd-analyze verify` on the unit file passes in CI.

## T11 — Capture all screens and the active window on X11

In `timereel-capture`:
- an X11 `ScreenSource` with x11rb: monitor layout from RandR `GetMonitors`; `GetImage` per monitor in bands of at most 256 rows, converted from BGRx and scaled into the canvas as each band arrives;
- an X11 `ActiveWindowSource` from EWMH: `_NET_ACTIVE_WINDOW`, `WM_CLASS` (class part, or the instance as fallback) for the app, `_NET_WM_NAME` with `WM_NAME` as fallback for the title. Window 0 or None means NoWindow; property errors mean Unknown;
- `detect_session()` from XDG_SESSION_TYPE, WAYLAND_DISPLAY and DISPLAY;
- wire the X11 sources into the agent. Wayland still reports 'not yet supported', and no session gives NoDisplay;
- a lost connection gives NoDisplay, and the agent reconnects at the next tick.

**Acceptance criteria**
- Integration test under xvfb-run at 2560x1080 with two virtual monitors (`xrandr --setmonitor`): the captured image is 1280 wide and the colour areas drawn by the test land in the right places.
- Integration test under Xvfb: a test window with a known WM_CLASS and title, set as `_NET_ACTIVE_WINDOW`, is read back correctly; an unset value gives NoWindow.
- Unit tests: detect_session for X11, Wayland and none.
- Unit test: band capture does not allocate more than one band of raw data (allocation counter on the band buffer).
- A manual check on a real X11 laptop is listed in the PR for the merge gate.

## T12 — Detect a locked screen through logind and ScreenSaver D-Bus

In `timereel-capture`, add a `LockSource`:
- primary source: logind `LockedHint` of the current session (`/org/freedesktop/login1/session/auto`);
- fallbacks: `org.gnome.ScreenSaver.GetActive` and `org.freedesktop.ScreenSaver.GetActive`;
- if none answers, log a warning once and treat the screen as unlocked (architecture section 5);
- bus connections are injectable, so tests can run fake services on a private bus;
- also expose a lock-changed signal stream for the viewer's lock-on-session-lock (T17);
- wire it into the agent.

**Acceptance criteria**
- Integration test under dbus-run-session with a fake logind service: LockedHint true skips the capture with reason locked; false captures.
- Integration test: when logind is missing, the fake GNOME ScreenSaver is used.
- Unit test: with no source answering, the warning is logged once and the result is unlocked.
- Integration test: the lock-changed stream emits on a fake signal.

## T13 — Capture screens on Wayland through the ScreenCast portal with a restore token

In `timereel-capture`, add a Wayland `ScreenSource` (ADR 0008):
- ashpd ScreenCast: create the session; select sources with Monitor type, multiple true, cursor hidden, persist_mode 2 and the stored restore token; start; read the streams' positions and sizes;
- open the PipeWire remote and use pipewire-rs to grab one SHM frame per stream (BGRx/RGBx, respecting strides);
- close the session and stitch with T08;
- a 10 s timeout per capture;
- save the new restore token after every start in `$XDG_STATE_HOME/timereel/portal-restore-token` (0600);
- with no token or a rejected token, prompt once and set needs_permission; on denial, set the state to 'Screen capture permission denied' and do not prompt again until `RetryPermission`;
- a portal version below 4 gives a clear Portal error;
- wire into the agent with session detection.

Put the portal behind a trait so the token and permission logic can be tested with fakes.

**Acceptance criteria**
- Unit tests with a fake portal: the stored token is passed in, and the new token replaces it after a start.
- Unit test with a fake portal: a rejected token leads to exactly one prompt and the needs_permission state.
- Unit test: after a denial, there are no further prompts until RetryPermission.
- Unit test: a portal version below 4 gives a clear error reason.
- Unit tests: frame conversion for BGRx and RGBx with padded strides; a two-stream layout is stitched correctly.
- Unit test: the token file is 0600, and no frame data is written to disk (watched TMPDIR and state dir during the fake capture).
- The PR lists a manual GNOME Wayland check (one prompt, then silent captures after a restart) for the merge gate.

## T14 — Read the active window on GNOME Wayland through a bundled extension

Add `extras/gnome-shell-extension/timereel-focus@timereel.org`: metadata.json for GNOME 45+ and an ES module that exports `org.timereel.Focus1.GetFocus() -> (ok, app_name, wm_class, title)` on the session bus, using `global.display.focus_window` and `Shell.WindowTracker`.

In `timereel-capture`, add a GNOME extension `ActiveWindowSource`. On Wayland it calls the extension and falls back to Unknown. In the agent, after three app-unknown skips in a row on Wayland, set the error state with the reason 'Active app unknown: enable the TimeReel GNOME extension or allow capture when unknown'. The state clears after the next capture.

**Acceptance criteria**
- Integration test under dbus-run-session with a fake Focus1 service: the app and title are returned.
- Integration test: a missing service gives Unknown; three Unknown skips set the error state with the documented reason.
- CI runs `node --check` on the extension's JavaScript.
- The extension never logs titles.
- The PR lists a manual check on GNOME Wayland for the merge gate.

## T15 — Add the tray process with state icon and controls

Write `timereel-tray` with ksni:
- icons for recording, paused and error (needs_permission uses the error icon); the tooltip shows the Reason;
- menu: Pause/Resume (the label follows the state), Open TimeReel (calls `org.timereel.Viewer1.Activate` if running, otherwise starts `timereel`), Grant capture permission (only when needs_permission; calls RetryPermission), Quit (calls Viewer1.Quit if present, then exits; the agent is untouched);
- follows the agent through PropertiesChanged and NameOwnerChanged; when the agent is not on the bus, show the error state with 'Agent not running';
- add `packaging/timereel-tray.desktop` for XDG autostart.

Keep the state-to-icon and state-to-menu mapping in pure functions.

**Acceptance criteria**
- Unit tests: the mapping from each agent state to icon, tooltip and menu items.
- Integration test under dbus-run-session with a fake agent: a State change is reflected in the tray model within 2 s.
- Integration test under dbus-run-session: the Pause menu action calls Pause on the fake agent.
- Integration test: when the fake agent leaves the bus, the tray shows the error with 'Agent not running'.
- Integration test: Quit does not call any agent method and the process exits.
- The PR lists a manual tray check on the owner's desktop for the merge gate.

## T16 — Build the viewer shell and the first-run set-up

Write the `timereel` binary with eframe (glow backend, `persistence` feature off):
- single instance through the D-Bus name `org.timereel.Viewer1` (Activate, Quit);
- `prctl(PR_SET_DUMPABLE, 0)`;
- start-up routing: no key file means set-up; otherwise the lock screen (a placeholder until T17).

Set-up screen:
- password and confirmation; a mismatch message; a warning below 12 characters;
- key creation (T03) on a worker thread with a progress indicator;
- the recovery key shown once in groups, with clear storage advice and no copy-to-clipboard button; the user must tick 'I have stored my recovery key' before continuing;
- write `public_key` to the config;
- run `systemctl --user enable --now timereel-agent.service` and show a clear message if it fails.

**Acceptance criteria**
- egui_kittest test: mismatched passwords show a message; a short password shows a warning but can continue.
- egui_kittest test: Continue is disabled until the recovery key box is ticked.
- Unit test: after set-up, the config holds the public key and the key file exists with mode 0600.
- Unit test: after set-up, the recovery key string appears in no file under the data, config, state, cache and temp directories.
- Unit test: a failing systemctl (fake command) shows an error message and does not crash.
- A second instance calls Activate on the first one and exits (integration test under dbus-run-session).

## T17 — Add the lock screen, unlock, auto-lock and the Session object

Viewer lock screen:
- a password field and a 'Use recovery key' toggle;
- unlock on a worker thread, so the UI never blocks;
- every failure shows 'Password or recovery key is not correct';
- after a recovery-key unlock, offer to set a new password (T05).

The `Session` object holds the UnlockedKey and has `on_lock` hooks that later caches and metadata register with. `lock()` drops and zeroizes everything and frees the textures.

Lock when:
- there has been no input for `ui_autolock_minutes` (default 10);
- the desktop session locks (T12 stream);
- the user chooses Lock.

Activate from the tray while locked shows and raises the lock screen.

**Acceptance criteria**
- Unit test with a fake clock: no input for the configured time locks the session; input resets the timer.
- Unit test: lock() runs all registered hooks and drops the key; a zeroize test-double confirms the secret buffer was wiped.
- egui_kittest test: a wrong password shows the generic message; the right password reaches the timeline placeholder.
- egui_kittest test: a recovery-key unlock shows the 'set a new password' offer, and setting it works (the new password unlocks afterwards).
- Integration test: a fake session-lock signal locks the viewer.
- Integration test: Activate while locked shows the lock screen.

## T18 — Implement the timeline index, queries, jumps and gaps

In `timereel-timeline`, with no GUI:
- `Index` built from `Store::list`, refreshed incrementally (re-list today's folder) every minute;
- queries: `next`, `prev`, `range(day in local time)`, `at_or_after(t)` falling back to the nearest capture when none follows, and `nearest(t)`;
- `quick_jump(kind, current, now, tz)` for today, yesterday, previous day, next day, -1 h and +1 h;
- `gaps(interval)` marking breaks longer than 1.5 x interval;
- local day boundaries with chrono, tested with chrono-tz.

**Acceptance criteria**
- Unit tests: next and prev at the start, middle and end of the index.
- Unit tests: range for a local day across the UTC day folders.
- Unit tests: at_or_after on an exact match, between captures, after the last capture (falls back to the nearest) and on an empty day.
- Unit tests: quick jumps, including a day with no captures and the Europe/Amsterdam DST change days.
- Unit test: gaps are detected at more than 1.5 x interval and not below.
- Unit test: an incremental refresh adds new captures without a full reload.

## T19 — Add the background image loader and byte-limited memory caches

In `timereel-timeline`:
- `ImageLoader` with min(4, CPU count - 1) worker threads and a priority queue ordered by distance from the current position;
- a generation counter that drops stale jobs after a jump;
- results are delivered over a channel and trigger a repaint callback;
- `ByteLru` caches of decoded RGBA images: full images 256 MB and thumbnails 64 MB; least recently used entries are evicted first;
- prefetch of 3 full images and 40 thumbnails on each side;
- decode or decrypt errors produce an error item, not a crash;
- register `clear` on the Session lock hook (T17);
- `request()` never blocks.

**Acceptance criteria**
- Unit test: eviction keeps the total bytes at or under the limit and removes the least recently used entry first.
- Unit test: the current position is served before its neighbours (ordered fake decoder).
- Unit test: after a jump, stale jobs are dropped and not delivered.
- Unit test: `request()` returns in under 1 ms while workers are busy.
- Unit test: lock clears both caches and pending jobs.
- Unit test: a tampered capture produces an error item and the loader keeps working.
- Unit test: nothing is written to disk during loading (watched data, temp and cache directories).

## T20 — Build the timeline view with thumbnail strip and large view

Viewer timeline screen in egui:
- a virtualised horizontal thumbnail strip that requests only visible thumbnails from T19;
- gap markers showing the gap length;
- a large view of the selected capture with local time, app name and window title from the decrypted metadata;
- while the full image loads, the thumbnail is shown scaled up;
- left/right arrow keys and the mouse wheel move the selection, and clicking a thumbnail selects it;
- repaints when images arrive.

Metadata for the labels is decrypted in the background after unlock and held in the Session, so it is cleared on lock.

**Acceptance criteria**
- egui_kittest test: arrow keys move the selection to the next and previous capture.
- egui_kittest test: the large view shows the time, app and title of the selected capture.
- egui_kittest test: a gap marker is shown between captures more than 1.5 x interval apart.
- Unit test: the strip requests thumbnails only for the visible range plus the prefetch.
- Unit test: a full image is requested only for the selected capture and its prefetch neighbours.
- Manual scrolling-feel check (30 fps or more over a full day) listed for the merge gate.

## T21 — Add jump to date/time and quick-jump buttons

Timeline controls:
- a date picker (`egui_extras` DatePickerButton) with hour and minute fields;
- a Go button that uses `at_or_after` with fallback to the nearest capture;
- quick-jump buttons: Today, Yesterday, Previous day, Next day, -1 h, +1 h;
- a message such as 'No captures on <date>; showing nearest' when needed;
- the jump target is given the highest loader priority and scrolling continues normally from it.

**Acceptance criteria**
- egui_kittest test: picking a date and time selects the expected capture.
- egui_kittest test: each quick-jump button selects the capture the T18 query returns.
- egui_kittest test: jumping to an empty day shows the 'showing nearest' message.
- egui_kittest test: after a jump, the arrow keys continue from the new position.

## T22 — Add in-memory search by app name or window title

In `timereel-timeline`, add a `MetaStore` filled by background decryption of all capture metadata after unlock, with progress shown in the UI. Searching works on the part already loaded. Filter: case-insensitive (Unicode lowercase) substring on the app name or window title. In the UI:
- a search box filters the timeline, and navigation moves within the matches;
- clearing the search returns to the full timeline with the same capture still selected.

The MetaStore is cleared on lock. Nothing decrypted is written to disk, and search terms are not logged.

**Acceptance criteria**
- Unit tests: matching is case-insensitive (including non-ASCII), matches in the app or the title, and an empty query matches everything.
- Unit test: the filtered next and prev skip non-matching captures.
- egui_kittest test: clearing the filter keeps the same capture selected in the full timeline.
- Unit test: lock empties the MetaStore.
- Unit test: a search run with a watched data, config, temp and cache directory writes no files.

## T23 — Build the Settings screen with validation, apply, password change and retention confirmation

Viewer Settings screen with these fields:
- interval, width, format, quality, thumbnail width, retention days;
- an excluded-apps list editor with example hints (KeePassXC, 1Password, Bitwarden);
- a 'capture when active app is unknown' toggle with a privacy warning;
- UI auto-lock minutes;
- a pause/resume toggle (D-Bus).

Behaviour:
- validation messages come from `timereel-config`;
- Save uses `config::update`, then calls ReloadConfig; if the agent is not running, show 'Saved; applies when the agent starts';
- lowering retention shows a confirmation with the number of captures that will be deleted (`count_older_than`), and after confirmation calls RunRetention;
- a Change password dialog takes the current password or the recovery key plus the new password twice, and uses T05.

**Acceptance criteria**
- egui_kittest test: invalid values show the field message and Save is refused.
- Integration test under dbus-run-session with a fake agent: Save writes config.toml and calls ReloadConfig.
- egui_kittest test: lowering retention shows a confirmation with the right count; Cancel deletes nothing and keeps the old value.
- Integration test: confirming the lower retention calls RunRetention.
- egui_kittest test: Change password with mismatched new passwords shows a message; with the right inputs, the new password unlocks afterwards.
- Unit test: with the agent missing, Save still writes the config and shows the message.

## T24 — Add the decrypt and decode benchmark to CI

`timereel-bench` binary:
- generate deterministic, desktop-like synthetic images (2560x1440 and 3840x1080 sources) through the real pipeline at defaults into a temp store;
- 20 warm-up runs and 200 measured runs of full-image decrypt + decode and thumbnail decrypt + decode;
- cold-jump latency through the T19 loader (empty cache: read + decrypt + decode);
- Argon2 unlock time, reported only;
- print a table (median, p95, max) to stdout;
- exit non-zero if any threshold in ADR 0013 is exceeded; environment variables can override the thresholds locally.

Add a `bench` job to ci.yml running `cargo run --release -p timereel-bench`.

**Acceptance criteria**
- The CI bench job runs in release mode and prints the results table in the log.
- The job fails if full-image median exceeds 40 ms or p95 exceeds 100 ms, if thumbnail median exceeds 5 ms or p95 exceeds 10 ms, or if cold jump p95 exceeds 120 ms.
- Unit test: the threshold check logic fails on an over-limit sample and passes on an under-limit sample.
- The test data is generated at default settings (1280 px, WebP quality 50, 240 px thumbnails).

## T25 — Package as a .deb and add the purge-data command

Add cargo-deb metadata that installs:
- the three binaries in /usr/bin;
- /usr/lib/systemd/user/timereel-agent.service;
- /etc/xdg/autostart/timereel-tray.desktop;
- /usr/share/applications/timereel.desktop;
- hicolor icons;
- the GNOME extension in /usr/share/gnome-shell/extensions/.

Declare the runtime dependencies (for example libpipewire-0.3-0). There are no maintainer scripts that start services.

Add `timereel purge-data`:
- list the exact folders it will delete and require typing DELETE;
- run `systemctl --user disable --now timereel-agent.service`;
- remove only the TimeReel data, config and state folders (paths checked to end in `timereel`);
- do nothing harmful if the folders are missing.

Add a CI job that builds the .deb, runs lintian, installs it with apt on the runner and checks the files.

**Acceptance criteria**
- The CI job builds the .deb; lintian has no errors.
- The CI job installs the .deb with apt and checks that all listed files exist.
- CI check: the installed unit is a user unit with no User= line, and no service is started by installing.
- Unit test: purge-data without typing DELETE deletes nothing.
- Unit test: with DELETE, purge-data removes only the three TimeReel folders under the test overrides; a sibling folder survives.
- Unit test: purge-data calls systemctl --user disable --now (fake command) and reports its errors clearly.
- Unit test: purge-data refuses a path that does not end in `timereel`.

## T26 — Write the README for install, set-up, data and removal

Replace the README stub with:
- what TimeReel is;
- requirements (Ubuntu/Debian; the AppIndicator extension on plain GNOME);
- install with the .deb;
- first set-up, including storing the recovery key safely;
- what happens without the password or the recovery key: the data cannot be recovered;
- the Wayland permission prompt and restore token, the brief sharing indicator, and enabling the GNOME extension;
- where data, config and state are stored;
- what is not encrypted (capture times, file sizes);
- swap and full-disk encryption advice, and the note about old key-file backups;
- how to build and run the tests and the benchmark;
- how to uninstall (`timereel purge-data`, then `sudo apt remove timereel`);
- links to docs/architecture.md and docs/adr/.

**Acceptance criteria**
- The README has sections for: about, install, first set-up, recovery key, Wayland permission and restore token, data locations, security notes, testing, uninstall and data deletion.
- It states plainly that without the password or the recovery key the data cannot be recovered.
- All commands in the README match the binary and package names (checked by review; a CI step greps the README for `timereel purge-data` and `timereel-agent.service`).
