# TimeReel architecture

Status: proposed at design gate G1. The Planner keeps this file current.

## 1. Overview

TimeReel takes a small screenshot every few minutes, encrypts it in memory and stores it under the user's home directory. A viewer lets the owner unlock the data and scroll back through the day. There is no network access, no server and no account. No program runs as root.

The project is one Rust Cargo workspace with three programs:

| Program | How it runs | Job |
|---|---|---|
| `timereel-agent` | systemd user service, started with the graphical session | capture, encrypt, store, retention. It knows only the public key. |
| `timereel-tray` | XDG autostart at login | tray icon with state, pause/resume, open, quit |
| `timereel` | on demand, from the tray or the app menu | first set-up, unlock, timeline, search, settings. Also has the `timereel purge-data` command. |

```
login -> systemd --user -> timereel-agent --(encrypted .trc files)--> ~/.local/share/timereel/captures/
                               ^  |
              D-Bus calls      |  | PropertiesChanged (state)
                               |  v
login -> XDG autostart -> timereel-tray --(start / activate)--> timereel (viewer)
                                                                 reads .trc files, decrypts in memory only
```

## 2. Components

### 2.1 Shared library crates (`crates/`)

- `timereel-config`: XDG paths, the `config.toml` schema, defaults, validation, and an atomic, file-locked read-modify-write.
- `timereel-crypto`: the key file (create; unlock with password or recovery key; change password), capture sealing and opening with age, and secret types that are zeroized on drop.
- `timereel-store`: on-disk layout, atomic writes, listing captures by time, reading single parts, and retention.
- `timereel-image`: stitching monitors, scaling, WebP/JPEG encode and decode, and thumbnails. All of it works in memory.
- `timereel-capture`: the traits `ScreenSource`, `ActiveWindowSource` and `LockSource`, with implementations for X11, the Wayland portal, the GNOME extension and logind/ScreenSaver. Also session-type detection and fakes for tests.
- `timereel-ipc`: the D-Bus interface definitions (zbus), the agent state enum, and client proxies.
- `timereel-timeline`: viewer logic with no GUI: the time index, queries, jumps, gaps, search, the background image loader and the byte-limited caches.
- `timereel-bench`: generates test data and runs the CI benchmark.

### 2.2 Agent (`timereel-agent`)

The agent runs on a single-threaded tokio runtime. It sleeps until the next tick and does no work in between. At each tick it runs one capture cycle:

1. Read the config again (one small file). If `paused`, skip with reason `paused`.
2. Ask the `LockSource`. If the screen is locked, skip with reason `locked`.
3. Ask the `ActiveWindowSource`.
   - If the app name matches the excluded list (case-insensitive, whole name), skip with reason `excluded`.
   - If the app is unknown and `capture_when_app_unknown = false` (the default), skip with reason `app-unknown`.
   - No focused window at all (for example the desktop) counts as known and is captured with app `(none)`.
4. Capture all monitors with the `ScreenSource`. Stitch them following the monitor layout, scale to the configured width, and make a thumbnail.
5. Ask the `ActiveWindowSource` again. If an excluded app became active during the capture, throw the frame away.
6. Encode the image and thumbnail, build the metadata JSON, encrypt all three parts with the public key, and write the container atomically.

Scheduling uses wall-clock times: next tick = last tick + interval. The agent also listens to logind `PrepareForSleep`, so it reschedules after a suspend. Retention runs once at start and then every 24 hours.

The agent writes logs to the user journal (`tracing` + `tracing-journald`). Logs contain reasons and error kinds only. They never contain window titles, app names, image data or keys.

### 2.3 Tray (`timereel-tray`)

The tray is a small process using `ksni` (StatusNotifierItem over D-Bus). It works on KDE, and on Ubuntu GNOME through the AppIndicator extension that Ubuntu ships. It follows the agent through D-Bus `PropertiesChanged` and `NameOwnerChanged`.

