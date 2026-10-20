// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Links the program with `user/user.ld`.

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| String::from("."));
    println!("cargo:rustc-link-arg-bins=-T{dir}/../user.ld");
    println!("cargo:rerun-if-changed=../user.ld");
}
