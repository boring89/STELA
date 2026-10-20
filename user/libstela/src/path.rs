// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Paths as people type them (`docs/userland.md`): `Volume:/a/b` is
//! absolute, `/a/b` starts at the current volume's root, anything else is
//! relative to the current folder; `.` stays, `..` goes up (never above a
//! volume's root). Resolved by text alone, without asking the kernel.
//! Pure logic, tested on the host.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A place: a volume and the folders below its root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// The volume's name.
    pub volume: String,
    /// The path components from the root.
    pub parts: Vec<String>,
}

impl Location {
    /// The root of `volume`.
    pub fn root(volume: &str) -> Location {
        Location {
            volume: volume.to_string(),
            parts: Vec::new(),
        }
    }

    /// The path below the root, `a/b` (empty at the root).
    pub fn relative(&self) -> String {
        self.parts.join("/")
    }

    /// The last component (the volume name at the root).
    pub fn name(&self) -> &str {
        self.parts.last().map_or(&self.volume, String::as_str)
    }

    /// The folder this is in (itself at the root).
    pub fn parent(&self) -> Location {
        let mut p = self.clone();
        p.parts.pop();
        p
    }
}

impl core::fmt::Display for Location {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}:/{}", self.volume, self.parts.join("/"))
    }
}

/// Why a path was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    /// An empty volume name (`:/x`) or a component too long.
    Invalid,
}

/// Where `input` leads from `cwd`.
pub fn resolve(cwd: &Location, input: &str) -> Result<Location, PathError> {
    let (mut loc, rest) = match input.find(':') {
        Some(colon) if !input[..colon].contains('/') => {
            let volume = &input[..colon];
            if volume.is_empty() {
                return Err(PathError::Invalid);
            }
            (Location::root(volume), &input[colon + 1..])
        }
        _ if input.starts_with('/') => (Location::root(&cwd.volume), input),
        _ => (cwd.clone(), input),
    };
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                loc.parts.pop();
            }
            _ if part.len() > 255 => return Err(PathError::Invalid),
            _ => loc.parts.push(part.to_string()),
        }
    }
    Ok(loc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Location {
        resolve(&Location::root("Data"), s).unwrap()
    }

    #[test]
    fn resolving() {
        let cwd = at("docs/notes");
        assert_eq!(cwd.to_string(), "Data:/docs/notes");
        assert_eq!(
            resolve(&cwd, "a.txt").unwrap().to_string(),
            "Data:/docs/notes/a.txt"
        );
        assert_eq!(resolve(&cwd, "../x").unwrap().to_string(), "Data:/docs/x");
        assert_eq!(resolve(&cwd, "../../../..").unwrap().to_string(), "Data:/");
        assert_eq!(resolve(&cwd, "/top").unwrap().to_string(), "Data:/top");
        assert_eq!(
            resolve(&cwd, "Boot:/EFI/./BOOT").unwrap().to_string(),
            "Boot:/EFI/BOOT"
        );
        assert_eq!(resolve(&cwd, "Initrd:").unwrap().to_string(), "Initrd:/");
        assert_eq!(resolve(&cwd, ".").unwrap(), cwd);
        assert_eq!(resolve(&cwd, ":/x"), Err(PathError::Invalid));
        // A colon after a slash is part of a name.
        assert_eq!(
            resolve(&cwd, "a/b:c").unwrap().relative(),
            "docs/notes/a/b:c"
        );
        assert_eq!(cwd.name(), "notes");
        assert_eq!(cwd.parent().to_string(), "Data:/docs");
        assert_eq!(Location::root("Data").name(), "Data");
    }
}
