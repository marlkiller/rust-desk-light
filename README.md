# rust-desk-light

Lightweight Rust remote administration toolkit with GUI-based device management, remote desktop, file transfer, terminal access, camera viewing, and local-first control features.

`rust-desk-light` is organized as three small binaries plus a shared protocol crate:

- `rdl-server`: presence, session registration, routing, and relay.
- `rdl-client`: endpoint agent with GUI status, terminal fallback, and local capability handlers.
- `rdl-admin`: operator GUI for client discovery, commands, live control, and result viewing.
- `rdl_protocol`: shared binary transport and command model.

The project is intentionally compact. It focuses on practical remote assistance workflows, simple deployment, and a readable Rust codebase rather than a large enterprise remote management stack.

## Screenshots

TODO: Screenshots will be added later.

## Features

- Native Rust workspace with `server`, `client`, `admin`, and shared protocol crates.
- Versioned binary TCP protocol with session tokens and typed payloads.
- Admin GUI with online client table, search/filter, activity log, and right-click command menu.
- Client GUI with connection status, identity details, and timestamped activity log.
- Persistent client/admin identity files.
- Confirmed session lifecycle commands for client restart/update, uninstall, process stop, shutdown, reboot, and client deletion.
- Command result windows with compact tables, filtering, sorting, copy actions, and row-wide context menus.
- System information, process/window/startup/driver lists, Windows registry snapshot, process kill, performance snapshot, active connections, and event log summary.
- Clipboard read/write commands.
- File manager with directory listing, chunked large file and directory upload/download, progress table, stop/delete transfer actions, delete, rename, new folder, path jump, and parent navigation.
- Remote terminal with streaming output, cancellation, command history, cwd prompt, copy, and clear actions.
- Remote desktop viewing with screen selection, quality options, frame coalescing, and binary video frame transport.
- Remote mouse movement, mouse click, and text input where supported.
- Camera live view with device selection, quality selection, save-current-frame, and binary video frame transport.
- Text chat between admin and client.
- User interaction commands for message boxes, system notifications / balloon tips, and opening text in Notepad or the platform editor.
- Terminal fallback mode for smoke tests, headless recovery, and protocol checks.
- GitHub Actions release workflow for Linux, macOS, and Windows artifacts.

## Supported Platforms

| Binary | Windows | Linux | macOS | Notes |
| --- | --- | --- | --- | --- |
| `rdl-server` | Supported | Supported | Supported | TCP relay and presence server. |
| `rdl-client` | Supported | Supported | Supported | GUI when available; terminal fallback with `RDL_FORCE_TERMINAL=1`. |
| `rdl-admin` | Supported | Supported | Supported | GUI operator console; terminal mode is mainly for smoke tests. |

Platform-specific capability notes:

- Windows: desktop capture uses native GDI; camera uses Media Foundation through `nokhwa`; input uses Windows APIs and PowerShell text input.
- Linux: desktop capture currently targets X11 through `maim` or ImageMagick `import`; mouse input uses `xdotool`; Wayland needs a portal/ydotool backend later.
- macOS: desktop capture uses `screencapture`; mouse input uses Core Graphics and requires Accessibility permission for the process that launches `rdl-client`; screen capture may require Screen Recording permission.
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

Run a client:

```sh
cargo run -p rust-desk-light-client -- --ip 127.0.0.1 --port 5169
```

Run the admin GUI:

```sh
cargo run -p rust-desk-light-admin -- --ip 127.0.0.1 --port 5169
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

Live desktop and camera frames use binary `VideoFrame` messages rather than local base64 encode/decode paths. Command result compatibility paths remain text-based where appropriate.

## Roadmap

See [ROADMAP.md](ROADMAP.md) for current milestones and planned work.

## License

This project is licensed under the Apache License 2.0.
