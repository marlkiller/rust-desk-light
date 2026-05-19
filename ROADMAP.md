# ROADMAP

`rust-desk-light` is a lightweight remote assistance tool. The goal is to keep it small, usable, and easy to build across Windows, Linux, and macOS.

## Keep In Mind

- Keep the server simple: presence, routing, and relay.
- Keep the protocol shared and typed.
- Keep the GUI lightweight.
- Keep terminal/headless mode usable for recovery.
- Avoid turning this into a large enterprise remote-desktop platform.

## Current State

- Admin, server, client, shared protocol, shared config, and release build wiring are in place.
- Admin can list clients, run commands, view results, and open live/file/terminal tools.
- Client supports system info, clipboard, process/window/startup/driver/registry views, event log, and performance snapshot.
- File manager supports local/remote navigation, upload/download, directory transfer, native file pickers, transfer status, cancel, delete, rename, and new folder.
- Remote terminal supports cwd, history, streaming output, cancellation, copy, and safe close.
- Remote desktop supports screen selection, TCP video frames, mouse move/click, and text input.
- Camera, audio listen, and voice chat are working.
- Admin supports saved theme and language preferences, with English and Chinese UI resources.
- Runtime config files are initialized automatically and can be overridden by startup args.
- Admin can update client server config remotely.
- Client has a single-instance process lock.

## Next

### Security

- [ ] Add SSL/TLS for admin/server/client TCP connections.

### Deployment

- [x] Add an admin-side client builder for embedding server IP/port into a generated client package.
- [x] Add admin-controlled client login autostart.
- [ ] Add optional service/daemon install mode.
- [ ] Add client config for automatic wake and sleep prevention while needed.

### Remote Control

- [ ] Sync clipboard inside remote-control sessions.
- [ ] Show a local visible "being remotely controlled" indicator on the client.
- [ ] Support raw keyboard events for shortcuts, arrows, function keys, and modifiers.

### Admin UI

- [x] Wire saved admin theme config into runtime UI styling.
- [x] Add admin i18n resources and apply the saved language config.
- [ ] Continue tightening translation coverage and add more languages when needed.

### File Transfer

- [ ] Improve conflict handling: overwrite, skip, rename.

### Reverse Proxy

- [ ] Add a Quasar-style reverse proxy for the three-part architecture: Admin opens a local SOCKS5 listener, Server only routes framed proxy messages, and the selected Client opens outbound TCP connections to target hosts.
- [ ] Support per-connection proxy streams with open, data, and close messages so browser/tool traffic can flow through a chosen remote Client.
- [ ] Keep this separate from client/server outbound network proxy settings; this feature is a remote network egress tunnel, not an HTTP/SOCKS proxy used to reach the RDL server.

### Menu TODOs

- Move To Group.
- Clone Client Settings.
- Create Task.
- Command Preset.
- Plugin Manager.

## Later

- Wayland capture/input support.
- Transfer resume and hash verification.
- PTY-backed remote terminal.
- Production macOS signing and notarization.
- Better video transport only if TCP video becomes a real blocker.

## Not Planned For Now

- Enterprise RBAC.
- Multi-tenant server.
- NAT traversal stack.
- Plugin sandbox.
- Full conferencing stack.
- Complex task scheduler.
- Auto-update system.
