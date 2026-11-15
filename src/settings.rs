//! Setup choices, kept separately from the scan cache. A file must contain
//! every choice of its version exactly once; malformed or incomplete files
//! are rejected so a damaged file cannot silently turn off delete
//! confirmation. Version 2 added the appearance; version 3 colours files
//! by type instead of by extension (`color_by_type`); version 4 adds the
//! map's density. Older files still load: the appearance follows Windows
//! and the density is Balanced when a file has none, and colouring by
//! extension becomes colouring by type.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::theme::Appearance;
use crate::treemap::Density;

const HEADER_V1: &str = "TOPOSSETTINGS 1";
const HEADER_V2: &str = "TOPOSSETTINGS 2";
const HEADER_V3: &str = "TOPOSSETTINGS 3";
const HEADER: &str = "TOPOSSETTINGS 4";
const MAX_BYTES: u64 = 4096;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Choices from Setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Show a drive's free space as a tile beside its contents.
    pub show_free_space: bool,
    pub confirm_before_delete: bool,
    /// Colour files by type (video, audio, images, …) instead of by their
    /// folder's depth.
    pub color_by_type: bool,
    /// Draw map captions in the classic 8-point bitmap font rather than the
    /// anti-aliased system font.
    pub classic_font: bool,
    /// Light, dark, or as Windows apps are set.
    pub appearance: Appearance,
    /// How small a tile may be before it joins its folder's group.
    pub density: Density,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_free_space: true,
            confirm_before_delete: true,
            color_by_type: false,
            classic_font: true,
            appearance: Appearance::System,
            density: Density::Balanced,
        }
    }
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "not a usable Topos settings file",
    )
}

impl Settings {
    /// Read every choice. Callers use the defaults on an error.
    ///
    /// # Errors
    /// Missing, unreadable, oversized, incomplete, or malformed files, and
    /// files from another format version.
    pub fn load(path: &Path) -> io::Result<Self> {
        let mut text = String::new();
        File::open(path)?
            .take(MAX_BYTES + 1)
            .read_to_string(&mut text)?;
        if text.len() as u64 > MAX_BYTES {
            return Err(invalid());
        }
        let mut lines = text.lines();
        // Version 1 had the four switches; version 2 added the appearance;
        // version 3 renamed colouring by extension to colouring by type.
        let (every, by_extension) = match lines.next() {
            Some(HEADER_V1) => (15, true),
            Some(HEADER_V2) => (31, true),
            Some(HEADER_V3) => (31, false),
            Some(HEADER) => (63, false),
            _ => return Err(invalid()),
        };
        let mut settings = Self::default();
        let mut seen = 0;
        for line in lines {
            let (key, value) = line.split_once('=').ok_or_else(invalid)?;
            let bit = match key {
                "show_free_space" => 1,
                "confirm_before_delete" => 2,
                "color_by_extension" if by_extension => 4,
                "color_by_type" if !by_extension => 4,
                "classic_font" => 8,
                "appearance" => 16,
                "density" => 32,
                _ => return Err(invalid()),
            };
            if seen & bit != 0 || every & bit == 0 {
                return Err(invalid());
            }
            match key {
                "appearance" => {
                    settings.appearance = Appearance::parse(value).ok_or_else(invalid)?;
                }
                "density" => {
                    settings.density = Density::parse(value).ok_or_else(invalid)?;
                }
                _ => {
                    let flag: bool = value.parse().map_err(|_| invalid())?;
                    match key {
                        "show_free_space" => settings.show_free_space = flag,
                        "confirm_before_delete" => settings.confirm_before_delete = flag,
                        "color_by_extension" | "color_by_type" => settings.color_by_type = flag,
                        _ => settings.classic_font = flag,
                    }
                }
            }
            seen |= bit;
        }
        if seen != every {
            return Err(invalid());
        }
        Ok(settings)
    }

