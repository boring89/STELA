// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! ehfs-cp: copies files into and out of an eHFS image, and small
//! changes (the command set of 68k_HAL's ehfs-cp, without the xattr
//! mapping of resource forks and Finder information).
//!
//! ```text
//! ehfs-cp [--offset S] IMAGE in HOSTFILE EHFSPATH
//! ehfs-cp [--offset S] IMAGE HOSTFILE EHFSPATH          (same as in)
//! ehfs-cp [--offset S] IMAGE out EHFSPATH HOSTFILE
//! ehfs-cp [--offset S] IMAGE ls [EHFSPATH]
//! ehfs-cp [--offset S] IMAGE mkdir EHFSPATH
//! ehfs-cp [--offset S] IMAGE rm EHFSPATH
//! ```

use ehfs_tools::{ImageFile, describe, now, resolve, split_parent, take_geometry};
use std::process::exit;

fn fail(what: &str) -> ! {
    eprintln!("ehfs-cp: {what}");
    exit(1)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (offset, sectors, rest) = take_geometry(args).unwrap_or_else(|e| fail(&e));
    let Some((target, command)) = rest.split_first() else {
        eprintln!("usage: ehfs-cp [--offset S] IMAGE in|out|ls|mkdir|rm ...");
        exit(2)
    };
    let writes = !matches!(command.first().map(String::as_str), Some("out" | "ls"));
    let image = ImageFile::open(target, writes, offset, sectors).unwrap_or_else(|e| fail(&e));
    let mut v = ehfs::Volume::mount(&image, !writes)
        .unwrap_or_else(|e| fail(&format!("{target}: {}", describe(e))));
    let words: Vec<&str> = command.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        ["in", host, path] | [host, path] if !matches!(*host, "out" | "ls" | "mkdir" | "rm") => {
            copy_in(&mut v, host, path)
        }
        ["out", path, host] => copy_out(&mut v, path, host),
        ["ls"] => ehfs_tools_ls(&mut v, "/"),
        ["ls", path] => ehfs_tools_ls(&mut v, path),
        ["mkdir", path] => split_parent(&mut v, path)
            .and_then(|(dir, name)| v.create(dir, name.as_bytes(), true, now()).map(|_| ()))
            .map_err(|e| format!("{path}: {}", describe(e))),
        ["rm", path] => split_parent(&mut v, path)
            .and_then(|(dir, name)| v.remove(dir, name.as_bytes(), now()))
            .map_err(|e| format!("{path}: {}", describe(e))),
        _ => Err(String::from("unknown command")),
    };
    let unmounted = v.unmount();
    if let Err(e) = result {
        fail(&e);
    }
    if let Err(e) = unmounted {
        fail(&format!("{target}: {}", describe(e)));
    }
}

fn copy_in(v: &mut ehfs::Volume<&ImageFile>, host: &str, path: &str) -> Result<(), String> {
    let data = std::fs::read(host).map_err(|e| format!("{host}: {e}"))?;
    let (dir, name) = split_parent(v, path).map_err(|e| format!("{path}: {}", describe(e)))?;
    let entry = match v.lookup(dir, name.as_bytes()) {
        Ok(e) if !e.is_dir => {
            v.set_length(e.cnid, false, 0, now())
                .map_err(|e| format!("{path}: {}", describe(e)))?;
            e
        }
        Ok(_) => return Err(format!("{path}: is a folder")),
        Err(_) => v
            .create(dir, name.as_bytes(), false, now())
            .map_err(|e| format!("{path}: {}", describe(e)))?,
    };
    v.write(entry.cnid, false, 0, &data, now())
        .map_err(|e| format!("{path}: {}", describe(e)))
}

fn copy_out(v: &mut ehfs::Volume<&ImageFile>, path: &str, host: &str) -> Result<(), String> {
    let e = resolve(v, path).map_err(|e| format!("{path}: {}", describe(e)))?;
    let mut data = vec![0u8; e.size() as usize];
    v.read(e.cnid, false, 0, &mut data)
        .map_err(|e| format!("{path}: {}", describe(e)))?;
    std::fs::write(host, data).map_err(|e| format!("{host}: {e}"))
}

fn ehfs_tools_ls(v: &mut ehfs::Volume<&ImageFile>, path: &str) -> Result<(), String> {
    let dir = resolve(v, path).map_err(|e| format!("{path}: {}", describe(e)))?;
    if !dir.is_dir {
        println!("{:>12}  {}", dir.size(), String::from_utf8_lossy(&dir.name));
        return Ok(());
    }
    for e in v
        .list(dir.cnid)
        .map_err(|e| format!("{path}: {}", describe(e)))?
    {
        let name = String::from_utf8_lossy(&e.name);
        if e.is_dir {
            println!("{:>12}  {name}/", format!("({} items)", e.valence));
        } else {
            println!("{:>12}  {name}", e.size());
        }
    }
    Ok(())
}
