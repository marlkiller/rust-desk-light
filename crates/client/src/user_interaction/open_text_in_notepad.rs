use super::payload::{clean_result_value, ParsedInteractionPayload};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) fn handle(payload: &str) -> String {
    let payload =
        ParsedInteractionPayload::parse(payload, "rdl-note.txt", String::new(), "text_b64");
    let file_name = safe_text_file_name(&payload.title);
    match write_text_file(&file_name, &payload.body)
        .and_then(|path| open_text_file(&path).map(|open_status| (path, open_status)))
    {
        Ok((path, open_status)) => format!(
            "open_text_in_notepad\nstatus={open_status}\npath={}\nbytes={}",
            clean_result_value(&path.display().to_string()),
            payload.body.len()
        ),
        Err(error) => format!(
            "open_text_in_notepad_error\nmessage={}",
            clean_result_value(&error.to_string())
        ),
    }
}

fn write_text_file(file_name: &str, text: &str) -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("rust-desk-light");
    fs::create_dir_all(&dir)?;
    let path = dir.join(file_name);
    fs::write(&path, text)?;
    Ok(path)
}

fn open_text_file(path: &Path) -> io::Result<&'static str> {
    #[cfg(target_os = "windows")]
    {
        Command::new("notepad")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok("opened_in_notepad")
    }

    #[cfg(target_os = "macos")]
    {
        let textedit = Command::new("open")
            .args(["-a", "TextEdit"])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if textedit.is_ok() {
            return Ok("opened_in_textedit");
        }
        Command::new("open")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        return Ok("opened_with_default_app");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for (program, args) in [
            ("xdg-open", Vec::<&str>::new()),
            ("gedit", Vec::<&str>::new()),
            ("kate", Vec::<&str>::new()),
            ("mousepad", Vec::<&str>::new()),
        ] {
            let result = Command::new(program)
                .args(args)
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if result.is_ok() {
                return Ok("opened_with_platform_editor");
            }
        }
        return Ok("written_no_editor_found");
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        let _ = path;
        Ok("written_no_editor_found")
    }
}

fn safe_text_file_name(value: &str) -> String {
    let mut name = value
        .trim()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect::<String>();
    if name.is_empty() || name == "." || name == ".." {
        name = format!("rdl-note-{}.txt", rdl_protocol::now_epoch_ms());
    }
    let has_txt_extension = name.to_ascii_lowercase().ends_with(".txt");
    let max_stem_len = if has_txt_extension { 120 } else { 116 };
    name = name.chars().take(max_stem_len).collect();
    if !has_txt_extension {
        name.push_str(".txt");
    }
    name
}
