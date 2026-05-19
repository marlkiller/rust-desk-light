use super::{startup_payload_field, table_row};
use crate::support::{join_sections, run_powershell};

pub(super) fn handle(payload: &str) -> String {
    let request = RegistryRequest::parse(payload);
    let output = if cfg!(target_os = "windows") {
        windows_registry_manager(&request)
    } else {
        unsupported_registry_table()
    };
    join_sections("registry_manager", vec![output])
}

#[derive(Debug, Eq, PartialEq)]
struct RegistryRequest {
    action: String,
    hive: Option<String>,
    path: Option<String>,
}

impl RegistryRequest {
    fn parse(payload: &str) -> Self {
        let action = startup_payload_field(payload, "action")
            .unwrap_or_else(|| "roots".to_string())
            .to_ascii_lowercase();
        Self {
            action,
            hive: startup_payload_field(payload, "hive"),
            path: startup_payload_field(payload, "path").filter(|value| value != "-"),
        }
    }
}

fn windows_registry_manager(request: &RegistryRequest) -> String {
    let script = match request.action.as_str() {
        "list_key" => {
            let hive = powershell_string(request.hive.as_deref().unwrap_or("HKEY_LOCAL_MACHINE"));
            let path = powershell_string(request.path.as_deref().unwrap_or(""));
            r#"
function Clean($value) {
  if ($null -eq $value) { return "-" }
  if ($value -is [byte[]]) {
    $text = (($value | ForEach-Object { $_.ToString("X2") }) -join " ")
  } elseif ($value -is [array]) {
    $text = (($value | ForEach-Object { [string]$_ }) -join "; ")
  } else {
    $text = [string]$value
  }
  $text = $text -replace "`r|`n|`t", " "
  if ([string]::IsNullOrWhiteSpace($text)) { "-" } else { $text.Trim() }
}
Write-Output "Hive`tPath`tName`tType`tValue"
$count = 0
function EmitRow($hive, $path, $name, $type, $value) {
  $script:count += 1
  "{0}`t{1}`t{2}`t{3}`t{4}" -f (Clean $hive),(Clean $path),(Clean $name),(Clean $type),(Clean $value)
}
function HiveRoot($hive) {
  switch ($hive.ToUpperInvariant()) {
    "HKCR" { "Registry::HKEY_CLASSES_ROOT"; break }
    "HKEY_CLASSES_ROOT" { "Registry::HKEY_CLASSES_ROOT"; break }
    "HKCU" { "Registry::HKEY_CURRENT_USER"; break }
    "HKEY_CURRENT_USER" { "Registry::HKEY_CURRENT_USER"; break }
    "HKLM" { "Registry::HKEY_LOCAL_MACHINE"; break }
    "HKEY_LOCAL_MACHINE" { "Registry::HKEY_LOCAL_MACHINE"; break }
    "HKU" { "Registry::HKEY_USERS"; break }
    "HKEY_USERS" { "Registry::HKEY_USERS"; break }
    "HKCC" { "Registry::HKEY_CURRENT_CONFIG"; break }
    "HKEY_CURRENT_CONFIG" { "Registry::HKEY_CURRENT_CONFIG"; break }
    default { $null }
  }
}
function DisplayHive($hive) {
  switch ($hive.ToUpperInvariant()) {
    "HKCR" { "HKEY_CLASSES_ROOT"; break }
    "HKEY_CLASSES_ROOT" { "HKEY_CLASSES_ROOT"; break }
    "HKCU" { "HKEY_CURRENT_USER"; break }
    "HKEY_CURRENT_USER" { "HKEY_CURRENT_USER"; break }
    "HKLM" { "HKEY_LOCAL_MACHINE"; break }
    "HKEY_LOCAL_MACHINE" { "HKEY_LOCAL_MACHINE"; break }
    "HKU" { "HKEY_USERS"; break }
    "HKEY_USERS" { "HKEY_USERS"; break }
    "HKCC" { "HKEY_CURRENT_CONFIG"; break }
    "HKEY_CURRENT_CONFIG" { "HKEY_CURRENT_CONFIG"; break }
    default { $hive }
  }
}
function JoinRegistryPath($hive, $path) {
  $root = HiveRoot $hive
  if ($null -eq $root) { return $null }
  if ([string]::IsNullOrWhiteSpace($path) -or $path -eq "-") { return $root }
  "$root\$path"
}
function ChildRelativePath($path, $childName) {
  if ([string]::IsNullOrWhiteSpace($path) -or $path -eq "-") { return $childName }
  "$path\$childName"
}
function EmitKey($hive, $path) {
  EmitRow (DisplayHive $hive) $path "(key)" "Key" "-"
}
function EmitValuesAndChildren($hive, $path) {
  $fullPath = JoinRegistryPath $hive $path
  if ($null -eq $fullPath -or !(Test-Path $fullPath)) {
    EmitRow (DisplayHive $hive) (Clean $path) "Error" "Error" "Registry key not found"
    return
  }
  $displayPath = if ([string]::IsNullOrWhiteSpace($path)) { "-" } else { $path }
  EmitKey $hive $displayPath
  $item = Get-Item -LiteralPath $fullPath -ErrorAction Stop
  foreach ($valueName in $item.GetValueNames()) {
    $displayName = if ([string]::IsNullOrEmpty($valueName)) { "(Default)" } else { $valueName }
    $kind = try { $item.GetValueKind($valueName) } catch { "Unknown" }
    $value = try { $item.GetValue($valueName) } catch { "-" }
    EmitRow (DisplayHive $hive) $displayPath $displayName $kind $value
  }
  Get-ChildItem -LiteralPath $fullPath -ErrorAction SilentlyContinue | ForEach-Object {
    EmitKey $hive (ChildRelativePath $displayPath $_.PSChildName)
  }
}
EmitValuesAndChildren __RDL_HIVE__ __RDL_PATH__
if ($count -eq 0) { EmitRow "-" "-" "No registry values found" "Info" "-" }
"#
            .replace("__RDL_HIVE__", &hive)
            .replace("__RDL_PATH__", &path)
        }
        _ => r#"
function Clean($value) {
  if ($null -eq $value) { return "-" }
  $text = [string]$value
  $text = $text -replace "`r|`n|`t", " "
  if ([string]::IsNullOrWhiteSpace($text)) { "-" } else { $text.Trim() }
}
Write-Output "Hive`tPath`tName`tType`tValue"
$count = 0
function EmitRow($hive, $path, $name, $type, $value) {
  $script:count += 1
  "{0}`t{1}`t{2}`t{3}`t{4}" -f (Clean $hive),(Clean $path),(Clean $name),(Clean $type),(Clean $value)
}
function EmitKey($hive, $path) {
  EmitRow $hive $path "(key)" "Key" "-"
}
foreach ($hive in @(
  "HKEY_CLASSES_ROOT",
  "HKEY_CURRENT_USER",
  "HKEY_LOCAL_MACHINE",
  "HKEY_USERS",
  "HKEY_CURRENT_CONFIG"
)) {
  if (Test-Path "Registry::$hive") {
    EmitKey $hive "-"
  }
}
if ($count -eq 0) { EmitRow "-" "-" "No registry keys found" "Info" "-" }
"#
        .to_string(),
    };
    run_powershell(&script, 300)
}

fn powershell_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn unsupported_registry_table() -> String {
    format!(
        "Hive\tPath\tName\tType\tValue\n{}",
        table_row(&[
            "-",
            "-",
            "Unsupported",
            "Info",
            "Registry Manager is only available on Windows",
        ])
    )
}
