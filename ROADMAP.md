# ROADMAP

This roadmap turns the project from an empty workspace into a practical lightweight cross-platform remote assistance program with `server`, `client`, and `admin` modules.

## Guiding Principles

- Build the safe remote-assistance path first: identity, consent, authentication, audit logs, and revocation before powerful operations.
- Keep `server` dumb and reliable: presence, routing, relay, auth, and observability; avoid storing secrets or executing commands on the relay.
- Keep platform-specific code behind narrow traits so Windows, macOS, and Linux can advance independently.
- Prefer proven libraries for screen capture, input injection, audio, GUI, crypto, and multiplexing.
- Every dangerous operation must have an explicit policy gate and audit event before it becomes functional.

## Milestone 0: Workspace Foundation

- [x] Create Rust workspace.
- [x] Add `server`, `client`, `admin`, and shared `protocol` crates.
- [x] Add `--ip` and `--port` startup arguments for `client` and `admin`.
- [x] Add terminal `server`.
- [x] Add GUI environment detection with terminal fallback for `client` and `admin`.
- [x] Add online client registration and admin-visible client list.
- [x] Add full admin command/menu vocabulary.
- [x] Add command forwarding stubs from `admin` to `client`.

## Milestone 1: Real Transport And Identity

- [ ] Replace ad-hoc text frames with a versioned binary or JSON protocol.
- [ ] Add message ids, correlation ids, errors, request/response envelopes, and heartbeats.
- [ ] Add TLS or Noise-based encryption.
- [ ] Add server-issued session tokens.
- [ ] Add client fingerprints and admin identities.
- [ ] Add reconnect, exponential backoff, and stale-session cleanup.
- [ ] Add structured logs and audit events.
- [ ] Add integration tests for registration, list, command forwarding, reconnect, and offline clients.

## Milestone 2: GUI Shells

- [x] Choose GUI stack: `egui/eframe` for the lightweight native first pass.
- [x] Implement first admin main window with online client table, action panel, logs, and context menu.
- [x] Implement first client window with connection status, server config, client id, and session activity log.
- [ ] Add grouping, status badges, search, filters, and richer admin table controls.
- [ ] Add client consent settings and session permission controls.
- [ ] Preserve terminal mode for headless Linux and recovery workflows.
- [ ] Add packaging assets for Windows, macOS, and Linux.

## Milestone 3: Consent, Policy, And Permissions

- [ ] Add per-client policy model for allowed capabilities.
- [ ] Add local consent prompt for interactive sessions.
- [ ] Add unattended mode with explicit enrollment key.
- [ ] Add audit trail for every admin action.
- [ ] Add role-based admin permissions.
- [ ] Add kill switch to disable all remote control on a client.

## Milestone 4: System Information

- [ ] Implement computer information.
- [ ] Implement clipboard read/write with platform permission checks.
- [ ] Implement active connections.
- [ ] Implement performance monitor.
- [ ] Implement event logs.
- [ ] Implement proxy capability reporting.

## Milestone 5: User Interaction

- [ ] Implement message box.
- [ ] Implement balloon/system notification.
- [ ] Implement text chat.
- [ ] Implement notepad/text opening behavior per platform.
- [ ] Implement voice chat after audio permission and codec spike.

## Milestone 6: File And Terminal Management

- [ ] Implement authenticated file browser.
- [ ] Implement upload/download with resume and hash verification.
- [ ] Implement remote terminal with explicit policy and audit.
- [ ] Add command timeout, output streaming, cancellation, and PTY support.
- [ ] Add safe default deny for privileged commands.

## Milestone 7: System Management

- [ ] Implement process listing and process termination with policy checks.
- [ ] Implement window listing and focus/close actions where supported.
- [ ] Implement startup item management.
- [ ] Implement Windows registry management behind Windows-only feature gates.
- [ ] Implement driver listing with platform-specific backends.
- [ ] Implement shutdown and reboot with confirmation and audit.
- [ ] Implement update, uninstall, and end-client-process flows.

## Milestone 8: Real-Time Desktop Control

- [ ] Add screen capture backend per platform.
- [ ] Add video encoding and adaptive frame rate.
- [ ] Add input injection backend per platform.
- [ ] Add clipboard sync for desktop sessions.
- [ ] Add multi-monitor selection.
- [ ] Add permission prompts for macOS Screen Recording and Accessibility.
- [ ] Add Wayland/X11 support detection and fallback.

## Milestone 9: Media Devices

- [ ] Add camera capability discovery.
- [ ] Add camera streaming with visible local indicator.
- [ ] Add microphone/audio capture with visible local indicator.
- [ ] Add admin-side playback and recording policy.

## Milestone 10: Execution And Automation

- [ ] Implement execute file.
- [ ] Implement code execution only under explicit policy, sandboxing, and audit.
- [ ] Implement static command templates.
- [ ] Implement task creation/scheduling.
- [ ] Implement command presets with signing and versioning.

## Milestone 11: Plugins

- [ ] Define plugin manifest and capability declarations.
- [ ] Add plugin manager UI.
- [ ] Add signed plugin loading.
- [ ] Add plugin sandbox and permission prompts.
- [ ] Add first-party examples for read-only info panels.

## Milestone 12: Production Hardening

- [ ] Add persistent config files.
- [ ] Add auto-update channel.
- [ ] Add service/daemon mode.
- [ ] Add NAT traversal or relay optimization.
- [ ] Add metrics and health checks.
- [ ] Add rate limiting and abuse protection on server.
- [ ] Add release builds for Windows, macOS, and Linux.
- [ ] Add end-to-end tests on all supported platforms.

## Admin Context Menu Map

```text
会话
├─ 客户端
│  ├─ 更新客户端: update_client
│  ├─ 卸载客户端: uninstall_client
│  └─ 结束客户端进程: kill_client_process
│
├─ 系统电源
│  ├─ 关机: shutdown
│  └─ 重启: reboot
│
└─ 会话管理
   ├─ 移动到分组: move_to_group
   ├─ 克隆客户端设置: clone_client_settings
   └─ 删除客户端: delete_client

远程管理
├─ 文件与终端
│  ├─ 文件管理: file_manager
│  └─ 远程终端: remote_terminal
│
├─ 系统管理
│  ├─ 进程管理: process_manager
│  ├─ 窗口管理: window_manager
│  ├─ 启动项管理: startup_manager
│  ├─ 注册表管理: registry_manager
│  ├─ 驱动管理: driver_manager
│  └─ 事件日志: event_log
│
└─ 系统监控
   ├─ 活动连接: active_connections
   └─ 性能监视: performance_monitor

实时控制
├─ 桌面控制
│  └─ 远程桌面: remote_desktop
│
└─ 媒体设备
   ├─ 摄像头: camera
   └─ 音频监听: audio_listen

用户交互
├─ 用户提示
│  ├─ 消息框: message_box
│  └─ 气泡提示: balloon_tip
│
├─ 通信功能
│  ├─ 文本聊天: text_chat
│  └─ 语音聊天: voice_chat
│
└─ 文本交互
   └─ 记事本打开文本: open_text_in_notepad

系统信息
├─ 基础信息
│  ├─ 计算机信息: computer_info
│  └─ 剪贴板: clipboard
│
└─ 网络能力
   └─ 代理: proxy

执行
├─ 代码与文件执行
│  ├─ 执行文件: execute_file
│  └─ 代码执行: execute_code
│
├─ 任务功能
│  ├─ 执行静态命令: execute_static_command
│  └─ 创建任务: create_task
│
└─ 自动化
   └─ 命令预设: command_preset

插件
└─ 扩展功能
   └─ 插件管理: plugin_manager
```
