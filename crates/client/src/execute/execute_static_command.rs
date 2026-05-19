use super::shared::{clean_value, payload_field, run_shell};
use base64::{engine::general_purpose::STANDARD, Engine};
use rdl_protocol::static_command_preset;

pub(super) fn handle(payload: &str) -> String {
    if let Some(script) = custom_static_command(payload) {
        let output = run_shell(&script);
        return format!(
            "execute_static_command\nmode=custom\ncommand={}\n{}",
            clean_value(&script),
            output
        );
    }

    let preset_id = payload_field(payload, "preset")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "whoami".to_string());
    let Some(preset) = static_command_preset(&preset_id) else {
        return format!(
            "execute_static_command\nstatus=failed\npreset={}\nmessage=unknown preset",
            clean_value(&preset_id)
        );
    };
    let script = if cfg!(target_os = "windows") {
        preset.windows
    } else {
        preset.unix
    };
    let output = run_shell(script);
    format!(
        "execute_static_command\npreset={}\nlabel={}\n{}",
        clean_value(preset.id),
        clean_value(preset.label),
        output
    )
}

fn custom_static_command(payload: &str) -> Option<String> {
    payload_field(payload, "command_b64")
        .and_then(|value| STANDARD.decode(value).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .or_else(|| payload_field(payload, "command"))
        .filter(|value| !value.trim().is_empty())
}
