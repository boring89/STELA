// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Builds libchewing (LGPL-2.1-or-later, `docs/ime.md`) with gcc into
//! `libchewing_c.a`. The sources are not in the repository: `make chewing`
//! unpacks the release tarball into `build/chewing/src` (or
//! `CHEWING_SRC=`). Both for STELA and for the host tests the C code is
//! compiled against the stand-in C library of `c/include` and `c/libc.c`
//! (every name with an `ime_` prefix), so the host tests run the same
//! code the IME does.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// libchewing's library sources with the hash user-phrase back end
/// (`src/Makefile.am` without sqlite3).
const SOURCES: &[&str] = &[
    "src/compat.c",
    "src/chewingio.c",
    "src/chewingutil.c",
    "src/choice.c",
    "src/dict.c",
    "src/tree.c",
    "src/bopomofo.c",
    "src/pinyin.c",
    "src/mod_aux.c",
    "src/userphrase.c",
    "src/hash.c",
    "src/userphrase-hash.c",
    "src/common/chewing-utf8-util.c",
    "src/common/key2pho.c",
    "src/porting_layer/src/plat_mmap_posix.c",
    "src/porting_layer/src/plat_path.c",
];

fn run(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("cannot run {cmd:?}: {e}"));
    assert!(status.success(), "{cmd:?} failed with {status}");
}

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let target = env::var("TARGET").expect("TARGET");
    let freestanding = target == "x86_64-unknown-none";
    let src = env::var("CHEWING_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("../../build/chewing/src"));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=c");
    println!("cargo:rerun-if-env-changed=CHEWING_SRC");
    assert!(
        src.join("src/chewingio.c").exists(),
        "libchewing's sources are not in {} (run `make chewing`)",
        src.display()
    );
    let c = root.join("c");

    let gcc_include = Command::new("gcc")
        .arg("-print-file-name=include")
        .output()
        .expect("gcc -print-file-name");
    let gcc_include = String::from_utf8(gcc_include.stdout).expect("utf-8");
    let mut common: Vec<String> = vec![
        "-std=gnu11".into(),
        "-O2".into(),
        "-g0".into(),
        "-ffreestanding".into(),
        "-fno-builtin".into(),
        "-nostdinc".into(),
        format!("-isystem{}", c.join("include").display()),
        // gcc's own freestanding headers (stddef.h, stdint.h, stdarg.h).
        format!("-isystem{}", gcc_include.trim()),
        "-DHAVE_CONFIG_H".into(),
        "-DCHEWING_DATADIR=\"/sys\"".into(),
        format!("-I{}", c.display()),
        format!("-I{}", src.join("include").display()),
        format!("-I{}", src.join("include/internal").display()),
        format!("-I{}", src.join("src/porting_layer/include").display()),
        "-fno-asynchronous-unwind-tables".into(),
    ];
    if freestanding {
        common.extend(
            [
                "-fno-stack-protector",
                "-fno-pic",
                "-fno-pie",
                "-mno-red-zone",
            ]
            .map(String::from),
        );
    } else {
        common.push("-fPIC".into());
    }

    let obj_dir = out.join("obj");
    std::fs::create_dir_all(&obj_dir).expect("create obj");
    let mut sources: Vec<(PathBuf, bool)> = SOURCES.iter().map(|s| (src.join(s), false)).collect();
    sources.push((c.join("libc.c"), true));
    let mut objects = Vec::new();
    for (i, (file, own)) in sources.iter().enumerate() {
        let stem = Path::new(file.file_stem().expect("stem"))
            .display()
            .to_string();
        let obj = obj_dir.join(format!("{i:02}_{stem}.o"));
        let mut cmd = Command::new("gcc");
        cmd.args(&common);
        if let Ok(extra) = env::var("IME_DEBUG_CFLAGS") {
            cmd.args(extra.split_whitespace());
        }
        if *own {
            cmd.args(["-Wall", "-Wextra", "-Werror"]);
        } else {
            cmd.arg("-w");
        }
        run(cmd.arg("-c").arg(file).arg("-o").arg(&obj));
        objects.push(obj);
    }
    let lib = out.join("libchewing_c.a");
    let _ = std::fs::remove_file(&lib);
    run(Command::new("ar").arg("crs").arg(&lib).args(&objects));
    println!(
        "cargo:rustc-link-arg-bins=-T{}",
        root.join("../user.ld").display()
    );
    println!("cargo:rerun-if-changed=../user.ld");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=chewing_c");
}
