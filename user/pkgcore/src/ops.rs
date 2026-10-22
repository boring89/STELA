// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! What `pkg` and Setup.app do on STELA: fetch and verify the index,
//! download, check and unpack packages, remove them, keep
//! `Data:/.pkg/installed.json` (`docs/pkg.md`).

use crate::{
    CONF, PROGRAMS, PUBLIC_KEY, Package, PkgError, STATE, install_order, installed_text,
    parse_index, parse_installed, pkginfo, sha256_hex, tar_entries, verify_signature,
};
use abi::Error;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libstela::fs::{self, open};
use libstela::ipc::handle_close;
use libstela::{abi, http, tls};

/// Seconds for each download step.
const TIMEOUT_NS: u64 = 30_000_000_000;

/// What went wrong, for the user.
pub fn describe(e: &PkgError) -> String {
    match e {
        PkgError::BadIndex => String::from("the index is not valid"),
        PkgError::BadSignature => String::from("the index's signature is wrong"),
        PkgError::BadHash => String::from("the download is not what the index says (SHA-256)"),
        PkgError::Unknown(n) => format!("no package called {n}"),
        PkgError::Cycle(n) => format!("the dependencies of {n} go round in a circle"),
        PkgError::BadArchive => String::from("the package file is broken"),
        PkgError::BadName(n) => format!("{n} cannot be a package name"),
    }
}

/// A failure: a system error or a package problem.
#[derive(Debug)]
pub enum Failure {
    /// From the system (files, network, TLS: `AccessDenied` for a bad
    /// certificate).
    System(Error),
    /// About the packages.
    Package(PkgError),
    /// Something to say as it is.
    Message(String),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Failure {
        Failure::System(e)
    }
}

impl From<PkgError> for Failure {
    fn from(e: PkgError) -> Failure {
        Failure::Package(e)
    }
}

impl Failure {
    /// The message for the user.
    pub fn text(&self) -> String {
        match self {
            Failure::System(e) => String::from(e.name()),
            Failure::Package(p) => describe(p),
            Failure::Message(m) => m.clone(),
        }
    }
}

/// Splits `Volume:path` into the volume's root handle and the inside path.
fn root_of(path: &str) -> Result<(abi::Handle, &str), Error> {
    let (volume, inside) = path.split_once(':').ok_or(Error::InvalidArgument)?;
    Ok((fs::volume_open(volume)?, inside.trim_matches('/')))
}

