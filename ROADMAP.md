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
- Keep headless/terminal mode working for smoke checks and recovery.
- Add risky capabilities gradually, with explicit user-visible behavior.
- Prefer small, verifiable milestones over a large professional remote-desktop stack.

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
  - session token length
  - payload length
  - session token bytes
  - payload bytes
- [x] Keep payloads typed in `rdl_protocol`; do not let server/client/admin invent separate encodings.
- [x] Keep strings as length-prefixed UTF-8 inside the binary payload.
- [x] Keep command enum values stable and documented.
- [x] Manually verify binary encode/decode with the smoke flow.
- [x] Add protocol errors as first-class messages.
- [x] Add heartbeat messages: `ping`, `pong`, and last-seen timestamps.
- [x] Add basic reconnect with backoff for client and admin.
- [x] Add stale client cleanup on the server.
- [x] Manually verify client registration.
- [x] Manually verify admin registration.
- [x] Manually verify client list.
- [x] Manually verify command forwarding.
- [x] Manually verify command ack forwarding.
- [x] Manually verify reconnect.
- [x] Manually verify offline client command failure.

Notes:

- Do not use JSON for the transport protocol.
- Do not pull in RustDesk's full protocol directly; implement a small compatible design style in `rdl_protocol`.
- Protobuf can be considered later if the custom binary protocol grows too much, but Milestone 1 should be readable and maintainable in this repo.
- Add a small protocol dump/debug tool so binary frames remain inspectable during development.
- TLS/Noise encryption is intentionally not required in this milestone; first make the protocol consistent and verified.

## Milestone 2: Identity And Local Trust

Keep identity useful but lightweight.

- [x] Add stable client fingerprint based on host/user/os plus generated local id.
- [x] Persist client id/fingerprint in a local config file.
- [x] Add admin identity string and display it in server logs.
- [x] Add server-issued session token after successful registration.
- [x] Require token on follow-up messages after registration.
- [x] Add clear server audit logs for connect, register, disconnect, list, command, and ack.
- [ ] Add a simple enrollment key option later if unattended access is needed.

Reference direction:

- RustDesk has richer identity, rendezvous, and relay behavior. This project only needs the small subset required to keep peers recognizable and messages attributable.

## Milestone 3: UI And Operator Workflow

- [x] Admin GUI with command menu, overview, client list, and activity log.
- [x] Client GUI with status and activity log.
- [x] Add admin search/filter for clients.
- [x] Improve command result tables with compact adaptive columns, sorting, filtering, copy actions, and process kill actions where supported.
- [x] Add clearer status badges: online, offline, reconnecting, stale.
- [x] Show client fingerprint, hostname, user, OS, and last heartbeat.
- [x] Show client IP address, OS version, GUI availability, and full-row selection in the admin client list.
- [x] Add a simple command result panel.
- [x] Allow command result text selection/copy for plain text outputs.
- [x] Disable problematic macOS child-window maximize controls and keep child windows out of automatic tabbing.
- [x] Preserve terminal mode for smoke checks.

## Milestone 4: Basic Client Capabilities

Implement read-only and low-risk commands first.

- [x] Computer info.
- [x] Expanded computer info with OS version, kernel/build, CPU, memory, session, and IP details where available.
- [x] Clipboard read/write as explicit command stubs, then real implementation.
- [x] Active connections.
- [x] Fix macOS active connections through `lsof -nP -iTCP -iUDP` instead of Linux-only `netstat -tunap`.
- [x] Process list.
- [x] Performance snapshot.
- [x] Event log summary where available.
- [x] Parse macOS compact `log show` error/fault rows robustly.

Reference direction:

- Look at RustDesk platform modules for how it separates OS-specific behavior, but keep this repo's API much smaller.

## Milestone 5: User Interaction

- [x] Message box.
- [x] System notification / balloon tip.
- [x] Text chat.
- [x] Open text in notepad or platform equivalent.

These are useful before full remote desktop because they validate bidirectional command/result flow.

## Milestone 6: File And Terminal Basics

- [x] Remote terminal command execution with timeout.
- [x] Improve remote terminal window UX with cwd prompt, command history, copy/clear actions, and safe close handling.
- [x] Command output streaming with remote terminal cancellation.
- [x] File list for a selected directory.
- [x] File download.
- [x] File upload.
- [x] Large file and directory upload/download through chunked binary file transfer messages.
- [x] File transfer progress table with stop and delete row actions.
- [x] File delete, rename, new folder, path jump, and parent directory navigation.

Keep this simple first. Resume, hashing, PTY, and permissions can come later.

## Milestone 7: Screen View First

Build view-only remote desktop before input control.

