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
//!  /dev              null, zero, tty, urandom, random, ptmx, pts/N (P12)
//!  /etc              resolv.conf (made from the network's DNS server)
//!  /lib, /usr/lib    shared libraries (P13 K5): Data:/Programs/lib, else
//!                    Initrd:/linux/lib
//!  /proc             self (or the process id): exe, maps, stat, status,
//!                    cmdline, fd/; cpuinfo, meminfo, uptime (P12)
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
    /// `/dev/urandom` and `/dev/random`: random bytes (P12).
    Random,
    /// `/dev/ptmx`: opening it makes a pseudo-terminal (P12).
    Ptmx,
    /// `/dev/pts/N`: the program side of pseudo-terminal N (P12).
    Pts(u32),
}

impl Device {
    /// Its inode number in `stat`.
    pub fn ino(self) -> u64 {
        match self {
            Device::Null => 3,
            Device::Zero => 4,
            Device::Tty => 5,
            Device::Random => 11,
            Device::Ptmx => 12,
            Device::Pts(n) => 0x1000 + u64::from(n),
        }
    }
}

/// What is under `/proc` (P12). `None` as the process means `self`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proc {
    /// `/proc`.
    Dir,
    /// `/proc/self`, `/proc/N`.
    PidDir(Option<u64>),
    /// `exe`: a link to the program.
    Exe(Option<u64>),
    /// `maps`: the memory areas.
    Maps(Option<u64>),
    /// `stat`: one line of numbers.
    Stat(Option<u64>),
    /// `status`: `Name:`, `State:`, ... lines.
    Status(Option<u64>),
    /// `cmdline`: the program name.
    Cmdline(Option<u64>),
    /// `fd/`: the open descriptors.
    FdDir(Option<u64>),
    /// `fd/N`: a link to what descriptor N is.
    Fd(Option<u64>, u64),
    /// `/proc/cpuinfo`.
    Cpuinfo,
    /// `/proc/meminfo`.
    Meminfo,
    /// `/proc/uptime`.
    Uptime,
}

impl Proc {
    /// Whether it is a folder.
    pub fn is_dir(self) -> bool {
        matches!(self, Proc::Dir | Proc::PidDir(_) | Proc::FdDir(_))
    }
}

/// What `/proc/...` names (`rest` after `proc`), if anything.
fn proc_target(rest: &[String]) -> Option<Proc> {
    let Some((first, rest)) = rest.split_first() else {
        return Some(Proc::Dir);
    };
    let pid = match first.as_str() {
        "cpuinfo" if rest.is_empty() => return Some(Proc::Cpuinfo),
        "meminfo" if rest.is_empty() => return Some(Proc::Meminfo),
        "uptime" if rest.is_empty() => return Some(Proc::Uptime),
        "self" | "thread-self" => None,
        n => Some(n.parse::<u64>().ok()?),
    };
    match rest {
        [] => Some(Proc::PidDir(pid)),
        [f] => match f.as_str() {
            "exe" => Some(Proc::Exe(pid)),
            "maps" => Some(Proc::Maps(pid)),
            "stat" => Some(Proc::Stat(pid)),
            "status" => Some(Proc::Status(pid)),
            "cmdline" => Some(Proc::Cmdline(pid)),
            "fd" => Some(Proc::FdDir(pid)),
            _ => None,
        },
        [fd, n] if fd == "fd" => Some(Proc::Fd(pid, n.parse().ok()?)),
        _ => None,
    }
}

/// The generated files under `/etc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Config {
    /// `/etc/resolv.conf`: `nameserver` with the DNS server DHCP gave.
    ResolvConf,
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
    /// `/etc`.
    EtcDir,
    /// A generated file in `/etc`.
    Config(Config),
    /// `/dev/pts` (P12).
    PtsDir,
    /// Something under `/proc` (P12).
    Proc(Proc),
    /// A name under `/proc` or `/dev/pts` that does not exist.
    Missing,
    /// An item on a volume: its name (`tmp` means the `Tmp` volume) and
    /// the path inside it (empty for its root).
    Volume(String, Vec<String>),
    /// Under `/lib` or `/usr/lib` (P13 K5): the path inside the library
    /// folders ([`LIB_FOLDERS`]).
    Lib(Vec<String>),
}

