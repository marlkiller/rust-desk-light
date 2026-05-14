# ROADMAP

This project is a lightweight Rust remote assistance tool with three small parts:

- `rdl-server`: presence, routing, and relay.
- `rdl-client`: endpoint agent with GUI status and terminal fallback.
- `rdl-admin`: operator GUI for listing clients and sending commands.
- `rdl_protocol`: shared wire protocol and command model.

The project can reference `D:\workspace\code\rust\rustdesk` and
`D:\workspace\code\rust\rustdesk-server` for architecture, naming, transport
ideas, and platform backends, but it should not copy their full production
complexity. The goal is a practical small tool first.

## Principles

- Keep the protocol consistent before adding features.
- Keep the server simple: register peers, track presence, route messages, and log events.
- Keep GUI lightweight with `egui/eframe`.
- Keep headless/terminal mode working for testing and recovery.
- Add risky capabilities gradually, with explicit user-visible behavior.
- Prefer small, testable milestones over a large professional remote-desktop stack.

## Milestone 0: Workspace Foundation

- [x] Create Rust workspace.
- [x] Add `server`, `client`, `admin`, and shared `protocol` crates.
- [x] Add `--ip` and `--port` startup arguments.
- [x] Add terminal server.
- [x] Add GUI startup with terminal fallback.
- [x] Add online client registration.
- [x] Add admin-visible client list.
- [x] Add admin command/menu vocabulary.
- [x] Add command forwarding stubs.
- [x] Improve admin and client UI shells.

## Milestone 1: Stable Binary Transport Protocol

This is the next important milestone. All later features depend on this being stable.

- [x] Replace the current ad-hoc pipe-separated text frames with one shared versioned binary protocol.
- [x] Use one format everywhere: client, admin, and server must encode/decode the same message model.
- [x] Use RustDesk as the architectural reference: framed transport plus typed messages, but keep this project much smaller than RustDesk's full protobuf/rendezvous stack.
- [x] Start with a simple custom binary frame instead of JSON:
  - magic bytes: `RDL1`
  - protocol version
  - frame length
  - message id
  - optional correlation id
  - role: `client`, `admin`, or `server`
  - message kind
  - payload length
  - payload bytes
- [x] Keep payloads typed in `rdl_protocol`; do not let server/client/admin invent separate encodings.
- [x] Keep strings as length-prefixed UTF-8 inside the binary payload.
- [x] Keep command enum values stable and documented.
- [x] Add encode/decode tests for core message types.
- [x] Add protocol errors as first-class messages.
- [ ] Add heartbeat messages: `ping`, `pong`, and last-seen timestamps.
- [ ] Add basic reconnect with backoff for client and admin.
- [ ] Add stale client cleanup on the server.
- [ ] Add integration tests for:
  - client registration
  - admin registration
  - client list
  - command forwarding
  - command ack forwarding
  - reconnect
  - offline client command failure

Notes:

- Do not use JSON for the transport protocol.
- Do not pull in RustDesk's full protocol directly; implement a small compatible design style in `rdl_protocol`.
- Protobuf can be considered later if the custom binary protocol grows too much, but Milestone 1 should be readable and maintainable in this repo.
- Add a small protocol dump/debug tool or test helper so binary frames remain inspectable during development.
- TLS/Noise encryption is intentionally not required in this milestone; first make the protocol consistent and tested.

## Milestone 2: Identity And Local Trust

Keep identity useful but lightweight.

- [ ] Add stable client fingerprint based on host/user/os plus generated local id.
- [ ] Persist client id/fingerprint in a local config file.
- [ ] Add admin identity string and display it in server logs.
- [ ] Add server-issued session token after successful registration.
- [ ] Require token on follow-up messages after registration.
- [ ] Add clear server audit logs for connect, register, disconnect, list, command, and ack.
- [ ] Add a simple enrollment key option later if unattended access is needed.

Reference direction:

- RustDesk has richer identity, rendezvous, and relay behavior. This project only needs the small subset required to keep peers recognizable and messages attributable.

## Milestone 3: UI And Operator Workflow

