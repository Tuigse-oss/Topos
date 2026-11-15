//! Topos: disk-space treemap visualizer for Windows 10 and 11.
//!
//! The window is native Win32 (`src/win/`); everything it shows and does
//! lives in the `topos` library, which also builds and tests elsewhere.

#![forbid(unsafe_code)]
// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

use std::path::PathBuf;

use clap::Parser;

/// Topos disk-space treemap visualizer.
#[derive(Debug, Parser)]
#[command(name = "topos", version, about = "Disk-space treemap visualizer")]
struct Args {
    /// Drive or folder to scan on startup, e.g. `C:\` or `D:\Games`.
    /// Without it, Topos scans the first drive.
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    let start = args.path.map(start_path);
    run(start)
}

#[cfg(windows)]
fn run(start: Option<PathBuf>) -> std::process::ExitCode {
    win::run(start)
}

/// The window is Win32-only; elsewhere the library and its tests still
/// build, and the program says what it is.
#[cfg(not(windows))]
fn run(start: Option<PathBuf>) -> std::process::ExitCode {
    let _ = start;
    eprintln!("Topos draws its window with Win32 and runs on Windows 10 and 11.");
    std::process::ExitCode::FAILURE
}

/// The command-line path as the scan root: absolute, with an upper-case
/// drive letter (so it matches the drive list), and repaired when cmd.exe
/// quoting turned a trailing `\"` into a literal quote (`"C:\"` arrives as
/// `C:"`).
fn start_path(raw: PathBuf) -> PathBuf {
    let text = repair_cmd_quote(&raw.to_string_lossy());
    let path = std::path::absolute(&text).unwrap_or_else(|_| PathBuf::from(&text));
    upper_drive_letter(path)
}

fn repair_cmd_quote(text: &str) -> String {
    match text.strip_suffix('"') {
        Some(stripped) => format!("{stripped}\\"),
        None => text.to_owned(),
    }
}

fn upper_drive_letter(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_lowercase() && bytes[1] == b':' {
        let mut fixed = text.into_owned();
        fixed[..1].make_ascii_uppercase();
        return PathBuf::from(fixed);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_letters_are_upper_cased() {
        assert_eq!(
            upper_drive_letter(PathBuf::from("c:\\Users")),
            PathBuf::from("C:\\Users")
        );
        assert_eq!(
            upper_drive_letter(PathBuf::from("D:\\")),
            PathBuf::from("D:\\")
        );
        assert_eq!(
            upper_drive_letter(PathBuf::from("/home")),
            PathBuf::from("/home")
        );
    }

    #[test]
    fn a_trailing_quote_from_cmd_becomes_a_separator() {
        assert_eq!(repair_cmd_quote("C:\""), "C:\\");
        assert_eq!(repair_cmd_quote("D:\\My Games\""), "D:\\My Games\\");
        assert_eq!(repair_cmd_quote("D:\\Games"), "D:\\Games");
    }

    #[cfg(windows)]
    #[test]
    fn a_quoted_drive_root_starts_at_that_drive() {
        assert_eq!(start_path(PathBuf::from("c:\"")), PathBuf::from("C:\\"));
    }

    #[test]
    fn relative_paths_become_absolute() {
        assert!(start_path(PathBuf::from("some-folder")).is_absolute());
    }
}
