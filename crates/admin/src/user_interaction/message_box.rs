use crate::i18n::t;
use base64::{engine::general_purpose::STANDARD, Engine};

pub(super) fn payload_for(title: &str, body: &str) -> String {
    [
        format!("title={}", sanitize_single_line(title)),
        format!("message_b64={}", STANDARD.encode(body)),
        "kind=info".to_string(),
    ]
    .join("\n")
}

pub(super) fn default_fields() -> (String, String) {
    ("Rust Desk Light".to_string(), String::new())
}

pub(super) fn title_label() -> &'static str {
    t("Title")
}

pub(super) fn title_hint() -> &'static str {
    "Rust Desk Light"
}

pub(super) fn body_label() -> &'static str {
    t("Message")
}

pub(super) fn sanitize_single_line(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}
