use rdl_protocol::{CommandKind, VideoSource};

pub(crate) fn detail_value(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

pub(crate) fn desktop_payload_is_transient_input(payload: &str) -> bool {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("action="))
        .map(|action| matches!(action.trim(), "move" | "key" | "text"))
        .unwrap_or(false)
}

pub(crate) fn desktop_input_reply_payload(result: String) -> String {
    let Some(message) = remote_desktop_error_message(&result) else {
        return result;
    };
    format!("remote_desktop_input\nmessage=input failed: {message}")
}

fn remote_desktop_error_message(detail: &str) -> Option<String> {
    let mut lines = detail.lines();
    if lines.next().unwrap_or_default().trim() != "remote_desktop_error" {
        return None;
    }
    let message = detail
        .lines()
        .find_map(|line| line.strip_prefix("message="))
        .unwrap_or("remote desktop input failed")
        .replace(['\t', '\r', '\n'], " ");
    Some(message)
}

pub(crate) fn remote_desktop_action(payload: &str) -> Option<String> {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("action="))
        .map(|action| action.trim().to_ascii_lowercase())
}

pub(crate) fn remote_desktop_value(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim().to_string())
}

pub(crate) fn video_fps_from_payload(payload: &str, quality: &str) -> u64 {
    remote_desktop_value(payload, "fps")
        .and_then(|value| value.parse::<u64>().ok())
        .map(|fps| fps.clamp(1, 30))
        .unwrap_or_else(|| quality_fps(quality))
}

fn quality_fps(value: &str) -> u64 {
    match value {
        "low" => 10,
        "high" => 2,
        _ => 5,
    }
}

pub(crate) fn video_control_action(payload: &str) -> Option<String> {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("action="))
        .map(|action| action.trim().to_ascii_lowercase())
}

pub(crate) fn video_control_value(payload: &str, key: &str) -> Option<String> {
    remote_desktop_value(payload, key)
}

pub(crate) fn video_source_command(source: &VideoSource) -> CommandKind {
    match source {
        VideoSource::RemoteDesktop => CommandKind::RemoteDesktop,
        VideoSource::Camera => CommandKind::Camera,
    }
}

pub(crate) fn stream_sequence_base(generation: u64) -> u64 {
    generation.saturating_mul(1_u64 << 32).max(1)
}

pub(crate) fn sanitize_log_value(value: &str) -> String {
    let mut value = value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    const MAX_LOG_VALUE_LEN: usize = 180;
    if value.len() > MAX_LOG_VALUE_LEN {
        value.truncate(MAX_LOG_VALUE_LEN);
        value.push_str("...");
    }
    value
}
