// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Linux paths on STELA: one synthetic root over the separate volumes.
//! Pure logic, tested on the host.
//!
//! ```text
//!  /                 the synthetic root: one folder per volume, dev, tmp
//!  /Data/docs/a.txt  Data:/docs/a.txt
//!  /Initrd/linux     Initrd:/linux
//!  /tmp/x            Tmp:/x (the kernel's memory file system)
//!  /dev              null, zero, tty
//! ```
//!
//! `.` and `..` are resolved here, textually (there are no symbolic
//! links); `..` at the root stays at the root.

use alloc::string::String;
use alloc::vec::Vec;

/// Longest path accepted (Linux PATH_MAX).
pub const PATH_MAX: usize = 4096;

/// The devices under `/dev`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// `/dev/null`: reads end at once, writes vanish.
    Null,
    /// `/dev/zero`: reads give zeros.
    Zero,
    /// `/dev/tty`: the process's terminal.
    Tty,
}

/// Where an absolute, normalised path leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// `/`.
    Root,
    /// `/dev`.
    DevDir,
    /// A device.
    Device(Device),
    /// An item on a volume: its name (`tmp` means the `Tmp` volume) and
    /// the path inside it (empty for its root).
    Volume(String, Vec<String>),
}

/// Why a path was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    /// Empty (ENOENT).
    Empty,
    /// Longer than [`PATH_MAX`] (ENAMETOOLONG).
    TooLong,
}

/// The components of `path` (absolute, or relative to `cwd`, which is
/// absolute), with `.` and `..` resolved.
pub fn normalize(cwd: &str, path: &str) -> Result<Vec<String>, PathError> {
    if path.is_empty() {
        return Err(PathError::Empty);
    }
    if path.len() > PATH_MAX {
        return Err(PathError::TooLong);
    }
    let mut parts: Vec<String> = Vec::new();
    let start = if path.starts_with('/') { "" } else { cwd };
    for part in start.split('/').chain(path.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            name => parts.push(String::from(name)),
        }
    }
    Ok(parts)
}

/// The absolute path of `parts` (`/` for none).
pub fn join(parts: &[String]) -> String {
    let mut s = String::new();
    for p in parts {
        s.push('/');
        s.push_str(p);
    }
    if s.is_empty() {
        s.push('/');
    }
    s
}

/// What the normalised `parts` name.
pub fn target(parts: &[String]) -> Target {
    match parts {
        [] => Target::Root,
        [dev] if dev == "dev" => Target::DevDir,
        [dev, name] if dev == "dev" => match name.as_str() {
            "null" => Target::Device(Device::Null),
            "zero" => Target::Device(Device::Zero),
            "tty" => Target::Device(Device::Tty),
            _ => Target::Volume(String::from("dev"), alloc::vec![name.clone()]),
        },
        [first, rest @ ..] => {
            let volume = if first == "tmp" {
                String::from("Tmp")
            } else {
                first.clone()
            };
            Target::Volume(volume, rest.to_vec())
        }
    }
}

/// Splits normalised `parts` into the parent and the last name (`None`
/// for the root).
pub fn split_last(parts: &[String]) -> Option<(&[String], &str)> {
    let (last, parent) = parts.split_last()?;
    Some((parent, last.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn s(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| String::from(*p)).collect()
    }

    #[test]
    fn normalizing() {
        assert_eq!(normalize("/Data", "a/b").unwrap(), s(&["Data", "a", "b"]));
        assert_eq!(normalize("/Data/x", "../y").unwrap(), s(&["Data", "y"]));
        assert_eq!(
            normalize("/Data", "/Initrd/./linux").unwrap(),
            s(&["Initrd", "linux"])
        );
        assert_eq!(normalize("/", "../../..").unwrap(), s(&[]));
        assert_eq!(normalize("/Data", ".").unwrap(), s(&["Data"]));
        assert_eq!(normalize("/Data", "a//b/").unwrap(), s(&["Data", "a", "b"]));
        assert_eq!(normalize("/Data", ""), Err(PathError::Empty));
        let long = "a/".repeat(3000);
        assert_eq!(normalize("/", &long), Err(PathError::TooLong));
    }

    #[test]
    fn targets() {
        assert_eq!(target(&[]), Target::Root);
        assert_eq!(target(&s(&["dev"])), Target::DevDir);
        assert_eq!(target(&s(&["dev", "null"])), Target::Device(Device::Null));
        assert_eq!(target(&s(&["dev", "tty"])), Target::Device(Device::Tty));
        assert_eq!(target(&s(&["dev", "zero"])), Target::Device(Device::Zero));
        assert_eq!(
            target(&s(&["tmp", "x"])),
            Target::Volume(String::from("Tmp"), s(&["x"]))
        );
        assert_eq!(
            target(&s(&["Data"])),
            Target::Volume(String::from("Data"), vec![])
        );
        assert_eq!(
            target(&s(&["Initrd", "linux", "hello"])),
            Target::Volume(String::from("Initrd"), s(&["linux", "hello"]))
        );
    }

    #[test]
    fn joining_and_splitting() {
        assert_eq!(join(&[]), "/");
        assert_eq!(join(&s(&["Data", "a"])), "/Data/a");
        let parts = s(&["Data", "a"]);
        assert_eq!(split_last(&parts), Some((&parts[..1], "a")));
        assert_eq!(split_last(&[]), None);
    }
}