- Icons and states: recording, paused, error. The `needs_permission` state uses the error icon with its own reason text.
- Menu: Pause/Resume, Open TimeReel, Grant capture permission (shown only when needed), Quit.
- Pause and resume are D-Bus calls to the agent, so they need no unlock.
- Quit asks the viewer to quit and then exits the tray. The agent keeps running.
- If the agent is not on the bus, the tray shows the error state with the reason 'Agent not running'.

### 2.4 Viewer (`timereel`)

The viewer uses `eframe`/`egui` with the glow backend. The eframe `persistence` feature is off, so no UI state is written to disk. The viewer claims the D-Bus name `org.timereel.Viewer1` (methods `Activate`, `Quit`), so only one instance runs; the tray uses this to bring an open viewer to the front.

Screens:

- **Set-up** runs when there is no key file.
- **Lock screen** is shown at start, after idle time (default 10 minutes), and when the desktop session locks.
- **Timeline**: a thumbnail strip, a large view, jump controls and a search box.
- **Settings.**

All decrypted data sits in a `Session` object. Locking drops the session: it zeroizes the private key, clears the image caches and metadata, and frees the GPU textures. Closing the viewer window ends the process, so it also drops everything.

### 2.5 GNOME Shell extension (`extras/gnome-shell-extension/`)

This is a tiny extension (`timereel-focus@timereel.org`). It exports `org.timereel.Focus1.GetFocus()` on the session bus and returns the focused window's app name, WM class and title. It is the only way to read the active window on GNOME Wayland (see ADR 0009). On other Wayland compositors the active app is reported as unknown.

## 3. Communication

### 3.1 Agent D-Bus API (session bus)

The agent owns the name `org.timereel.Agent1` and serves the object `/org/timereel/Agent1`.

| Member | Kind | Meaning |
|---|---|---|
| `State` | property `s` | `recording`, `paused`, `error`, `needs_permission` or `not_set_up` |
| `Reason` | property `s` | short reason for users, for example 'Disk full' or 'Screen capture permission denied' |
| `LastCaptureTime` | property `x` | Unix time in ms, or 0 |
| `LastSkipReason` | property `s` | `paused`, `locked`, `excluded` or `app-unknown`. It never holds content. |
| `Pause()` / `Resume()` | method | Saves `paused` to the config and takes effect at once. A pending capture is cancelled. |
| `ReloadConfig()` | method | Re-reads and validates the config and reschedules the next tick. Returns an error text if the config is invalid. |
| `RunRetention()` | method `-> u` | Runs retention now and returns the number of captures deleted. |
| `RetryPermission()` | method | On Wayland, asks for portal permission again. |

The API never sends titles, images or keys. Any process of the same user can call it. That is acceptable: such a process can already read the user's screen and files.

### 3.2 Config file

`$XDG_CONFIG_HOME/timereel/config.toml` (mode 0600). Both the viewer and the agent change it only through `timereel_config::update`. That function takes an exclusive `flock` on `config.lock`, reads, applies the change, validates, writes a temp file in the same directory, fsyncs, renames and fsyncs the directory. The viewer writes the settings; the agent writes only `paused`. After a save, the viewer calls `ReloadConfig()`. The agent also reads the file at every tick, so changes apply even if the call was missed.

```toml
interval_minutes = 5
width_px = 1280
format = 'webp'          # or 'jpeg'
quality = 50
thumbnail_width_px = 240
retention_days = 30
excluded_apps = []       # e.g. ['KeePassXC', '1Password', 'Bitwarden']
capture_when_app_unknown = false
paused = false
ui_autolock_minutes = 10
public_key = 'age1...'
```

For tests, environment variables override the paths: `TIMEREEL_DATA_DIR`, `TIMEREEL_CONFIG_DIR` and `TIMEREEL_STATE_DIR`.

## 4. Data and file layout

All directories are mode 0700 and all files are mode 0600.