/// Where `/lib` and `/usr/lib` lead, in the order they are searched:
/// volume and folder (P13 K5). Libraries a package installs come first.
pub const LIB_FOLDERS: [(&str, &str, &str); 2] =
    [("Data", "Programs", "lib"), ("Initrd", "linux", "lib")];

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
            "urandom" | "random" => Target::Device(Device::Random),
            "ptmx" => Target::Device(Device::Ptmx),
            "pts" => Target::PtsDir,
            _ => Target::Volume(String::from("dev"), alloc::vec![name.clone()]),
        },
        [dev, pts, n] if dev == "dev" && pts == "pts" => match n.parse() {
            Ok(n) => Target::Device(Device::Pts(n)),
            Err(_) => Target::Missing,
        },
        [proc, rest @ ..] if proc == "proc" => {
            proc_target(rest).map_or(Target::Missing, Target::Proc)
        }
        [lib, rest @ ..] if lib == "lib" => Target::Lib(rest.to_vec()),
        [usr, lib, rest @ ..] if usr == "usr" && lib == "lib" => Target::Lib(rest.to_vec()),
        [etc] if etc == "etc" => Target::EtcDir,
        [etc, name] if etc == "etc" && name == "resolv.conf" => Target::Config(Config::ResolvConf),
        [etc, rest @ ..] if etc == "etc" => Target::Volume(String::from("etc"), rest.to_vec()),
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

/// Whether `parts` (a path from the root, the volume first) is one of the
/// folders `allowed` (absolute paths, e.g. `/Data/Programs/x/container`)
/// or below one (P13 K15: a confined program, `docs/sandbox.md`).
pub fn within(parts: &[String], allowed: &[String]) -> bool {
    allowed.iter().any(|a| {
        let a: Vec<&str> = a.split('/').filter(|c| !c.is_empty()).collect();
        !a.is_empty() && parts.len() >= a.len() && parts.iter().zip(&a).all(|(p, a)| p == a)
    })
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
        assert_eq!(target(&s(&["etc"])), Target::EtcDir);
        assert_eq!(
            target(&s(&["etc", "resolv.conf"])),
            Target::Config(Config::ResolvConf)
        );
        assert_eq!(
            target(&s(&["etc", "hosts"])),
            Target::Volume(String::from("etc"), s(&["hosts"]))
        );
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
        // P13 K5: the library folders.
        assert_eq!(
            target(&s(&["lib", "ld-musl-x86_64.so.1"])),
            Target::Lib(s(&["ld-musl-x86_64.so.1"]))
        );
        assert_eq!(
            target(&s(&["usr", "lib", "libz.so.1"])),
            Target::Lib(s(&["libz.so.1"]))
        );
        assert_eq!(target(&s(&["lib"])), Target::Lib(vec![]));
        assert_eq!(
            target(&s(&["usr", "bin"])),
            Target::Volume(String::from("usr"), s(&["bin"]))
        );
    }

    #[test]
    fn devices_and_proc() {
        assert_eq!(
            target(&s(&["dev", "urandom"])),
            Target::Device(Device::Random)
        );
        assert_eq!(target(&s(&["dev", "ptmx"])), Target::Device(Device::Ptmx));
        assert_eq!(target(&s(&["dev", "pts"])), Target::PtsDir);
        assert_eq!(
            target(&s(&["dev", "pts", "3"])),
            Target::Device(Device::Pts(3))
        );
        assert_eq!(target(&s(&["dev", "pts", "x"])), Target::Missing);
        assert_eq!(target(&s(&["proc"])), Target::Proc(Proc::Dir));
        assert_eq!(
            target(&s(&["proc", "self", "exe"])),
            Target::Proc(Proc::Exe(None))
        );
        assert_eq!(
            target(&s(&["proc", "12", "stat"])),
            Target::Proc(Proc::Stat(Some(12)))
        );
        assert_eq!(
            target(&s(&["proc", "self", "fd", "2"])),
            Target::Proc(Proc::Fd(None, 2))
        );
        assert_eq!(
            target(&s(&["proc", "cpuinfo"])),
            Target::Proc(Proc::Cpuinfo)
        );
        assert_eq!(target(&s(&["proc", "self", "nothing"])), Target::Missing);
        assert_eq!(target(&s(&["proc", "abc"])), Target::Missing);
        assert!(Proc::FdDir(None).is_dir() && !Proc::Maps(None).is_dir());
        assert_eq!(Device::Pts(2).ino(), 0x1002);
    }

    #[test]
    fn joining_and_splitting() {
        assert_eq!(join(&[]), "/");
        assert_eq!(join(&s(&["Data", "a"])), "/Data/a");
        let parts = s(&["Data", "a"]);
        assert_eq!(split_last(&parts), Some((&parts[..1], "a")));
        assert_eq!(split_last(&[]), None);
    }

    #[test]
    fn confined_paths() {
        let allowed = alloc::vec![String::from("/Data/Programs/x/container")];
        assert!(within(
            &s(&["Data", "Programs", "x", "container"]),
            &allowed
        ));
        assert!(within(
            &s(&["Data", "Programs", "x", "container", "a"]),
            &allowed
        ));
        assert!(!within(&s(&["Data", "Programs", "x"]), &allowed));
        assert!(!within(&s(&["Data"]), &allowed));
        assert!(!within(
            &s(&["Data", "Programs", "x", "containers"]),
            &allowed
        ));
        assert!(!within(&s(&["Initrd"]), &allowed));
        assert!(!within(&s(&["Data"]), &[String::from("/")]));
    }
}
