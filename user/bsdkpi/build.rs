// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Builds bsdnet's C half (P14 K13-K14, `docs/bsdkpi.md`): FreeBSD's
//! `sys/dev/e1000` (BSD-2-Clause, as it is in `freebsd/dev/e1000`) and
//! the shim (`shim/`, MIT) with gcc into `libbsdkpi_c.a`, freestanding
//! as Classic's C (`classic/build.rs`). The FreeBSD code is compiled
//! without warnings (it is not ours); the shim with `-Werror`.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("cannot run {cmd:?}: {e}"));
    assert!(status.success(), "{cmd:?} failed with {status}");
}

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed=freebsd");
    let e1000 = root.join("freebsd/dev/e1000");
    let mut sources: Vec<(PathBuf, bool)> = std::fs::read_dir(&e1000)
        .expect("read_dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "c"))
        .map(|p| (p, false))
        .collect();
    sources.sort();
    sources.push((root.join("shim/bsdkpi.c"), true));
    let gcc_include = Command::new("gcc")
        .arg("-print-file-name=include")
        .output()
        .expect("gcc -print-file-name");
    let gcc_include = String::from_utf8(gcc_include.stdout).expect("utf-8");
    let common: Vec<String> = vec![
        "-std=gnu11".into(),
        "-O2".into(),
        "-g0".into(),
        "-ffreestanding".into(),
        "-fno-stack-protector".into(),
        "-fno-pic".into(),
        "-fno-pie".into(),
        "-mno-red-zone".into(),
        "-nostdinc".into(),
        "-fno-asynchronous-unwind-tables".into(),
        "-fno-builtin".into(),
        format!("-isystem{}", root.join("shim/include").display()),
        format!("-isystem{}", gcc_include.trim()),
        format!("-I{}", e1000.display()),
    ];
    let obj_dir = out.join("obj");
    std::fs::create_dir_all(&obj_dir).expect("obj");
    let mut objects = Vec::new();
    for (i, (src, own)) in sources.iter().enumerate() {
        let stem = Path::new(src.file_stem().expect("stem"))
            .display()
            .to_string();
        let obj = obj_dir.join(format!("{i:02}_{stem}.o"));
        let mut cmd = Command::new("gcc");
        cmd.args(&common);
        if *own {
            cmd.args(["-Wall", "-Wextra", "-Werror"]);
        } else {
            cmd.arg("-w");
        }
        run(cmd.arg("-c").arg(src).arg("-o").arg(&obj));
        objects.push(obj);
    }
    let lib = out.join("libbsdkpi_c.a");
    let _ = std::fs::remove_file(&lib);
    run(Command::new("ar").arg("crs").arg(&lib).args(&objects));
    println!(
        "cargo:rustc-link-arg-bins=-T{}",
        root.join("../user.ld").display()
    );
    println!("cargo:rerun-if-changed=../user.ld");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=bsdkpi_c");
}