The data directory is `$TIMEREEL_DATA_DIR` if set (tests), otherwise `$XDG_DATA_HOME/timereel`, which is `~/.local/share/timereel`. `timereel-store` owns the `captures/` folder inside it; nothing is written outside the data directory (no temp files in `/tmp`) and nothing unencrypted is written anywhere. Day folders and file names use UTC, and the file name is `YYYYMMDDTHHMMSSmmmZ` (milliseconds). A capture is written as `.<name>.partial` in its day folder, fsynced, renamed to `<name>.trc`, and the folder is fsynced. Listing uses file names only and ignores partial files, symlinks and unknown names. The agent removes leftover `.partial` files at start (`Store::cleanup_partials`).

```
~/.local/share/timereel/            ($XDG_DATA_HOME)
  keyring.json                      key file: public key + encrypted private key slots
  captures/
    2025-05-14/                     UTC day
      20250514T093000123Z.trc       one capture = one encrypted container
      .20250514T093500456Z.partial  write in progress (ignored, cleaned at start)
~/.config/timereel/                 ($XDG_CONFIG_HOME)
  config.toml, config.lock
~/.local/state/timereel/            ($XDG_STATE_HOME)
  portal-restore-token              Wayland ScreenCast restore token
```

### 4.1 Capture container (`.trc`)

```
'TRC1' | version u8 | thumb_len u32 | meta_len u32 | image_len u32 | thumb age blob | meta age blob | image age blob
```

- All integers are little-endian.
- Each part is its own age file encrypted to the X25519 public key, so each part has a fresh random file key.
- The framing is not cryptography. It only lets the viewer read the thumbnail (a few KB at the start of the file) without reading the full image.
- The reader enforces size limits (thumb 1 MB, meta 64 KB, image 32 MB) before it allocates memory.
- Each plaintext part starts with a part tag and a random 16-byte capture id. The reader checks that the tag is right and that all three ids match. It also checks that the metadata timestamp matches the file name. This detects parts or files that were swapped.

The metadata is JSON inside the encryption: `version, capture_id, taken_at_ms (UTC), tz_offset_min, app_name, window_title, width, height, format`. A field that cannot be read is stored as `unknown`.

### 4.2 What is NOT encrypted

- **Capture times.** They are in the file names and in the file times, so the times when the owner worked are visible without the password. This is needed so that retention and the time index work without the private key (ADR 0006).
- File sizes and the number of captures.
- The public key.
- `config.toml`, including the excluded app list.
- The portal restore token.

Everything else (images, thumbnails, app names, window titles) is encrypted before it reaches the disk.

## 5. Capture

- **Session detection** happens at start. `XDG_SESSION_TYPE` is checked first, then `WAYLAND_DISPLAY` and `DISPLAY`. If nothing usable is found, the agent goes to the error state with the reason 'No display'.
- **X11** (ADR 0007): `x11rb`. Monitor rectangles come from RandR `GetMonitors`. Each monitor is grabbed with `GetImage` in horizontal bands and scaled straight into the output canvas, so the peak memory stays at about one band. The active window comes from EWMH: `_NET_ACTIVE_WINDOW`, `WM_CLASS` gives the app name, and `_NET_WM_NAME` / `WM_NAME` gives the title.
- **Wayland** (ADR 0008): the ScreenCast portal (version 4 or later) through `ashpd`, with frames read through `pipewire`. Each capture opens a short session with the stored restore token (`persist_mode = 2`), grabs one frame per monitor stream, and closes the session. The new restore token is saved after every start. If there is no token, or the token is rejected, the portal dialog is shown once and the state is `needs_permission`. If the user declines, the agent does not ask again until the user chooses 'Grant capture permission' in the tray. The Screenshot portal is not used because it saves a PNG file to disk. A short screen-sharing indicator may flash at each capture; this is accepted.
- **Active window on Wayland** (ADR 0009): the GNOME extension. Otherwise the app is `unknown`. With the safe default, unknown means the capture is skipped. Repeated `app-unknown` skips on Wayland raise the error state with the reason 'Active app unknown: enable the TimeReel GNOME extension or allow capture when unknown'.
- **Lock state:** logind `LockedHint` for the current session, with `org.gnome.ScreenSaver` and `org.freedesktop.ScreenSaver` `GetActive` as fallbacks. If no source answers, the agent logs a warning once and treats the screen as unlocked. A wrong guess here at worst captures a lock screen.
- **Idle skip:** by the owner's decision on story #12, v1 does NOT skip unchanged screens. Every interval produces a capture unless the screen is locked, recording is paused or an excluded app is active. This differs from the original spec text on purpose.

