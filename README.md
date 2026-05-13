# rust-desk-light

A lightweight Rust remote assistance workspace inspired by the split between RustDesk clients and RustDesk Server.

The repository is intentionally small at the start:

- `rdl-server`: terminal relay and presence server.
- `rdl-client`: assisted endpoint GUI, with GUI environment detection and terminal fallback.
- `rdl-admin`: operator GUI with online client table and full right-click command menu.
- `rdl_protocol`: shared protocol primitives.

This first milestone is a runnable foundation. It does not yet implement real remote desktop streaming, file transfer, shell execution, camera, microphone, persistence, or privileged system operations.

## Quick Start

Launch the dev stack for manual GUI testing. This opens `server` in a terminal, then starts `client` and `admin` as GUI windows:

```sh
./scripts/start-dev.sh
```

On Windows PowerShell:

```powershell
.\scripts\start-dev.ps1
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
.\scripts\smoke-test.ps1
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

In the admin GUI:

```text
select an online client
right-click the client row to open the command menu
or use the Action panel buttons for quick commands
```

## Design Notes

The current transport is a newline-delimited text protocol over TCP so every frame can be inspected while building the product. Later milestones will replace this with authenticated, encrypted, multiplexed channels.

`client` starts as a GUI when the current system has GUI support. On headless Linux, or when `RDL_FORCE_TERMINAL=1` is set, it falls back to terminal mode. `admin` starts as a GUI by default; `RDL_FORCE_TERMINAL=1` is kept only for automated protocol smoke tests.
