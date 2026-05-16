# rust-desk-light

Lightweight Rust remote administration toolkit with GUI-based device management, remote desktop, file transfer, terminal access, camera viewing, and local-first control features.

`rust-desk-light` is organized as three small binaries plus shared protocol/assets crates:

- `rdl-server`: presence, session registration, routing, and relay.
- `rdl-client`: endpoint agent with GUI status, terminal fallback, and local capability handlers.
- `rdl-admin`: operator GUI for client discovery, commands, live control, and result viewing.
- `rdl_protocol`: shared binary transport and command model.
- `rust-desk-light-assets`: shared embedded GUI resources such as the app icon.

The project is intentionally compact. It focuses on practical remote assistance workflows, simple deployment, and a readable Rust codebase rather than a large enterprise remote management stack.

## Screenshots

TODO: Screenshots will be added later.

## Features

- Lightweight Rust admin/client/server workspace with a shared binary protocol and embedded GUI assets.
- TCP control channel with session tokens, typed messages, reconnect, heartbeat, and server-side client routing.
- UDP audio relay for low-latency audio listen and duplex voice chat.
- Admin console with online clients, search/filter, activity log, command menu, and rich result windows.
- Session actions: update, uninstall, kill client process, shutdown, reboot, and delete offline clients.
- Remote management tools: file manager, streaming terminal, process/window/startup/driver managers, registry snapshot, event log, active connections, and performance monitor.
- System tools: computer information, clipboard read/write, execute file, execute code, and reusable static commands.
- Live control: remote desktop, mouse/keyboard input, camera view, audio listen, and voice chat.
- User interaction: message boxes, system notifications, text chat, and opening text in the platform editor.
- Cross-platform GUI/terminal fallback builds for Windows, Linux, and macOS, with GitHub Actions release artifacts.

## Supported Platforms

| Binary | Windows | Linux | macOS | Notes |
| --- | --- | --- | --- | --- |
| `rdl-server` | ✅ | ✅ | ✅ | Terminal server. |
| `rdl-client` | ✅ | ✅ | ✅ | GUI client; terminal fallback with `RDL_FORCE_TERMINAL=1`. |
| `rdl-admin` | ✅ | ✅ | ✅ | GUI admin console; terminal mode for smoke tests. |

Platform-specific capability notes:

- Windows: desktop capture uses native GDI; camera uses Media Foundation through `nokhwa`; audio capture/playback uses `cpal`; input uses Windows APIs and PowerShell text input.
- Linux: desktop capture currently targets X11 through `maim` or ImageMagick `import`; audio capture/playback uses `cpal` with the system audio backend; mouse input uses `xdotool`; Wayland needs a portal/ydotool backend later.
- macOS: desktop capture uses `screencapture`; audio capture/playback uses `cpal` and may require Microphone permission; mouse input uses Core Graphics and requires Accessibility permission for the process that launches `rdl-client`; screen capture may require Screen Recording permission.
- macOS debug/release binaries can be ad-hoc signed. Production Developer ID signing and notarization are still future work.

## Requirements

- Rust stable toolchain, installed with `rustup`.
- Git.
- Windows, Linux, or macOS.

Linux remote desktop testing may also require desktop tools such as `maim`, ImageMagick `import`, `xdotool`, and X11 utilities. See [Ubuntu X11 remote desktop testing](docs/ubuntu-x11-remote-desktop-testing.md).

Install or update Rust:

```sh
rustup update stable
rustup default stable
```

Check the toolchain:

```sh
rustc --version
cargo --version
```

## Build

Download crate dependencies:

```sh
cargo fetch
```

Check the workspace:

```sh
cargo check --workspace
```

Build debug binaries:

```sh
cargo build --workspace
```

Build release binaries:

```sh
cargo build --workspace --release
```

Debug binaries are written to `target/debug`; release binaries are written to `target/release`. Windows builds use the `.exe` suffix.

## Version Info

All three binaries expose the build version:

```sh
rdl-server --version
rdl-client --version
rdl-admin --version
```

Tagged builds use the exact current git tag, for example `v0.1.0`. Untagged local builds fall back to the workspace package version from `Cargo.toml`. `RDL_BUILD_VERSION` can be set by CI to override the displayed version explicitly.

## Quick Start

Launch the local dev stack. This starts the server, client, and admin GUI for manual testing:

```sh
./scripts/start-dev.sh
```

On Windows:

```powershell
.\scripts\start-dev.bat
```

Run the server manually:

```sh
cargo run -p rust-desk-light-server -- --ip 0.0.0.0 --port 5169
```

The server uses the configured port for both TCP control/video/file traffic and UDP audio relay traffic. If you run across machines, allow both TCP and UDP on that port.

Run a client:

```sh
cargo run -p rust-desk-light-client -- --ip 127.0.0.1 --port 5169
```

Run the admin GUI:

```sh
cargo run -p rust-desk-light-admin -- --ip 127.0.0.1 --port 5169
```

For release-mode manual testing, put Cargo flags before the `--` separator and app flags after it:

```sh
cargo run --release -p rust-desk-light-admin -- --ip 127.0.0.1 --port 5169
```

Useful environment variables:

```sh
RDL_IP=127.0.0.1
RDL_PORT=5169
RDL_FORCE_TERMINAL=1
```

In the admin GUI, select an online client, right-click the client row, and choose a command from the menu.

## Smoke Test

Run the automated local smoke flow. It uses terminal mode so CI and local shells can drive the protocol without opening GUI windows:

```sh
./scripts/smoke-test.sh
```

On Windows PowerShell:

```powershell
.\scripts\smoke-test.bat
```

## Release Builds

Tagged releases are built by GitHub Actions from `.github/workflows/release.yml`.

Pushing a tag like `v0.1.0` creates platform artifacts for:

- Linux x64
- macOS x64
- macOS ARM64
- Windows x64

Each release package contains `rdl-server`, `rdl-client`, `rdl-admin`, and `README.md`. Rust release builds are native binaries, so there is no separate runtime/no-runtime split.

On macOS, if a downloaded release binary is blocked by quarantine metadata, clear it after extracting the archive:

```sh
xattr -cr ./rdl-client
xattr -cr ./rdl-admin
xattr -cr ./rdl-server
```

## Design Notes

The transport is a custom versioned binary protocol over TCP. Frames use `RDL1` magic bytes, protocol version, length, role, message kind, session token, and typed payloads. Client and admin peers register first, then the server issues a session token required by follow-up messages.

Live desktop and camera frames use binary `VideoFrame` messages over TCP. This keeps large JPEG frames reliable; the current UDP relay is intentionally not used for remote desktop or camera because high-quality desktop frames can split into hundreds of UDP packets, where one lost packet drops the whole frame without a proper video codec, retransmission, FEC, or QUIC/WebRTC-style transport.

Audio listen and voice chat use a separate lightweight UDP packet format with `RDU1` magic bytes. The receiver registers a stream id with the server's UDP relay, and the sender emits small PCM `pcm_s16le` packets with sequence numbers, capture timestamps, sample rate, and channel count. Audio listen uses one client-to-admin stream. Voice chat uses two streams, one admin-to-client and one client-to-admin. The audio path is UDP-only by design so it does not build up seconds of TCP head-of-line delay under interactive use.

Command result compatibility paths remain text-based where appropriate.

## Roadmap

See [ROADMAP.md](ROADMAP.md) for current milestones and planned work.

## License

This project is licensed under the Apache License 2.0.