### 5.1 Image defaults and size

Defaults are WebP at quality 50 and 1280 px wide, with a thumbnail 240 px wide at quality 40 (ADR 0011).

A normal work month at a 5-minute interval is about 2,000 captures (8 h x 12 per hour x 21 days). The target of 50-100 MB therefore means 25-50 KB per capture. A typical 1280x720 desktop at WebP quality 50 is about 30-45 KB, and a thumbnail is 3-5 KB. Wide stitched layouts give smaller images (for example 1280x360).

T08 checks the sizes with committed fixture screenshots. The owner checks real use at the merge gate. If the result is too large, the default quality drops to 40.

## 6. Encryption and keys

See ADRs 0004 and 0005. Only maintained RustCrypto and age crates are used. There is no home-made cryptography.

- **Key pair:** an age X25519 identity, generated at set-up. The agent gets only the `age1...` public key, through `config.toml`.
- **`keyring.json`**: `{version, public_key, password_slot, recovery_slot}`.
  - `password_slot`: Argon2id (`argon2` crate) with m = 256 MiB, t = 3, p = 4 and a 16-byte salt. The parameters are stored in the slot. The derived 32-byte key encrypts the identity with XChaCha20-Poly1305 (`chacha20poly1305` crate) and a random 24-byte nonce. The associated data binds the slot name and the public key.
  - `recovery_slot`: the recovery key is 32 random bytes from the OS RNG. It is shown once as a Bech32 string with a checksum, in groups. HKDF-SHA256 with a salt turns it into a wrapping key, and XChaCha20-Poly1305 encrypts the same identity. Argon2 is not needed because the recovery key has 256 bits of entropy.
- **Unlock:** Argon2id runs once per unlock, on a worker thread. The opened identity must match `public_key`. Any failure gives the same message: 'Password or recovery key is not correct'.
- **Change password:** only `password_slot` is rewritten, with a new salt and nonce. The file is replaced atomically (temp file in the same directory, fsync, rename, fsync of the directory). Captures and the recovery slot are not touched.
- **Captures:** age encryption to the public key, one age file per part (section 4.1).
- **Secrets in memory:** `secrecy` and `zeroize`. The agent and the viewer call `prctl(PR_SET_DUMPABLE, 0)`, which turns off core dumps and blocks ptrace by other non-root processes.

## 7. Security model

**Protects against:**

- someone who has the disk, a backup or file access while the viewer is locked or closed;
- someone at the desk while the viewer is locked;
- leaks through temp or cache files.

The viewer writes nothing derived from decrypted data to disk: no thumbnail cache, no egui persistence, no clipboard copy of the recovery key, and no titles in logs.

**Does not protect against:**

- malware running as the same user while the session is unlocked (it can capture the screen itself);
- root;
- swap: decrypted images live in RAM and could be swapped out. The README recommends full-disk encryption or encrypted swap;
- capture times (see 4.2).

Old copies of `keyring.json` in backups still open with the old password. The README warns about this.

**Privacy defaults:**

- Excluded apps are checked before and after each capture.
- An unknown active app means no capture, unless the owner allows it.
- No capture while the screen is locked or recording is paused.

## 8. Viewer performance

- **Index:** at unlock, the capture file names are listed (about 8,600 at most for 30 days). This needs no decryption. The listing is refreshed every minute while unlocked.
- **Metadata:** all metadata is decrypted in the background after unlock (about 1 ms each) into an in-memory list used for the large view labels and for search.
- **Loader:** min(4, CPU count - 1) worker threads take jobs from a priority queue ordered by distance from the current position. A generation counter drops stale jobs after a jump. Results go back over a channel and ask egui to repaint. The UI thread never decrypts or decodes.
- **Caches:** byte-limited LRU caches of decoded RGBA images: 256 MB for full images and 64 MB for thumbnails. The least recently used entries are evicted first. Both are cleared on lock.
- **Prefetch:** 3 full images and 40 thumbnails on each side of the current position. Only visible thumbnails are requested for the strip. While a full image is loading, its thumbnail is shown scaled up.
- **Budgets** (ADR 0013): CI checks decrypt + decode time per full image and per thumbnail, and the cold jump latency through the loader.