- [x] Capture screen frame on client. Windows lightweight MVP via native GDI capture.
- [x] Send compressed single-shot image frames to admin. Command result compatibility path still carries base64.
- [x] Display remote screen in admin.
- [x] Add frame rate limit. Polling MVP with a conservative refresh interval.
- [x] Add screen selection before starting the remote desktop session.
- [x] Add Ubuntu X11 testing documentation for Linux remote desktop.
- [x] Improve admin remote desktop frame handling by coalescing frames and decoding off the UI thread.
- [x] Move live video frames to binary `VideoFrame` transport instead of command/ack base64 payloads.
- [x] Keep live remote desktop capture on direct binary bytes across Windows/Linux/macOS.

Reference direction:

- Use RustDesk as a reference for screen capture and encoding choices, but start with the smallest working frame transport.

## Milestone 8: Remote Control

- [x] Mouse movement. Windows, X11, and macOS lightweight MVP.
- [x] Mouse click. Windows, X11, and macOS lightweight MVP.
- [x] Keyboard input. Text send MVP; raw key events still pending.
- [x] Separate desktop capture, mouse movement, and mouse click controls in the admin remote desktop window.
- [x] Keep remote desktop capture running when an input action fails.
- [x] Surface macOS Accessibility/TCC permission failures as input status instead of stopping capture.
- [ ] Clipboard sync during remote session.
- [ ] Local visible indicator while remote control is active.
- [ ] Raw keyboard events for desktop control.

This should only be implemented after screen view and protocol reliability are good enough.

macOS note:

- Remote mouse input requires Accessibility permission for the app that launches `rdl-client` (for example Terminal, iTerm, Warp, or the Codex host), not only the bare `rdl-client` file. Screen capture still requires Screen Recording permission for the running client process.

## Milestone 8.5: Camera Capture

- [x] Camera command routing through live control.
- [x] Admin camera control window with device selection, quality selection, start/stop capture, status bar, and save-current-frame action.
- [x] Windows camera backend via native Media Foundation through nokhwa.
- [x] Reuse the Windows camera stream during capture instead of reopening the device per frame.
- [x] Decode camera frames off the admin UI thread and coalesce latest frames.
- [x] Linux/macOS lightweight snapshot fallback through local camera tools.
- [x] Use shared binary `VideoFrame` transport for camera capture frames.
- [x] Keep live camera capture on direct binary bytes instead of local base64 encode/decode.

## Milestone 9: Packaging And Runtime

- [x] Persistent client/admin identity config files.
- [x] Avoid repeated macOS hostname `scutil` calls by caching hostname and using `gethostname` first.
- [x] Automatically ad-hoc sign macOS debug builds for `rdl-client`, `rdl-admin`, and `rdl-server`.
- [ ] General persistent config files beyond identity.
- [ ] Server config file.
- [x] Windows build artifact.
- [x] Linux build artifact.
- [x] macOS release artifact.
- [ ] Optional service/daemon mode.
- [x] Basic GitHub Actions release workflow.

## Not Planned For Now

These are useful in a professional product but too heavy for the current lightweight goal:

- Plugin sandbox.
- Full role-based access control.
- Enterprise audit database.
- Multi-tenant server.
- NAT traversal optimization.
- Microphone streaming and full A/V session streaming.
- Auto-update system.
- Signed plugin loading.
- Complex task scheduler.

They can be reconsidered only after the basic remote assistance flow works reliably.

## Command Menu Map

```text
Session
  [x] Update Client: update_client
  [x] Uninstall Client: uninstall_client
  [x] Kill Client Process: kill_client_process
  [x] Shutdown: shutdown
  [x] Reboot: reboot
  [ ] Move To Group: move_to_group
  [ ] Clone Client Settings: clone_client_settings
  [x] Delete Client: delete_client

Remote Management
  [x] File Manager: file_manager
  [x] Remote Terminal: remote_terminal
  [x] Process Manager: process_manager
  [x] Window Manager: window_manager
  [x] Startup Manager: startup_manager
  [x] Registry Manager: registry_manager
  [x] Driver Manager: driver_manager
  [x] Event Log: event_log
  [x] Active Connections: active_connections
  [x] Performance Monitor: performance_monitor

Live Control
  [x] Remote Desktop: remote_desktop
  [x] Camera: camera
  [ ] Audio Listen: audio_listen

User Interaction
  [x] Message Box: message_box
  [x] Balloon Tip: balloon_tip
  [x] Text Chat: text_chat
  [ ] Voice Chat: voice_chat
  [x] Open Text In Notepad: open_text_in_notepad

System Info
  [x] Computer Info: computer_info
  [x] Clipboard: clipboard
  [ ] Proxy: proxy

Execute
  [x] Execute File: execute_file
  [x] Execute Code: execute_code
  [x] Execute Static Command: execute_static_command
  [ ] Create Task: create_task
  [ ] Command Preset: command_preset

Plugins
  [ ] Plugin Manager: plugin_manager
```