/// Makes the folder `path` (`Volume:a/b`) and the ones above it.
pub fn make_folders(path: &str) -> Result<(), Error> {
    let (root, inside) = root_of(path)?;
    let mut dir = root;
    let mut result = Ok(());
    for part in inside.split('/').filter(|p| !p.is_empty()) {
        match fs::open(
            dir,
            part,
            open::READ | open::WRITE | open::CREATE | open::DIRECTORY,
        ) {
            Ok(next) => {
                let _ = handle_close(dir);
                dir = next;
            }
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    let _ = handle_close(dir);
    result
}

/// Writes `data` as the file `path`, making its folders.
pub fn write_file(path: &str, data: &[u8]) -> Result<(), Error> {
    let (folder, _) = path.rsplit_once('/').ok_or(Error::InvalidArgument)?;
    make_folders(folder)?;
    let (root, inside) = root_of(path)?;
    let file = fs::open(
        root,
        inside,
        open::READ | open::WRITE | open::CREATE | open::TRUNCATE,
    );
    let _ = handle_close(root);
    let file = file?;
    let mut at = 0;
    let mut result = Ok(());
    while at < data.len() {
        match fs::write(file, &data[at..], at as u64) {
            Ok(0) => {
                result = Err(Error::NoSpace);
                break;
            }
            Ok(n) => at += n,
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    let _ = handle_close(file);
    result
}

/// Removes the folder `path` and everything in it (missing: fine).
pub fn remove_tree(path: &str) -> Result<(), Error> {
    let (root, inside) = root_of(path)?;
    let (parent, name) = match inside.rsplit_once('/') {
        Some((p, n)) => (Some(p), n),
        None => (None, inside),
    };
    let dir = match parent {
        Some(p) => {
            let d = fs::open(root, p, open::READ | open::WRITE | open::DIRECTORY);
            let _ = handle_close(root);
            match d {
                Ok(d) => d,
                // No parent folder: nothing to remove.
                Err(Error::NotFound) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        None => root,
    };
    let result = remove_in(dir, name);
    let _ = handle_close(dir);
    match result {
        Err(Error::NotFound) => Ok(()),
        r => r,
    }
}

/// Removes `name` in folder `dir`, emptying it first if it is a folder.
fn remove_in(dir: abi::Handle, name: &str) -> Result<(), Error> {
    if let Ok(sub) = fs::open(dir, name, open::READ | open::WRITE | open::DIRECTORY) {
        let names: Vec<String> = fs::entries(sub)
            .unwrap_or_default()
            .iter()
            .map(|e| e.name().to_string())
            .collect();
        for n in names {
            let r = remove_in(sub, &n);
            if r.is_err() {
                let _ = handle_close(sub);
                return r;
            }
        }
        let _ = handle_close(sub);
    }
    fs::remove(dir, name)
}

/// The configuration: (server URL, extra trusted roots file).
pub fn conf() -> (Option<String>, Option<String>) {
    let Ok(data) = fs::read_path(CONF) else {
        return (None, None);
    };
    let text = String::from_utf8_lossy(&data);
    (
        crate::conf_value(&text, "server").map(String::from),
        crate::conf_value(&text, "ca").map(String::from),
    )
}

/// The roots trusted for the server: the system's, plus `ca` from the
/// configuration.
fn roots() -> Result<tls::Roots, Error> {
    let mut data = fs::read_path(tls::SYSTEM_ROOTS).unwrap_or_default();
    if let (_, Some(ca)) = conf() {
        data.extend_from_slice(&fs::read_path(&ca)?);
    }
    Ok(tls::Roots::from_der(data))
}

/// `GET url` (http or https), status 200 only.
fn download(url: &str, roots: &tls::Roots) -> Result<Vec<u8>, Failure> {
    let r = http::get_with(url, TIMEOUT_NS, Some(roots))?;
    if r.status != 200 {
        return Err(Failure::Message(format!("{url}: HTTP {}", r.status)));
    }
    Ok(r.body)
}

/// `pkg update`: fetches `index.json` and `index.json.sig` from the
/// server, checks the signature with `Initrd:/certs/pkg.pub`, keeps the
/// index. Returns how many packages it lists.
pub fn update() -> Result<usize, Failure> {
    let (Some(server), _) = conf() else {
        return Err(Failure::Message(format!(
            "no server: write `server = https://...` in {CONF}"
        )));
    };
    let base = if server.ends_with('/') {
        server
    } else {
        format!("{server}/")
    };
    let roots = roots()?;
    let index = download(&format!("{base}index.json"), &roots)?;
    let sig = download(&format!("{base}index.json.sig"), &roots)?;
    let key = fs::read_path(PUBLIC_KEY)?;
    let key = String::from_utf8_lossy(&key);
    if !verify_signature(&key, &index, &String::from_utf8_lossy(&sig)) {
        return Err(PkgError::BadSignature.into());
    }
    let packages = parse_index(&index)?;
    write_file(&format!("{STATE}/index.json"), &index)?;
    Ok(packages.len())
}

/// The System image the kept index offers (P14 K6; `pkg update` first).
pub fn system_release() -> Result<Option<crate::SystemRelease>, Failure> {
    let data = fs::read_path(&format!("{STATE}/index.json"))
        .map_err(|_| Failure::Message(String::from("no index: run `pkg update`")))?;
    Ok(crate::parse_system(&data))
}

/// `GET url` from the package server (its trusted roots), status 200
/// only (P14 K6: `stela-update`).
pub fn fetch(url: &str) -> Result<Vec<u8>, Failure> {
    download(url, &roots()?)
}

/// The packages of the kept index (`pkg update` first).
pub fn index() -> Result<Vec<Package>, Failure> {
    let data = fs::read_path(&format!("{STATE}/index.json"))
        .map_err(|_| Failure::Message(String::from("no index: run `pkg update`")))?;
    Ok(parse_index(&data)?)
}

/// What is installed: (name, version).
pub fn installed() -> Vec<(String, String)> {
    fs::read_path(&format!("{STATE}/installed.json"))
        .map(|d| parse_installed(&d))
        .unwrap_or_default()
}

fn save_installed(list: &[(String, String)]) -> Result<(), Error> {
    write_file(
        &format!("{STATE}/installed.json"),
        installed_text(list).as_bytes(),
    )
}

/// Unpacks package `archive` into `Data:/Programs/NAME/` (replacing what
/// was there) and records it. Returns the package.
pub fn unpack(archive: &[u8]) -> Result<Package, Failure> {
    let entries = tar_entries(archive)?;
    let info = pkginfo(&entries).ok_or(PkgError::BadArchive)?;
    let folder = format!("{PROGRAMS}/{}", info.name);
    remove_tree(&folder)?;
    make_folders(&folder)?;
    for e in &entries {
        let path = format!("{folder}/{}", e.path);
        if e.is_dir {
            make_folders(&path)?;
        } else {
            write_file(&path, e.data)?;
        }
    }
    let mut list = installed();
    list.retain(|(n, _)| n != &info.name);
    list.push((info.name.clone(), info.version.clone()));
    list.sort();
    save_installed(&list)?;
    Ok(info)
}

/// `pkg install NAME...` (with what they need, from the index) or `pkg
/// install ./file.tar`; `progress` hears about each step.
pub fn install(names: &[&str], mut progress: impl FnMut(&str)) -> Result<Vec<Package>, Failure> {
    let mut done = Vec::new();
    let (files, wanted): (Vec<&str>, Vec<&str>) = names.iter().partition(|n| n.ends_with(".tar"));
    for file in files {
        let data = match file.contains(':') {
            true => fs::read_path(file)?,
            false => {
                let h = fs::open(libstela::bootstrap::CWD, file, open::READ)?;
                let d = fs::read_all(h);
                let _ = handle_close(h);
                d?
            }
        };
        progress(&format!("unpacking {file}"));
        done.push(unpack(&data)?);
    }
    if wanted.is_empty() {
        return Ok(done);
    }
    let index = index()?;
    let have: Vec<String> = installed()
        .into_iter()
        .filter(|(n, _)| !wanted.contains(&n.as_str()))
        .map(|(n, _)| n)
        .collect();
    let order = install_order(&index, &wanted, &have)?;
    let roots = roots()?;
    for p in order {
        progress(&format!(
            "downloading {} {} ({} bytes)",
            p.name, p.version, p.size
        ));
        let data = download(&p.url, &roots)?;
        if sha256_hex(&data) != p.sha256.to_ascii_lowercase() {
            return Err(PkgError::BadHash.into());
        }
        progress(&format!("installing {} {}", p.name, p.version));
        let got = unpack(&data)?;
        if got.name != p.name {
            return Err(PkgError::BadArchive.into());
        }
        done.push(got);
    }
    Ok(done)
}

/// `pkg remove NAME`.
pub fn remove(name: &str) -> Result<(), Failure> {
    let mut list = installed();
    if !list.iter().any(|(n, _)| n == name) {
        return Err(Failure::Message(format!("{name} is not installed")));
    }
    remove_tree(&format!("{PROGRAMS}/{name}"))?;
    list.retain(|(n, _)| n != name);
    save_installed(&list)?;
    Ok(())
}

/// The installed packages whose version in the index differs.
pub fn upgradable() -> Result<Vec<String>, Failure> {
    let index = index()?;
    Ok(installed()
        .into_iter()
        .filter(|(n, v)| index.iter().any(|p| &p.name == n && &p.version != v))
        .map(|(n, _)| n)
        .collect())
}

/// The index's packages whose name or description contains `term`
/// (ignoring case).
pub fn search(term: &str) -> Result<Vec<Package>, Failure> {
    let t = term.to_ascii_lowercase();
    Ok(index()?
        .into_iter()
        .filter(|p| {
            p.name.to_ascii_lowercase().contains(&t)
                || p.description.to_ascii_lowercase().contains(&t)
        })
        .collect())
}