## 9. Agent resource budget

The target is under 30 MB memory and under 1 % average CPU. The agent sleeps between ticks. Large buffers are freed after each capture; glibc returns large mmap'd blocks to the OS.

The X11 source works in bands, so it never holds a full raw multi-monitor frame. PipeWire is loaded only on Wayland, and each portal session is short. The systemd unit sets `MemoryHigh=64M` as a guard. The owner measures real use over a day at the merge gate.

## 10. States and errors

The agent maps every failure to an error kind with a short reason and a journal entry:

| Error kind | Example reason |
|---|---|
| `NoDisplay` | No display found |
| `NoPermission` | Screen capture permission denied |
| `Portal` | Screen capture portal error |
| `DiskFull` | Disk full (ENOSPC) |
| `Encryption` | Encryption failed |
| `Storage` | Could not save the screenshot |
| `NotSetUp` | No public key: run set-up in TimeReel |

The state stays `error` until the next successful capture; then it goes back to `recording` on its own. Errors are never swallowed.

## 11. Testing strategy

**CI (GitHub Actions, ubuntu-latest):**

- `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`, `cargo deny check`.
- **Unit tests:** crypto (round trip, wrong key, tampering, truncation, wrong password, recovery key, password change), store (write/read/list, atomic writes, no plaintext files, checked with a `notify` watcher on the data dir and TMPDIR), retention boundaries, skip logic with fake lock/pause/window sources, image stitching and sizes, timeline queries and jumps (including empty days and DST), search matching, cache eviction and the loader.
- **Integration under Xvfb** (`xvfb-run`): real X11 capture with two virtual RandR monitors, and active-window reading.
- **Integration under `dbus-run-session`:** the agent D-Bus API, tray state following, lock detection against fake services, and the GNOME extension client against a fake service.
- **UI flow tests** with `egui_kittest`, run headless: set-up, lock screen messages, arrow keys, settings validation.
- **Benchmark job** (release build): decrypt + decode per full image and per thumbnail, and cold jump latency through the loader. It prints a table and fails on the thresholds in ADR 0013.
- **Packaging job:** builds the `.deb`, runs lintian, installs it on the runner and checks the files and the unit.

**Human checks on a real machine at the merge gate:**

- capture on X11 with real monitors;
- Wayland portal permission and restore token (later, after the move);
- the GNOME extension;
- the tray icon on the owner's desktop;
- how unlocking and scrolling feel (under 200 ms per jump, 30 fps or more);
- agent memory and CPU over a full day;
- storage per month.

## 12. Decisions

See `docs/adr/`:

- 0001 Rust workspace
- 0002 egui viewer
- 0003 separate tray with ksni
- 0004 age for captures
- 0005 key file with Argon2id and recovery slots
- 0006 container file with plain timestamps
- 0007 X11 via x11rb
- 0008 Wayland ScreenCast portal
- 0009 GNOME extension for the active window
- 0010 D-Bus plus config file
- 0011 WebP via libwebp
- 0012 .deb via cargo-deb
- 0013 benchmark thresholds

## 13. Repository layout

```
Cargo.toml (workspace)    rust-toolchain.toml    deny.toml
crates/timereel-{config,crypto,store,image,capture,ipc,timeline,bench}/
apps/timereel-agent/  apps/timereel-tray/  apps/timereel/
extras/gnome-shell-extension/timereel-focus@timereel.org/
packaging/ (systemd user unit, .desktop files, icons)
tests/fixtures/ (synthetic and team-made screenshots, no personal data)
docs/architecture.md  docs/adr/
.github/workflows/ci.yml
```
