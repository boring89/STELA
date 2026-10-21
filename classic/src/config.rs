// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Data:/Classic/classic.cfg` (P11, `docs/classic.md` §9): one
//! `key=value` per line, `#` starts a comment; no file means the
//! defaults. Classic.app's arguments of the same form override the file.
//!
//! ```text
//!  ram=16              MiB of RAM, 8 to 1024
//!  resolution=800x600  the NuBus video card in this mode (none: the
//!                      built-in 640x480 video)
//!  depth=8             the card's bits per pixel: 1, 2, 4, 8, 16, 32
//!  scale=2             window pixels per Mac pixel, 1 or 2 (none: 2 on
//!                      screens 1920 or more wide where the window fits)
//!  fs=off              the Data volume is not given to the Mac
//!  disk2=Apps.img      a second disk from Data:/Classic/ (",ro": read
//!                      only)
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Classic.app's settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// RAM in MiB.
    pub ram_mib: u32,
    /// The card's resolution; `None` for the built-in video.
    pub resolution: Option<(u32, u32)>,
    /// The card's bits per pixel.
    pub depth: u32,
    /// Window pixels per Mac pixel; `None`: chosen from the screen.
    pub scale: Option<u32>,
    /// Whether the Mac gets the Data volume.
    pub fs: bool,
    /// A second disk: its file name in `Data:/Classic/` and whether it is
    /// read only.
    pub disk2: Option<(String, bool)>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            ram_mib: 16,
            resolution: None,
            depth: 8,
            scale: None,
            fs: true,
            disk2: None,
        }
    }
}

/// Smallest and largest RAM.
pub const RAM_MIB: core::ops::RangeInclusive<u32> = 8..=1024;
/// Rows of the screen the automatic scale leaves for the menu bar and the
/// window's title bar.
pub const ROOM: u32 = 64;
/// The depths the card has.
pub const DEPTHS: [u32; 6] = [1, 2, 4, 8, 16, 32];

impl Config {
    /// The settings in `text` over the defaults, and a message for every
    /// line that was not understood.
    pub fn parse(text: &str) -> (Config, Vec<String>) {
        let mut c = Config::default();
        let mut problems = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Err(e) = c.set(line) {
                problems.push(format!("line {}: {e}", n + 1));
            }
        }
        (c, problems)
    }

    /// Applies one `key=value` (a line of the file, or an argument).
    pub fn set(&mut self, setting: &str) -> Result<(), String> {
        let Some((key, value)) = setting.split_once('=') else {
            return Err(format!("\"{setting}\" is not key=value"));
        };
        let (key, value) = (key.trim(), value.trim());
        let bad = || format!("bad value for {key}: \"{value}\"");
        match key {
            "ram" => {
                let mib: u32 = value.parse().map_err(|_| bad())?;
                if !RAM_MIB.contains(&mib) {
                    return Err(format!("ram must be 8 to 1024 (MiB), not {mib}"));
                }
                self.ram_mib = mib;
            }
            "resolution" => {
                self.resolution = if value.is_empty() || value == "builtin" {
                    None
                } else {
                    let (w, h) = value.split_once('x').ok_or_else(bad)?;
                    Some((
                        w.trim().parse().map_err(|_| bad())?,
                        h.trim().parse().map_err(|_| bad())?,
                    ))
                };
            }
            "depth" => {
                let bits: u32 = value.parse().map_err(|_| bad())?;
                if !DEPTHS.contains(&bits) {
                    return Err(bad());
                }
                self.depth = bits;
            }
            "scale" => {
                self.scale = match value {
                    "" | "auto" => None,
                    "1" => Some(1),
                    "2" => Some(2),
                    _ => return Err(bad()),
                };
            }
            "fs" => {
                self.fs = match value {
                    "on" | "yes" | "1" => true,
                    "off" | "no" | "0" => false,
                    _ => return Err(bad()),
                };
            }
            "disk2" => {
                self.disk2 = if value.is_empty() || value == "none" {
                    None
                } else {
                    let (name, ro) = match value.rsplit_once(',') {
                        Some((name, "ro")) => (name, true),
                        Some((name, "rw")) => (name, false),
                        Some(_) => return Err(bad()),
                        None => (value, false),
                    };
                    if name.is_empty() || name.contains('/') || name.contains(':') {
                        return Err(bad());
                    }
                    Some((String::from(name.trim()), ro))
                };
            }
            _ => return Err(format!("unknown setting \"{key}\"")),
        }
        Ok(())
    }

    /// The scale for a Mac screen of `mac` pixels on a STELA screen of
    /// `screen` pixels: the setting, or 2 on screens at least 1920 wide
    /// when the doubled window fits (with [`ROOM`] for the menu bar and
    /// the title bar), else 1.
    pub fn scale_for(&self, screen: (u32, u32), mac: (u32, u32)) -> u32 {
        if let Some(scale) = self.scale {
            return scale;
        }
        let fits = mac.0 * 2 <= screen.0 && mac.1 * 2 + ROOM <= screen.1;
        if screen.0 >= 1920 && fits { 2 } else { 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let (c, p) = Config::parse("");
        assert_eq!(c, Config::default());
        assert!(p.is_empty());
        assert_eq!(c.ram_mib, 16);
        assert!(c.fs);
        assert_eq!(c.scale_for((2560, 1600), (640, 480)), 2);
        assert_eq!(c.scale_for((1920, 1080), (640, 480)), 2);
        assert_eq!(c.scale_for((1280, 800), (640, 480)), 1);
        // The doubled window would not fit.
        assert_eq!(c.scale_for((1920, 1080), (1024, 768)), 1);
        assert_eq!(c.scale_for((2560, 1600), (1280, 768)), 2);
        assert_eq!(c.scale_for((2560, 1600), (1280, 800)), 1);
    }

    #[test]
    fn every_setting() {
        let text = "# my Mac\nram = 64\nresolution=1024x768  # big\ndepth=32\nscale=1\nfs=off\n\ndisk2=Apps.img,ro\n";
        let (c, p) = Config::parse(text);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(c.ram_mib, 64);
        assert_eq!(c.resolution, Some((1024, 768)));
        assert_eq!(c.depth, 32);
        assert_eq!(c.scale, Some(1));
        assert_eq!(c.scale_for((4000, 3000), (640, 480)), 1);
        assert!(!c.fs);
        assert_eq!(c.disk2, Some((String::from("Apps.img"), true)));
    }

    #[test]
    fn problems_are_reported_and_skipped() {
        let (c, p) = Config::parse(
            "ram=4\nram=2048\ndepth=3\nscale=3\nfs=maybe\nfoo=1\njunk\ndisk2=a/b\nresolution=800y600",
        );
        assert_eq!(c, Config::default());
        assert_eq!(p.len(), 9);
        assert!(p[0].starts_with("line 1"));
        assert!(p[5].contains("unknown setting"));
    }

    #[test]
    fn arguments_override() {
        let (mut c, _) = Config::parse("resolution=800x600\ndisk2=X.img");
        c.set("resolution=builtin").unwrap();
        c.set("disk2=none").unwrap();
        c.set("disk2=Y.img,rw").unwrap();
        assert_eq!(c.resolution, None);
        assert_eq!(c.disk2, Some((String::from("Y.img"), false)));
        assert!(c.set("ram").is_err());
    }
}