    /// Write the choices through a new file beside `path`, closing it before
    /// replacing the old file. Different instances use different temporary
    /// names; the last successful save wins.
    ///
    /// # Errors
    /// Any error creating, writing, syncing, or renaming the file. A failed
    /// save leaves the previous settings in place.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = File::create_new(&temporary)?;
        let result = (|| {
            writeln!(file, "{HEADER}")?;
            writeln!(file, "show_free_space={}", self.show_free_space)?;
            writeln!(file, "confirm_before_delete={}", self.confirm_before_delete)?;
            writeln!(file, "color_by_type={}", self.color_by_type)?;
            writeln!(file, "classic_font={}", self.classic_font)?;
            writeln!(file, "appearance={}", self.appearance.code())?;
            writeln!(file, "density={}", self.density.code())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("topos-settings-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_combination_survives_repeated_saves_and_loads() {
        let dir = temp_dir("round-trip");
        let path = dir.join("Topos").join("settings.txt");
        for bits in 0..144 {
            let settings = Settings {
                show_free_space: bits & 1 != 0,
                confirm_before_delete: bits & 2 != 0,
                color_by_type: bits & 4 != 0,
                classic_font: bits & 8 != 0,
                appearance: Appearance::ALL[bits / 16 % 3],
                density: Density::ALL[bits / 48],
            };
            settings.save(&path).unwrap();
            assert_eq!(Settings::load(&path).unwrap(), settings);
            assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_or_unusable_files_fall_back_to_safe_defaults() {
        let dir = temp_dir("invalid");
        let path = dir.join("settings.txt");
        let defaults = Settings::default();
        assert!(defaults.confirm_before_delete);
        assert_eq!(Settings::load(&path).unwrap_or_default(), defaults);
        for text in [
            "",
            "TOPOSSETTINGS 2\n",
            "TOPOSSETTINGS 3\n",
            "TOPOSSETTINGS 4\n",
            "TOPOSSETTINGS 5\n",
            // Only version 4 has a density.
            concat!(
                "TOPOSSETTINGS 3\nshow_free_space=false\nconfirm_before_delete=false\n",
                "color_by_type=true\nclassic_font=false\nappearance=dark\ndensity=simple\n"
            ),
            concat!(
                "TOPOSSETTINGS 4\nshow_free_space=false\nconfirm_before_delete=false\n",
                "color_by_type=true\nclassic_font=false\nappearance=dark\ndensity=dense\n"
            ),
            // Version 3 has no colouring by extension; versions 1 and 2 no
            // colouring by type.
            concat!(
                "TOPOSSETTINGS 3\nshow_free_space=false\nconfirm_before_delete=false\n",
                "color_by_extension=true\nclassic_font=false\nappearance=dark\n"
            ),
            concat!(
                "TOPOSSETTINGS 2\nshow_free_space=false\nconfirm_before_delete=false\n",
                "color_by_type=true\nclassic_font=false\nappearance=dark\n"
            ),
            "TOPOSSETTINGS 1\nconfirm_before_delete=false\n",
            concat!(
                "TOPOSSETTINGS 1\nshow_free_space=false\nconfirm_before_delete=false\n",
                "color_by_extension=true\nclassic_font=garbage\n"
            ),
            concat!(
                "TOPOSSETTINGS 1\nshow_free_space=false\nconfirm_before_delete=false\n",
                "color_by_extension=true\nclassic_font=false\nconfirm_before_delete=true\n"
            ),
            concat!(
                "TOPOSSETTINGS 1\nshow_free_space=false\nconfirm_before_delete=false\n",
                "color_by_extension=true\nclassic_font=false\nunknown=true\n"
            ),
            // Version 1 never had an appearance.
            concat!(
                "TOPOSSETTINGS 1\nshow_free_space=false\nconfirm_before_delete=false\n",
                "color_by_extension=true\nclassic_font=false\nappearance=dark\n"
            ),
            // Version 2 needs it, spelled exactly.
            concat!(
                "TOPOSSETTINGS 2\nshow_free_space=false\nconfirm_before_delete=false\n",
                "color_by_extension=true\nclassic_font=false\n"
            ),
            concat!(
                "TOPOSSETTINGS 2\nshow_free_space=false\nconfirm_before_delete=false\n",
                "color_by_extension=true\nclassic_font=false\nappearance=Dark\n"
            ),
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(
                Settings::load(&path).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(Settings::load(&path).unwrap_or_default(), defaults);
        }
        fs::write(&path, [0xff]).unwrap();
        assert_eq!(Settings::load(&path).unwrap_or_default(), defaults);
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert_eq!(Settings::load(&path).unwrap_or_default(), defaults);
        assert_eq!(
            Settings::load(&dir).unwrap_or_default(),
            defaults,
            "unreadable"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn older_files_line_endings_and_reordered_fields_are_accepted() {
        let dir = temp_dir("crlf");
        let path = dir.join("settings.txt");
        fs::write(
            &path,
            concat!(
                "TOPOSSETTINGS 1\r\nclassic_font=false\r\ncolor_by_extension=true\r\n",
                "confirm_before_delete=false\r\nshow_free_space=false\r\n"
            ),
        )
        .unwrap();
        assert_eq!(
            Settings::load(&path).unwrap(),
            Settings {
                show_free_space: false,
                confirm_before_delete: false,
                color_by_type: true,
                classic_font: false,
                appearance: Appearance::System,
                density: Density::Balanced,
            }
        );
        fs::write(
            &path,
            concat!(
                "TOPOSSETTINGS 2\r\nappearance=light\r\nclassic_font=false\r\n",
                "color_by_extension=true\r\nconfirm_before_delete=false\r\n",
                "show_free_space=false\r\n"
            ),
        )
        .unwrap();
        let v2 = Settings::load(&path).unwrap();
        assert_eq!(v2.appearance, Appearance::Light);
        assert!(v2.color_by_type, "colouring by extension carries over");
        fs::write(
            &path,
            concat!(
                "TOPOSSETTINGS 3\nshow_free_space=true\nconfirm_before_delete=true\n",
                "color_by_type=true\nclassic_font=true\nappearance=dark\n"
            ),
        )
        .unwrap();
        let v3 = Settings::load(&path).unwrap();
        assert!(v3.color_by_type && v3.appearance == Appearance::Dark);
        assert_eq!(v3.density, Density::Balanced);
        fs::write(
            &path,
            concat!(
                "TOPOSSETTINGS 4\nshow_free_space=true\nconfirm_before_delete=true\n",
                "color_by_type=false\nclassic_font=true\nappearance=light\ndensity=detailed\n"
            ),
        )
        .unwrap();
        assert_eq!(Settings::load(&path).unwrap().density, Density::Detailed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_replacement_reports_an_error_and_cleans_up() {
        let dir = temp_dir("failed-save");
        let path = dir.join("settings.txt");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), "keep").unwrap();
        assert!(Settings::default().save(&path).is_err());
        assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "keep");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
