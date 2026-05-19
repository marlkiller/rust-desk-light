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

#[cfg(test)]
mod tests {
    use super::payload_for;
    use base64::{engine::general_purpose::STANDARD, Engine};

    #[test]
    fn encodes_message_box_payload() {
        let payload = payload_for("Title", "hello\nworld");

        assert!(payload.contains("title=Title"));
        assert!(payload.contains("kind=info"));
        assert!(payload.contains(&format!("message_b64={}", STANDARD.encode("hello\nworld"))));
    }
}
