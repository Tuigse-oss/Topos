//! Puts the Windows resources in `Topos.exe`:
//!
//! - the application manifest (common controls 6 for themed controls and
//!   task dialogs, system DPI awareness, the UTF-8 code page, long paths),
//!   through `embed-manifest`;
//! - the program icon, drawn by the same code the installer and Store
//!   images come from (`src/icon.rs`), in every shell size, plus the
//!   window's own icon group, built so the title bar shows the crisp small
//!   drawing;
//! - the version details that file Properties, Task Manager, and Add or
//!   Remove Programs show.
//!
//! The icon and version are written straight into a `.res` file, which the
//! MSVC linker accepts, so no resource compiler or extra crate is needed.

#![forbid(unsafe_code)]

#[allow(dead_code)]
#[path = "src/icon.rs"]
mod icon;

use std::path::Path;

use embed_manifest::manifest::DpiAwareness;
use embed_manifest::{embed_manifest, new_manifest};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
/// The program icon: what Explorer, Start, and Setup show.
const ICON_ID: u16 = 1;
/// The window's icon (see `icon::render_window`).
const WINDOW_ICON_ID: u16 = 2;
const PUBLISHER: &str = "Tuigse LLC";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/icon.rs");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    let manifest = new_manifest("Topos").dpi_awareness(DpiAwareness::System);
    if let Err(err) = embed_manifest(manifest) {
        panic!("could not embed the application manifest: {err}");
    }
    // A `.res` file is what LINK.EXE takes; other toolchains get the
    // manifest only.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let out_dir = std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR");
    let res = Path::new(&out_dir).join("topos.res");
    std::fs::write(&res, resources()).expect("write topos.res");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// A `.res` file: the two icon groups with their frames, and the version
/// resource.
fn resources() -> Vec<u8> {
    let mut out = Vec::new();
    // Every .res file starts with an empty entry.
    entry(&mut out, 0, 0, 0, &[]);
    let mut next_frame = 1;
    icon_group(
        &mut out,
        ICON_ID,
        &icon::ICO_SIZES,
        icon::render,
        &mut next_frame,
    );
    icon_group(
        &mut out,
        WINDOW_ICON_ID,
        &icon::WINDOW_SIZES,
        icon::render_window,
        &mut next_frame,
    );
    entry(&mut out, RT_VERSION, 1, 0x0030, &version_info());
    out
}

/// One `RT_GROUP_ICON` and its `RT_ICON` frames, numbered from
/// `next_frame` (frame ids are shared by every group in the file).
fn icon_group(
    out: &mut Vec<u8>,
    group_id: u16,
    sizes: &[usize],
    draw: fn(usize) -> icon::Image,
    next_frame: &mut u16,
) {
    let mut group = Vec::new();
    group.extend(0u16.to_le_bytes());
    group.extend(1u16.to_le_bytes());
    group.extend((sizes.len() as u16).to_le_bytes());
    for &size in sizes {
        let id = *next_frame;
        *next_frame += 1;
        let frame = icon::icon_frame(&draw(size));
        // A group entry is the `.ico` directory entry with the frame's
        // resource id in place of its file offset.
        group.extend(icon::icon_dir_fields(size, frame.len()));
        group.extend(id.to_le_bytes());
        // MOVEABLE | DISCARDABLE, as resource compilers mark icons.
        entry(out, RT_ICON, id, 0x1010, &frame);
    }
    entry(out, RT_GROUP_ICON, group_id, 0x1030, &group);
}

/// One resource entry with an ordinal type and name, US English, padded to
/// a DWORD.
fn entry(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    // Header size.
    out.extend(32u32.to_le_bytes());
    out.extend([0xff, 0xff]);
    out.extend(kind.to_le_bytes());
    out.extend([0xff, 0xff]);
    out.extend(name.to_le_bytes());
    // Data version.
    out.extend(0u32.to_le_bytes());
    out.extend(flags.to_le_bytes());
    out.extend(if kind == 0 { 0u16 } else { 0x0409u16 }.to_le_bytes());
    // Version and characteristics.
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(data);
    pad(out);
}

/// `VS_VERSIONINFO`: the numeric version plus the strings Windows shows.
fn version_info() -> Vec<u8> {
    let version = env!("CARGO_PKG_VERSION");
    let parts: Vec<u32> = version
        .split(['.', '-', '+'])
        .map(|part| part.parse().unwrap_or(0))
        .chain(std::iter::repeat(0))
        .take(4)
        .collect();
    let ms = (parts[0] << 16) | parts[1];
    let ls = (parts[2] << 16) | parts[3];
    let mut fixed = Vec::new();
    for value in [
        // Signature and structure version.
        0xfeef_04bdu32,
        0x0001_0000,
        // File version, then product version.
        ms,
        ls,
        ms,
        ls,
        // Flags mask and flags (none set).
        0x3f,
        0,
        // VOS_NT_WINDOWS32, VFT_APP, no subtype, no date.
        0x0004_0004,
        1,
        0,
        0,
        0,
    ] {
        fixed.extend(value.to_le_bytes());
    }
    let copyright = format!("Copyright 2026 {PUBLISHER}");
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", PUBLISHER),
        ("FileDescription", "Topos"),
        ("FileVersion", version),
        ("InternalName", "Topos"),
        ("LegalCopyright", copyright.as_str()),
        ("OriginalFilename", "Topos.exe"),
        ("ProductName", "Topos"),
        ("ProductVersion", version),
    ]
    .iter()
    .map(|(key, value)| {
        let text = utf16z(value);
        node(key, &text, (text.len() / 2) as u16, true, &[])
    })
    .collect();
    // US English, Unicode.
    let table = node("040904B0", &[], 0, true, &strings);
    let string_info = node("StringFileInfo", &[], 0, true, &[table]);
    let mut translation = Vec::new();
    translation.extend(0x0409u16.to_le_bytes());
    translation.extend(0x04b0u16.to_le_bytes());
    let var = node("Translation", &translation, 4, false, &[]);
    let var_info = node("VarFileInfo", &[], 0, true, &[var]);
    node(
        "VS_VERSION_INFO",
        &fixed,
        fixed.len() as u16,
        false,
        &[string_info, var_info],
    )
}

/// One version-resource block: length, value length, type (1 for text),
/// key, value, and children, each aligned to 32 bits.
fn node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0, 0];
    out.extend(value_len.to_le_bytes());
    out.extend(u16::from(text).to_le_bytes());
    out.extend(utf16z(key));
    pad(&mut out);
    out.extend(value);
    for child in children {
        pad(&mut out);
        out.extend(child);
    }
    let len = out.len() as u16;
    out[..2].copy_from_slice(&len.to_le_bytes());
    out
}

fn pad(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}
