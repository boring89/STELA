// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Builds the C half of the Classic environment (`classic/README.md`):
//! Musashi's `m68kmake` runs on the build machine to generate
//! `m68kops.c`/`m68kops.h`, then Musashi, the 68k wrapper (`c/m68k`), the
//! Macintosh model (`c/mac`) and the shims are compiled with gcc into
//! `libclassic_c.a`.
//!
//! For STELA (`x86_64-unknown-none`) the C code is freestanding: no host
//! headers except gcc's own (`stddef.h`, `stdint.h`, ...), the stand-in
//! headers of `c/freestanding/include` first, no stack protector, no
//! position-independent code (user programs are linked at a fixed
//! address). For the host tests it is plain C against the host's libc.
//! The program itself is linked with `user/user.ld`, like every user
//! program.

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
    let target = env::var("TARGET").expect("TARGET");
    let freestanding = target == "x86_64-unknown-none";
    let musashi = root.join("musashi");
    let c = root.join("c");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=c");
    println!("cargo:rerun-if-changed=musashi");

    // m68kmake is a build-machine tool: always the host compiler.
    let gen_dir = out.join("musashi_gen");
    std::fs::create_dir_all(&gen_dir).expect("create musashi_gen");
    let m68kmake = out.join("m68kmake");
    run(Command::new("gcc")
        .args(["-O1", "-w", "-o"])
        .arg(&m68kmake)
        .arg(musashi.join("m68kmake.c")));
    run(Command::new(&m68kmake)
        .arg(&gen_dir)
        .arg(musashi.join("m68k_in.c"))
        .stdout(std::process::Stdio::null()));

    let mut sources: Vec<(PathBuf, bool)> = vec![
        // (file, our own code: built with warnings as errors)
        (musashi.join("m68kcpu.c"), false),
        (musashi.join("softfloat/softfloat.c"), false),
        (gen_dir.join("m68kops.c"), false),
    ];
    for dir in ["shim", "m68k", "mac"] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(c.join(dir))
            .expect("read_dir")
            .map(|e| e.expect("dir entry").path())
            .filter(|p| p.extension().is_some_and(|e| e == "c"))
            .collect();
        files.sort();
        sources.extend(files.into_iter().map(|f| (f, true)));
    }
    if freestanding {
        sources.push((c.join("freestanding/libc.c"), true));
    }

    let mut common: Vec<String> = vec![
        "-std=gnu11".into(),
        "-O2".into(),
        "-g0".into(),
        format!("-I{}", c.display()),
        format!("-I{}", musashi.display()),
        format!("-I{}", gen_dir.display()),
        "-DMUSASHI_CNF=\"m68k/m68kconf.h\"".into(),
        "-fno-asynchronous-unwind-tables".into(),
    ];
    if freestanding {
        common.extend(
            [
                "-ffreestanding",
                "-fno-stack-protector",
                "-fno-pic",
                "-fno-pie",
                "-mno-red-zone",
                "-nostdinc",
            ]
            .map(String::from),
        );
        common.push(format!(
            "-isystem{}",
            c.join("freestanding/include").display()
        ));
        // gcc's own freestanding headers (stddef.h, stdint.h, stdarg.h,
        // stdbool.h, limits.h's fallback).
        let gcc_include = Command::new("gcc")
            .arg("-print-file-name=include")
            .output()
            .expect("gcc -print-file-name");
        let gcc_include = String::from_utf8(gcc_include.stdout).expect("utf-8");
        common.push(format!("-isystem{}", gcc_include.trim()));
    } else {
        common.push("-fPIC".into());
    }

    let obj_dir = out.join("obj");
    std::fs::create_dir_all(&obj_dir).expect("create obj");
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
    let lib = out.join("libclassic_c.a");
    let _ = std::fs::remove_file(&lib);
    run(Command::new("ar").arg("crs").arg(&lib).args(&objects));
    // Classic.app is a user program: linked with user/user.ld.
    println!(
        "cargo:rustc-link-arg-bins=-T{}",
        root.join("../user/user.ld").display()
    );
    println!("cargo:rerun-if-changed=../user/user.ld");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=classic_c");
}