- [x] Admin GUI with command menu, overview, client list, and activity log.
- [x] Client GUI with status and activity log.
- [ ] Add admin search/filter for clients.
- [ ] Add clearer status badges: online, offline, reconnecting, stale.
- [ ] Show client fingerprint, hostname, user, OS, and last seen.
- [ ] Add a simple command result panel.
- [ ] Preserve terminal mode for smoke tests.

## Milestone 4: Basic Client Capabilities

Implement read-only and low-risk commands first.

- [ ] Computer info.
- [ ] Clipboard read/write as explicit command stubs, then real implementation.
- [ ] Active connections.
- [ ] Process list.
- [ ] Performance snapshot.
- [ ] Event log summary where available.

Reference direction:

- Look at RustDesk platform modules for how it separates OS-specific behavior, but keep this repo's API much smaller.

## Milestone 5: User Interaction

- [ ] Message box.
- [ ] System notification / balloon tip.
- [ ] Text chat.
- [ ] Open text in notepad or platform equivalent.

These are useful before full remote desktop because they validate bidirectional command/result flow.

## Milestone 6: File And Terminal Basics

- [ ] Remote terminal command execution with timeout.
- [ ] Command output streaming.
- [ ] File list for a selected directory.
- [ ] File download.
- [ ] File upload.

Keep this simple first. Resume, hashing, cancellation, PTY, and permissions can come later.

## Milestone 7: Screen View First

Build view-only remote desktop before input control.

- [ ] Capture screen frame on client.
- [ ] Send compressed image frames to admin.
- [ ] Display remote screen in admin.
- [ ] Add frame rate limit.
- [ ] Add single-monitor first.

Reference direction:

- Use RustDesk as a reference for screen capture and encoding choices, but start with the smallest working frame transport.

## Milestone 8: Remote Control

- [ ] Mouse movement.
- [ ] Mouse click.
- [ ] Keyboard input.
- [ ] Clipboard sync during remote session.
- [ ] Local visible indicator while remote control is active.

This should only be implemented after screen view and protocol reliability are good enough.

## Milestone 9: Packaging And Runtime

- [ ] Persistent config files.
- [ ] Server config file.
- [ ] Windows build artifact.
- [ ] Linux build artifact.
- [ ] Optional service/daemon mode.
- [ ] Basic release script.

## Not Planned For Now

These are useful in a professional product but too heavy for the current lightweight goal:

- Plugin sandbox.
- Full role-based access control.
- Enterprise audit database.
- Multi-tenant server.
- NAT traversal optimization.
- Camera and microphone streaming.
- Auto-update system.
- Signed plugin loading.
- Complex task scheduler.

They can be reconsidered only after the basic remote assistance flow works reliably.

## Command Menu Map

```text
Session
  Client
    Update Client: update_client
    Uninstall Client: uninstall_client
    Kill Client Process: kill_client_process
  Power
    Shutdown: shutdown
    Reboot: reboot
  Session Management
    Move To Group: move_to_group
    Clone Client Settings: clone_client_settings
    Delete Client: delete_client

Remote Management
  Files And Terminal
    File Manager: file_manager
    Remote Terminal: remote_terminal
  System Tools
    Process Manager: process_manager
    Window Manager: window_manager
    Startup Manager: startup_manager
    Registry Manager: registry_manager
    Driver Manager: driver_manager
    Event Log: event_log
  Monitoring
    Active Connections: active_connections
    Performance Monitor: performance_monitor

Live Control
  Desktop
    Remote Desktop: remote_desktop
  Media Devices
    Camera: camera
    Audio Listen: audio_listen

User Interaction
  Prompts
    Message Box: message_box
    Balloon Tip: balloon_tip
  Communication
    Text Chat: text_chat
    Voice Chat: voice_chat
  Text Actions
    Open Text In Notepad: open_text_in_notepad

System Info
  Basics
    Computer Info: computer_info
    Clipboard: clipboard
  Network
    Proxy: proxy

Execute
  Code And Files
    Execute File: execute_file
    Execute Code: execute_code
  Tasks
    Execute Static Command: execute_static_command
    Create Task: create_task
  Automation
    Command Preset: command_preset

Plugins
  Extensions
    Plugin Manager: plugin_manager
```
