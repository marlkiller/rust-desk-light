---
name: rust-desk-light-feature
description: Implement or modify rust-desk-light features in the repository's established style. Use when work touches the Admin GUI, Server routing, Client behavior, rdl_protocol messages or CommandKind values, remote-management windows, egui UI layout/theme, i18n strings, or end-to-end admin/server/client interactions.
---

# Rust Desk Light Feature

## Start Here

Work from the existing architecture instead of inventing a parallel path.

- Read the nearby implementation before editing: protocol definitions, Admin event handling, Server routing, Client handling, and the closest UI window.
- Keep changes end-to-end. If a feature crosses Admin, Server, Client, and protocol, update all four in the same pass.
- Preserve user work in the dirty tree. Do not revert unrelated files.
- Prefer small, focused helpers and package-local tests over broad refactors.
- Keep product/docs language neutral. Do not mention external reference projects in user-facing docs or roadmap entries.

## Repository Map

Use these files as anchors:

- `crates/protocol/src/lib.rs`: wire protocol, `PROTOCOL_VERSION`, `Role`, `CommandKind`, `Message`, binary encode/decode tests.
- `crates/server/src/main.rs`: peer registry, role checks, audit logs, routing between Admin and Client.
- `crates/client/src/app.rs`: client message loop and long-running stream workers.
- `crates/client/src/remote_management/`: client-side command implementations.
- `crates/admin/src/app.rs`: Admin state, event dispatch, window storage, command/event handling.
- `crates/admin/src/command_menu.rs`: client context menu commands.
- `crates/admin/src/remote_management/`: dedicated Admin tool windows.
- `crates/admin/src/app/command_result.rs`: shared result-table windows and row actions.
- `crates/admin/src/theme.rs`: colors, sizes, frames, tables, status bars.
- `crates/admin/src/i18n.rs`: `t`, `tf`, command titles, Chinese translations, i18n tests.

## Protocol And Routing

For ordinary request/response commands, follow the `Message::Command` pattern:

- Admin sends `Message::Command { target_id, command, payload }`.
- Server validates the sender role and routes by `target_id`.
- Client executes and replies with `CommandAck` and/or `CommandOutput`.
- Admin maps the returned `client_id`, `command`, and payload into the relevant window.

For interactive streams or bidirectional data, add explicit `Message` variants instead of overloading command output:

- Use `target_id` for Admin-to-Client open/control messages.
- Use `client_id` for Client-to-Admin result/data/close messages.
- Add a stable stream/session id when multiple streams can coexist.
- Server must role-check every direction, keep any required route map, remove routes on close/failure, and send a clear error/result back to Admin when routing fails.
- Client must clean up stream state on close, failed writer, failed reader, or disconnect.

When changing the wire protocol:

- Update `PROTOCOL_VERSION` if old binaries cannot safely understand the new messages.
- Add or update `Message::kind_code`, encoder, decoder, and roundtrip tests.
- If adding `CommandKind`, update `as_str`, `parse`, `to_code`, `from_code`, `requires_client_gui` if needed, Admin command titles, command menu, client execution, and tests.

## Admin Windows

Prefer the existing window shape:

- Store per-tool windows in `AdminApp`, render them from `window_dispatch`, and expose `open_window`, `render_windows`, `handle_*`, and `stop_all` style functions for standalone remote-management tools.
- Use `windowing::child_viewport_builder` and `ctx.show_viewport_immediate` for child tool windows.
- Use `Arc<Mutex<_>>` for shared mutable window state and `AtomicBool` request flags for buttons. Process the request flags after the UI closure to avoid egui borrow tangles.
- Do blocking socket, file, or process work on worker threads. Do not block egui rendering.
- Reserve space for status bars before allocating tables or scroll areas.
- For connection/session histories, cap closed historical rows and keep active rows visible.

## UI And Theme

Follow `crates/admin/src/theme.rs`; do not invent one-off dimensions or colors.

- Use `CONTROL_HEIGHT` (`28.0`) for normal controls.
- Use `COMPACT_CONTROL_HEIGHT` (`24.0`) for compact toolbars and table controls.
- Use `PANEL_MARGIN` (`8.0`) and `SECTION_GAP` (`6.0`) for panel spacing.
- Use `TABLE_HEADER_HEIGHT` and `TABLE_ROW_HEIGHT` for tables.
- Use `panel_frame_with_margin`, `page_frame`, `status_frame`, `footer_frame`, and `clickable_table`.
- Use `palette().text`, `palette().muted`, `COLOR_GOOD`, `COLOR_BAD`, and `COLOR_WARN`; avoid hardcoded ad hoc colors.
- Size action areas from text (`action_area_width`-style helpers) when labels can be translated.
- Reuse existing buttons, table styles, context-menu style, and status-line components.
- Keep operational UIs dense and utilitarian. Avoid landing-page/marketing patterns, decorative cards, nested cards, and oversized headings inside tool windows.
- Use icon/button text only when it matches local conventions. Do not add new visual language to one window.

For tables:

- Use `egui_extras::TableBuilder` through `crate::theme::clickable_table` unless a nearby window uses a different established table helper.
- Keep row/header height stable.
- Use sortable/filterable/cache behavior only when the existing window pattern supports it.
- Do not make rows clickable unless the click performs a visible action; avoid stale selection state.

## I18n

All user-facing text must go through `t()` or `tf()`.

- Use `t("Static Label")` for static labels.
- Use `tf("Message with {name}", &[("name", value)])` for dynamic text.
- Add every new key to `zh()` in `crates/admin/src/i18n.rs`.
- If a feature has an i18n key test list, add keys there too.
- Prefer generic future-proof strings: `Settings saved.` instead of `Theme and language saved.` when the setting group may grow.
- Remove stale i18n keys when UI text is removed and no other code uses them.
- Keep command titles in `command_key`.

## OS-Specific Behavior

Use local OS conventions and test helpers:

- Prefer `cfg!(target_os = "windows")`, `cfg!(target_os = "macos")`, or `std::env::consts::OS` for Admin-side OS-specific strings.
- For Client commands, mirror existing Windows/macOS/Linux branches in `crates/client/src/remote_management`.
- Keep Windows PowerShell/cmd syntax, macOS shell syntax, and Linux shell/systemd behavior distinct.
- When generating copyable terminal commands, produce a command that fits the Admin machine's OS unless the user asks for a remote/client OS command.

## Performance

Avoid unbounded UI and memory growth.

- Cap logs and closed/failed history rows; active sessions should remain visible.
- Separate active state maps from history queues when a feature can run for a long time.
- Avoid sorting/formatting large histories every frame unless the data changed.
- Throttle UI refresh for high-frequency streams.
- Keep binary data off text logs; log summaries or counters instead.

## Validation

Run the smallest relevant checks, then broaden when protocol or shared behavior changes.

- Always run `cargo fmt --all -- --check`.
- For Admin UI changes, run `cargo test -p rust-desk-light-admin`.
- For protocol changes, run protocol tests and check all affected packages.
- For Admin/Server/Client end-to-end changes, run:
  - `cargo check -p rust-desk-light-admin -p rust-desk-light-server -p rust-desk-light-client`
  - package tests touched by the change
- Add focused tests for protocol roundtrips, parser/formatter helpers, i18n key coverage, routing error mapping, and history caps.

## Final Checklist

Before finishing:

- Confirm no unused UI text, dead enum variants, stale tests, or stale roadmap wording remain.
- Confirm user-visible docs do not mention private/reference implementation names.
- Confirm the final answer says what changed and what validation passed.
