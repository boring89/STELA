// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Relative paths as `DirOpen` takes them: components separated by `/`;
//! empty components and `.` are skipped; `..` is refused (programs resolve
//! it themselves, lexically: libstela). Pure logic, tested on the host.

use alloc::vec::Vec;

/// Why a path was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    /// Not UTF-8, a `..` component, a NUL, or a component over 255 bytes.
    Invalid,
}

/// The components of `path`.
pub fn components(path: &[u8]) -> Result<Vec<&str>, PathError> {
    let s = core::str::from_utf8(path).map_err(|_| PathError::Invalid)?;
    let mut out = Vec::new();
    for c in s.split('/') {
        match c {
            "" | "." => continue,
            ".." => return Err(PathError::Invalid),
            _ if c.len() > abi::NAME_MAX || c.contains('\0') => return Err(PathError::Invalid),
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// The single name in `name` (no `/`, not empty, `.` or `..`).
pub fn single(name: &[u8]) -> Result<&str, PathError> {
    let s = core::str::from_utf8(name).map_err(|_| PathError::Invalid)?;
    if s.is_empty() || s == "." || s == ".." || s.contains(['/', '\0']) || s.len() > abi::NAME_MAX {
        return Err(PathError::Invalid);
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting() {
        assert_eq!(components(b"a/b//c/./d").unwrap(), ["a", "b", "c", "d"]);
        assert_eq!(components(b"").unwrap(), Vec::<&str>::new());
        assert_eq!(components(b"/x/").unwrap(), ["x"]);
        assert_eq!(components(b"a/../b"), Err(PathError::Invalid));
        assert_eq!(components(&[0xFF]), Err(PathError::Invalid));
        assert_eq!(components(&[b'a'; 256]), Err(PathError::Invalid));
        assert_eq!(single(b"name.txt").unwrap(), "name.txt");
        for bad in [&b""[..], b".", b"..", b"a/b", b"a\0"] {
            assert_eq!(single(bad), Err(PathError::Invalid));
        }
    }
}
