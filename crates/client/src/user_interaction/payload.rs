use base64::{engine::general_purpose::STANDARD, Engine};

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ParsedInteractionPayload {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) kind: Option<String>,
}

impl ParsedInteractionPayload {
    pub(super) fn parse(
        payload: &str,
        default_title: impl Into<String>,
        default_body: impl Into<String>,
        encoded_body_key: &str,
    ) -> Self {
        let default_title = default_title.into();
        let default_body = default_body.into();
        let title = payload_field(payload, "title")
            .or_else(|| payload_field(payload, "file_name"))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(default_title);
        let body = payload_field(payload, encoded_body_key)
            .and_then(|value| STANDARD.decode(value).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .or_else(|| payload_field(payload, "message"))
            .or_else(|| payload_field(payload, "text"))
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                let trimmed = payload.trim();
                if trimmed.is_empty() || trimmed.lines().all(|line| line.contains('=')) {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .unwrap_or(default_body);
        let kind = payload_field(payload, "kind").filter(|value| !value.trim().is_empty());

        Self {
            title: single_line(&title),
            body,
            kind: kind.map(|value| value.trim().to_ascii_lowercase()),
        }
    }
}

pub(super) fn clean_result_value(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(str::trim)
        .map(str::to_string)
}

fn single_line(value: &str) -> String {
    let value = value.replace(['\t', '\r', '\n'], " ");
    let value = value.trim();
    if value.is_empty() {
        "Rust Desk Light".to_string()
    } else {
        value.chars().take(120).collect()
    }
}
