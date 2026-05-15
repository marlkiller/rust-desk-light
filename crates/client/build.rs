fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        configure_macos_build();
    }
}

fn configure_macos_build() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let plist = std::path::Path::new(&manifest_dir).join("macos/Info.plist");
    println!(
        "cargo:rustc-link-arg-bin=rdl-client=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
    println!("cargo:rerun-if-changed={}", plist.display());
    println!("cargo:rerun-if-env-changed=RDL_SKIP_MACOS_ADHOC_SIGN");
    println!("cargo:rerun-if-env-changed=RDL_MACOS_CODESIGN_IDENTIFIER");

    if std::env::var_os("RDL_SKIP_MACOS_ADHOC_SIGN").is_some() {
        return;
    }
    spawn_macos_adhoc_signer();
}

fn spawn_macos_adhoc_signer() {
    let out_dir = match std::env::var_os("OUT_DIR") {
        Some(value) => std::path::PathBuf::from(value),
        None => return,
    };
    let Some(profile_dir) = cargo_profile_dir(&out_dir) else {
        return;
    };
    let binary = profile_dir.join("rdl-client");
    let identifier = std::env::var("RDL_MACOS_CODESIGN_IDENTIFIER")
        .unwrap_or_else(|_| "local.rust-desk-light.client".to_string());
    let watch_stamp = out_dir.join("macos-adhoc-codesign.watch");
    let log_path = out_dir.join("macos-adhoc-codesign.log");
    let _ = std::fs::write(&watch_stamp, "watch\n");
    println!("cargo:rerun-if-changed={}", watch_stamp.display());

    let script = r#"
bin="$1"
identifier="$2"
watch_stamp="$3"
log_path="$4"
stable_size=""
stable_count=0
attempt=0

while [ "$attempt" -lt 200 ]; do
    if [ -f "$bin" ] && [ "$bin" -nt "$watch_stamp" ]; then
        size="$(/usr/bin/stat -f %z "$bin" 2>/dev/null || echo "")"
        if [ -n "$size" ] && [ "$size" = "$stable_size" ]; then
            stable_count=$((stable_count + 1))
        else
            stable_size="$size"
            stable_count=0
        fi

        if [ "$stable_count" -ge 2 ]; then
            /usr/bin/codesign --force --sign - --identifier "$identifier" "$bin" >>"$log_path" 2>&1
            /usr/bin/touch "$watch_stamp"
            exit 0
        fi
    fi

    attempt=$((attempt + 1))
    /bin/sleep 0.05
done

echo "timed out waiting to ad-hoc sign $bin" >>"$log_path"
/usr/bin/touch "$watch_stamp"
exit 0
"#;

    let _ = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("rdl-client-adhoc-codesign")
        .arg(binary)
        .arg(identifier)
        .arg(watch_stamp)
        .arg(log_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn cargo_profile_dir(out_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    for ancestor in out_dir.ancestors() {
        if ancestor.file_name().and_then(|name| name.to_str()) == Some("build") {
            return ancestor.parent().map(std::path::Path::to_path_buf);
        }
    }
    None
}
