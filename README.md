# rust-desk-light

A lightweight Rust remote assistance workspace inspired by the split between RustDesk clients and RustDesk Server.

The repository is intentionally small at the start:

- `rdl-server`: terminal relay and presence server.
- `rdl-client`: assisted endpoint GUI, with GUI environment detection and terminal fallback.
- `rdl-admin`: operator GUI with online client table and full right-click command menu.
- `rdl_protocol`: shared protocol primitives.

This project is still early. It has a stable binary transport, identity/session handshake, admin/client GUIs, command routing, and several low-risk client capability commands. It does not yet implement real remote desktop streaming, file transfer, shell execution, camera, microphone, or privileged system operations.

## Requirements

- Rust stable toolchain, installed with `rustup`.
- Git.
- Windows, Linux, or macOS.

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

## Dependencies And Build

Download Rust crate dependencies without building:

```sh
cargo fetch
```

Compile every workspace crate:

```sh
cargo build --workspace
```

Fast compile/type check:

```sh
cargo check --workspace
```

Build release binaries:

```sh
cargo build --workspace --release
```

The debug binaries are written to:

```text
target/debug/rdl-server
target/debug/rdl-client
target/debug/rdl-admin
```

On Windows they have `.exe` suffixes.

Release binaries are written to `target/release`.

## Quick Start

Launch the dev stack for manual GUI testing. This opens `server` in a terminal, then starts `client` and `admin` as GUI windows:

```sh
./scripts/start-dev.sh
```

On Windows:

```powershell
.\scripts\start-dev.bat
```

Optional environment variables:

```sh
RDL_IP=127.0.0.1 RDL_PORT=21116 ./scripts/start-dev.sh
```

Run an automated local smoke test. This intentionally forces terminal mode so CI can drive the protocol without opening GUI windows:

```sh
./scripts/smoke-test.sh
```

On Windows PowerShell:

```powershell
.\scripts\smoke-test.bat
```

Run the server:

```sh
cargo run -p rust-desk-light-server -- --ip 0.0.0.0 --port 21115
```

Run a client:

```sh
cargo run -p rust-desk-light-client -- --ip 127.0.0.1 --port 21115
```

Run the admin GUI:

```sh
cargo run -p rust-desk-light-admin -- --ip 127.0.0.1 --port 21115
```

Run client/admin in terminal mode:

```sh
RDL_FORCE_TERMINAL=1 cargo run -p rust-desk-light-client -- --ip 127.0.0.1 --port 21115
RDL_FORCE_TERMINAL=1 cargo run -p rust-desk-light-admin -- --ip 127.0.0.1 --port 21115
```

On Windows PowerShell:

```powershell
$env:RDL_FORCE_TERMINAL = "1"
cargo run -p rust-desk-light-client -- --ip 127.0.0.1 --port 21115
```

In the admin GUI:

```text
select an online client
right-click the client row to open the command menu
click a command
admin opens a result window for the command output
```

## Design Notes

The current transport is a custom versioned binary protocol over TCP. Frames use `RDL1` magic bytes, protocol version, length, role, message kind, session token, and typed UTF-8 payloads. Client/admin peers register first, then the server issues a session token required by follow-up messages.

`client` starts as a GUI when the current system has GUI support. On headless Linux, or when `RDL_FORCE_TERMINAL=1` is set, it falls back to terminal mode. `admin` starts as a GUI by default; `RDL_FORCE_TERMINAL=1` is kept only for automated protocol smoke tests.

## Useful Commands

Format code:

```sh
cargo fmt
```

Run the local smoke flow:

```sh
scripts/smoke-test.sh
```

On Windows:

```powershell
.\scripts\smoke-test.bat
```

Clean build artifacts:

```sh
cargo clean
```

## Docs

- [Ubuntu X11 remote desktop testing](docs/ubuntu-x11-remote-desktop-testing.md)
