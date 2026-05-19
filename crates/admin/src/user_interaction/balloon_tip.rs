use super::message_box::sanitize_single_line;
use crate::i18n::t;
use base64::{engine::general_purpose::STANDARD, Engine};

pub(super) fn payload_for(title: &str, body: &str) -> String {
    [
        format!("title={}", sanitize_single_line(title)),
        format!("message_b64={}", STANDARD.encode(body)),
    ]
    .join("\n")
}

pub(super) fn default_fields() -> (String, String) {
    (
        "Rust Desk Light".to_string(),
        t("Hello from Rust Desk Light.").to_string(),
    )
}

pub(super) fn body_label() -> &'static str {
    t("Notification")
}
