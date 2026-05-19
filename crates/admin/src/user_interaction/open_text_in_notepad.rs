use super::message_box::sanitize_single_line;
use crate::i18n::t;
use base64::{engine::general_purpose::STANDARD, Engine};

pub(super) fn payload_for(file_name: &str, text: &str) -> String {
    [
        format!("file_name={}", sanitize_single_line(file_name)),
        format!("text_b64={}", STANDARD.encode(text)),
    ]
    .join("\n")
}

pub(super) fn default_fields() -> (String, String) {
    ("rdl-note.txt".to_string(), String::new())
}

pub(super) fn title_label() -> &'static str {
    t("File Name")
}

pub(super) fn title_hint() -> &'static str {
    "rdl-note.txt"
}

pub(super) fn body_label() -> &'static str {
    t("Text")
}
