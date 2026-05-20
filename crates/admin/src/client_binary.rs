#[derive(Clone)]
pub(crate) struct BinaryFormat {
    pub(crate) platform: &'static str,
    pub(crate) format: &'static str,
    pub(crate) arch: String,
}

pub(crate) fn detect_binary_format(bytes: &[u8]) -> BinaryFormat {
    if let Some(format) = detect_pe(bytes) {
        return format;
    }
    if let Some(format) = detect_elf(bytes) {
        return format;
    }
    if let Some(format) = detect_mach_o(bytes) {
        return format;
    }
    BinaryFormat {
        platform: "Unknown",
        format: "Unknown binary",
        arch: "unknown".to_string(),
    }
}

pub(crate) fn binary_platform_matches_client_os(platform: &str, client_os: &str) -> bool {
    let os = client_os.trim().to_ascii_lowercase();
    if os.is_empty() {
        return true;
    }
    match platform {
        "Windows" => os.contains("windows") || os.starts_with("win"),
        "macOS" => os.contains("macos") || os.contains("darwin") || os.contains("os x"),
        "Linux/Unix" => {
            os.contains("linux")
                || os.contains("ubuntu")
                || os.contains("debian")
                || os.contains("fedora")
                || os.contains("centos")
                || os.contains("red hat")
                || os.contains("arch")
                || os.contains("alpine")
                || os.contains("nixos")
                || os.contains("mint")
                || os.contains("unix")
                || os.contains("bsd")
        }
        _ => true,
    }
}

pub(crate) fn binary_arch_matches_client_os(binary_arch: &str, client_os: &str) -> bool {
    let client_arches = known_arches(client_os);
    if client_arches.is_empty() {
        return true;
    }

    let binary_arches = known_arches(binary_arch);
    if binary_arches.is_empty() {
        return false;
    }

    binary_arches
        .iter()
        .any(|binary_arch| client_arches.contains(binary_arch))
}

fn known_arches(value: &str) -> Vec<&'static str> {
    let mut arches = Vec::new();
    for token in value
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
    {
        if let Some(arch) = normalize_arch_token(token) {
            if !arches.contains(&arch) {
                arches.push(arch);
            }
        }
    }
    arches
}

fn normalize_arch_token(token: &str) -> Option<&'static str> {
    match token {
        "x86_64" | "amd64" | "x64" => Some("x86_64"),
        "aarch64" | "arm64" => Some("arm64"),
        "x86" | "i386" | "i486" | "i586" | "i686" => Some("x86"),
        "arm" | "armv6" | "armv7" | "armv7l" | "armhf" | "armel" => Some("arm"),
        "riscv" | "riscv64" => Some("riscv"),
        _ => None,
    }
}

fn detect_pe(bytes: &[u8]) -> Option<BinaryFormat> {
    if !bytes.starts_with(b"MZ") || bytes.len() < 0x40 {
        return None;
    }
    let header_offset = read_u32_le(bytes, 0x3c)? as usize;
    if header_offset.checked_add(6)? > bytes.len()
        || bytes.get(header_offset..header_offset + 4)? != b"PE\0\0"
    {
        return None;
    }
    let machine = read_u16_le(bytes, header_offset + 4)?;
    Some(BinaryFormat {
        platform: "Windows",
        format: "PE",
        arch: pe_arch(machine).to_string(),
    })
}

fn detect_elf(bytes: &[u8]) -> Option<BinaryFormat> {
    if !bytes.starts_with(b"\x7fELF") || bytes.len() < 20 {
        return None;
    }
    let class = match bytes[4] {
        1 => "ELF 32-bit",
        2 => "ELF 64-bit",
        _ => "ELF",
    };
    let machine = match bytes[5] {
        1 => read_u16_le(bytes, 18)?,
        2 => read_u16_be(bytes, 18)?,
        _ => return None,
    };
    Some(BinaryFormat {
        platform: "Linux/Unix",
        format: class,
        arch: elf_arch(machine).to_string(),
    })
}

fn detect_mach_o(bytes: &[u8]) -> Option<BinaryFormat> {
    if bytes.len() < 8 {
        return None;
    }
    let magic = &bytes[..4];
    match magic {
        [0xca, 0xfe, 0xba, 0xbe] | [0xca, 0xfe, 0xba, 0xbf] => detect_mach_o_fat(bytes, true),
        [0xbe, 0xba, 0xfe, 0xca] | [0xbf, 0xba, 0xfe, 0xca] => detect_mach_o_fat(bytes, false),
        [0xfe, 0xed, 0xfa, 0xce] | [0xfe, 0xed, 0xfa, 0xcf] => detect_mach_o_single(bytes, true),
        [0xce, 0xfa, 0xed, 0xfe] | [0xcf, 0xfa, 0xed, 0xfe] => detect_mach_o_single(bytes, false),
        _ => None,
    }
}

fn detect_mach_o_single(bytes: &[u8], big_endian: bool) -> Option<BinaryFormat> {
    let cputype = if big_endian {
        read_u32_be(bytes, 4)?
    } else {
        read_u32_le(bytes, 4)?
    };
    let format = match &bytes[..4] {
        [0xfe, 0xed, 0xfa, 0xcf] | [0xcf, 0xfa, 0xed, 0xfe] => "Mach-O 64-bit",
        _ => "Mach-O 32-bit",
    };
    Some(BinaryFormat {
        platform: "macOS",
        format,
        arch: mach_arch(cputype).to_string(),
    })
}

fn detect_mach_o_fat(bytes: &[u8], big_endian: bool) -> Option<BinaryFormat> {
    let count = if big_endian {
        read_u32_be(bytes, 4)?
    } else {
        read_u32_le(bytes, 4)?
    }
    .min(16);
    let mut archs = Vec::new();
    for index in 0..count as usize {
        let offset = 8 + index * 20;
        if offset + 4 > bytes.len() {
            break;
        }
        let cputype = if big_endian {
            read_u32_be(bytes, offset)?
        } else {
            read_u32_le(bytes, offset)?
        };
        archs.push(mach_arch(cputype).to_string());
    }
    Some(BinaryFormat {
        platform: "macOS",
        format: "Mach-O universal",
        arch: if archs.is_empty() {
            "unknown".to_string()
        } else {
            archs.join(", ")
        },
    })
}

fn pe_arch(machine: u16) -> &'static str {
    match machine {
        0x014c => "x86",
        0x8664 => "x86_64",
        0xaa64 => "arm64",
        0x01c0 | 0x01c4 => "arm",
        _ => "unknown",
    }
}

fn elf_arch(machine: u16) -> &'static str {
    match machine {
        0x0003 => "x86",
        0x003e => "x86_64",
        0x0028 => "arm",
        0x00b7 => "arm64",
        0x00f3 => "riscv",
        _ => "unknown",
    }
}

fn mach_arch(cputype: u32) -> &'static str {
    match cputype {
        0x0000_0007 => "x86",
        0x0100_0007 => "x86_64",
        0x0000_000c => "arm",
        0x0100_000c => "arm64",
        _ => "unknown",
    }
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u16_be(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
