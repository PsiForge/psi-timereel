# 0002. 0002 Use egui (eframe) for the viewer instead of Tauri

## Context
The viewer shows decrypted images and must never write them to disk. It must scroll at 30 fps or more, and lock reliably by dropping all decrypted data. The spec suggested Tauri, but did not require it.

## Options
- **Tauri (webview):** a familiar web UI. But images have to reach the WebKitGTK webview through a custom protocol, and WebKit has its own memory and disk caches. Proving that no decrypted image ever lands in `~/.cache` is hard. It also adds a second runtime (JavaScript), and memory use is higher.
- **GTK4 (gtk4-rs):** native and mature, but texture handling and custom virtualised strips are more work, and the bindings are verbose.
- **iced:** pure Rust, but there are fewer ready-made widgets (date picker) and fewer UI test tools.
- **egui / eframe:** pure Rust and immediate mode. Images are GPU textures that we control directly. The `egui_extras` date picker and the `egui_kittest` headless UI tests exist. Persistence can be turned off.

## Decision
egui through eframe with the glow backend and the `persistence` feature disabled. The decrypted RGBA buffers and textures live only in the process.

## Consequences
The look is less native than GTK. The 'no plaintext on disk' rule is easier to reason about and to test. The UI logic sits in `timereel-timeline` and is unit-tested without a GUI. Flows are tested with egui_kittest. Raising an existing window on Wayland is best effort.
