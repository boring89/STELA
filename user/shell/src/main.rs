// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! shell: STELA's command line (`docs/userland.md`). Reads keys from its
//! main port, edits the line, and runs commands on volumes and folders:
//!
//! ```text
//! ls [path]   cat file   cp from to   mkdir path   rm path   mv from to
//! cd [path]   pwd        run program [args...]     ps   mem   uptime
//! vols        help       exit      shutdown  reboot
//! ```
//!
//! Paths: `Volume:/a/b`, `/a/b` (this volume), or relative, with `..`.
//! `run` looks in the current folder, then in `Initrd:/`; the program runs
//! in the foreground (Ctrl-C kills it), with the current folder as its
//! working directory, and the keyboard comes back afterwards.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use libstela::abi::key::{KeyEvent, key_to_char};
use libstela::abi::kind;
use libstela::abi::object_kind;
use libstela::fs::{self, open};
use libstela::ipc::{
    handle_close, handle_duplicate, handle_kind, input_attach, port_receive, port_send,
};
use libstela::line::{Edit, LineEditor};
use libstela::path::{self, Location};
use libstela::process::{self, pass};
use libstela::{Error, Handle, Message, Rights, bootstrap, print, println, tag, tty};

libstela::entry!(main);

/// A handle closed when dropped.
struct Owned(Handle);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = handle_close(self.0);
    }
}

struct Shell {
    cwd: Location,
    /// The window server, when the shell runs in a terminal window: given
    /// to the programs it starts as their `[2]`.
    window_server: Option<Handle>,
}

fn describe(e: Error) -> &'static str {
    match e {
        Error::NotFound => "not found",
        e => e.name(),
    }
}

/// The handle of `loc` (a folder or file), opened with `flags`.
fn open_at(loc: &Location, flags: u64) -> Result<Owned, Error> {
    let root = Owned(fs::volume_open(&loc.volume)?);
    fs::open(root.0, &loc.relative(), flags).map(Owned)
}

impl Shell {
    fn resolve(&self, input: &str) -> Result<Location, &'static str> {
        path::resolve(&self.cwd, input).map_err(|_| "bad path")
    }

    fn prompt(&self) {
        print!("{}> ", self.cwd);
    }

    fn run_line(&mut self, line: &str) -> bool {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some((&command, args)) = words.split_first() else {
            return true;
        };
        let result = match (command, args) {
            ("help", _) => {
                help();
                Ok(())
            }
            ("exit", _) => return false,
            ("pwd", []) => {
                println!("{}", self.cwd);
                Ok(())
            }
            ("cd", []) => {
                self.cwd = Location::root(&self.cwd.volume);
                Ok(())
            }
            ("cd", [to]) => self.cd(to),
            ("ls", []) => self.ls("."),
            ("ls", [p]) => self.ls(p),
            ("cat", [p]) => self.cat(p),
            ("cp", [a, b]) => self.cp(a, b),
            ("mv", [a, b]) => self.mv(a, b),
            ("mkdir", [p]) => self.mkdir(p),
            ("rm", [p]) => self.rm(p),
            ("run", [program, rest @ ..]) => self.run(program, rest),
            // The network tools are programs in the initrd (docs/net.md).
            ("ifconfig" | "ping" | "fetch", rest) => self.run(command, rest),
            ("ps", []) => ps(),
            ("mem", []) => mem(),
            ("uptime", []) => uptime(),
            ("vols", []) => vols(),
            ("shutdown", []) => Err(String::from(describe(process::shutdown()))),
            ("reboot", []) => Err(String::from(describe(process::reboot()))),
            _ => Err(String::from("unknown command or wrong arguments; try help")),
        };
        if let Err(why) = result {
            println!("{command}: {why}");
        }
        true
    }

    fn cd(&mut self, to: &str) -> Result<(), String> {
        let loc = self.resolve(to)?;
        let h = open_at(&loc, open::READ).map_err(|e| alloc::format!("{loc}: {}", describe(e)))?;
        let st = fs::stat(h.0).map_err(|e| String::from(describe(e)))?;
        if st.kind != kind::DIRECTORY {
            return Err(alloc::format!("{loc}: not a folder"));
        }
        self.cwd = loc;
        Ok(())
    }

    fn ls(&self, p: &str) -> Result<(), String> {
        let loc = self.resolve(p)?;
        let dir =
            open_at(&loc, open::READ).map_err(|e| alloc::format!("{loc}: {}", describe(e)))?;
        let st = fs::stat(dir.0).map_err(|e| String::from(describe(e)))?;
        if st.kind != kind::DIRECTORY {
            println!("{:>10}  {}", st.size, loc.name());
            return Ok(());
        }
        let mut index = 0;
        while let Some(e) = fs::read_dir(dir.0, index).map_err(|e| String::from(describe(e)))? {
            if e.kind == kind::DIRECTORY {
                println!("{:>10}  {}/", "", e.name());
            } else {
                println!("{:>10}  {}", e.size, e.name());
            }
            index += 1;
        }
        println!("{index} item(s)");
        Ok(())
    }

    fn read_all(&self, p: &str) -> Result<Vec<u8>, String> {
        let loc = self.resolve(p)?;
        let f = open_at(&loc, open::READ).map_err(|e| alloc::format!("{loc}: {}", describe(e)))?;
        let st = fs::stat(f.0).map_err(|e| String::from(describe(e)))?;
        if st.kind == kind::DIRECTORY {
            return Err(alloc::format!("{loc}: is a folder"));
        }
        let mut data = vec![0u8; st.size as usize];
        let n = fs::read(f.0, &mut data, 0).map_err(|e| String::from(describe(e)))?;
        data.truncate(n);
        Ok(data)
    }

    fn cat(&self, p: &str) -> Result<(), String> {
        let data = self.read_all(p)?;
        for chunk in data.utf8_chunks() {
            print!("{}", chunk.valid());
        }
        if !data.ends_with(b"\n") && !data.is_empty() {
            println!();
        }
        Ok(())
    }

    fn cp(&self, from: &str, to: &str) -> Result<(), String> {
        let data = self.read_all(from)?;
        let loc = self.resolve(to)?;
        let parent = open_at(&loc.parent(), open::READ | open::WRITE)
            .map_err(|e| alloc::format!("{}: {}", loc.parent(), describe(e)))?;
        let f = fs::open(
            parent.0,
            loc.name(),
            open::READ | open::WRITE | open::CREATE | open::TRUNCATE,
        )
        .map(Owned)
        .map_err(|e| alloc::format!("{loc}: {}", describe(e)))?;
        fs::write(f.0, &data, 0).map_err(|e| alloc::format!("{loc}: {}", describe(e)))?;
        println!("{} bytes copied", data.len());
        Ok(())
    }

    fn mv(&self, from: &str, to: &str) -> Result<(), String> {
        let a = self.resolve(from)?;
        let b = self.resolve(to)?;
        if a.volume != b.volume {
            return Err(String::from("cannot move between volumes (use cp)"));
        }
        let pa = open_at(&a.parent(), open::READ | open::WRITE)
            .map_err(|e| alloc::format!("{}: {}", a.parent(), describe(e)))?;
        let pb = open_at(&b.parent(), open::READ | open::WRITE)
            .map_err(|e| alloc::format!("{}: {}", b.parent(), describe(e)))?;
        fs::rename(pa.0, a.name(), pb.0, b.name())
            .map_err(|e| alloc::format!("{a}: {}", describe(e)))
    }

    fn mkdir(&self, p: &str) -> Result<(), String> {
        let loc = self.resolve(p)?;
        let parent = open_at(&loc.parent(), open::READ | open::WRITE)
            .map_err(|e| alloc::format!("{}: {}", loc.parent(), describe(e)))?;
        fs::open(
            parent.0,
            loc.name(),
            open::CREATE | open::DIRECTORY | open::EXCLUSIVE,
        )
        .map(Owned)
        .map(|_| ())
        .map_err(|e| alloc::format!("{loc}: {}", describe(e)))
    }

    fn rm(&self, p: &str) -> Result<(), String> {
        let loc = self.resolve(p)?;
        let parent = open_at(&loc.parent(), open::READ | open::WRITE)
            .map_err(|e| alloc::format!("{}: {}", loc.parent(), describe(e)))?;
        fs::remove(parent.0, loc.name()).map_err(|e| alloc::format!("{loc}: {}", describe(e)))
    }

    /// The program file (or bundle, `Name.app`): in the current folder
    /// (or the given path), then in `Initrd:/`.
    fn find_program(&self, name: &str) -> Result<Owned, String> {
        let runnable = |h: Handle| {
            fs::stat(h).is_ok_and(|s| {
                s.kind == kind::FILE || (s.kind == kind::DIRECTORY && is_bundle(name))
            })
        };
        if let Ok(loc) = self.resolve(name)
            && let Ok(h) = open_at(&loc, open::READ)
            && runnable(h.0)
        {
            return Ok(h);
        }
        if !name.contains(':') && !name.starts_with('/') {
            let initrd = Owned(fs::volume_open("Initrd").map_err(|e| String::from(describe(e)))?);
            if let Ok(h) = fs::open(initrd.0, name, open::READ) {
                let h = Owned(h);
                if runnable(h.0) {
                    return Ok(h);
                }
            }
        }
        Err(alloc::format!("{name}: no such program"))
    }

    fn run(&self, program: &str, rest: &[&str]) -> Result<(), String> {
        let file = self.find_program(program)?;
        let cwd = open_at(&self.cwd, open::READ | open::WRITE)
            .or_else(|_| open_at(&self.cwd, open::READ))
            .map_err(|e| alloc::format!("{}: {}", self.cwd, describe(e)))?;
        let base = program.rsplit(['/', ':']).next().unwrap_or(program);
        let base = base.trim_end_matches('/');
        let base = if is_bundle(base) {
            &base[..base.len() - 4]
        } else {
            base
        };
        let mut args = vec![base];
        args.extend_from_slice(rest);
        // The program's terminal: ours if we run in one (the terminal
        // application's), else a new one that prints on the console. Keys
        // typed while it runs are passed on to it (Linux programs read
        // them there).
        let tty = if handle_kind(bootstrap::CONSOLE) == Ok(object_kind::TTY) {
            Owned(
                handle_duplicate(
                    bootstrap::CONSOLE,
                    Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
                )
                .map_err(|e| String::from(describe(e)))?,
            )
        } else {
            Owned(tty::create().map_err(|e| String::from(describe(e)))?)
        };
        let handles = [
            pass(tty.0, Rights::SEND | Rights::DUPLICATE),
            self.window_server.map_or(Handle::NONE, |ws| {
                pass(ws, Rights::SEND | Rights::DUPLICATE)
            }),
            pass(cwd.0, Rights::READ | Rights::WRITE | Rights::DUPLICATE),
        ];
        let child =
            Owned(process::spawn(file.0, &args, handles).map_err(|e| String::from(describe(e)))?);
        // Wait, watching the keyboard for Ctrl-C (unless the program took
        // the keyboard for itself); other keys go to the program's tty.
        let code = loop {
            match process::wait(child.0, 20_000_000) {
                Ok(code) => break code,
                Err(Error::TimedOut) => {}
                Err(e) => return Err(String::from(describe(e))),
            }
            let mut msg = Message::new();
            while port_receive(bootstrap::MAIN_PORT, &mut msg, 0).is_ok() {
                if msg.tag != tag::INPUT_KEY {
                    continue;
                }
                if KeyEvent::decode(msg.payload()).and_then(|e| key_to_char(&e)) == Some('\u{3}') {
                    println!("^C");
                    let _ = process::kill(child.0);
                } else {
                    let _ = port_send(tty.0, &msg);
                }
            }
        };
        // The keyboard comes back here.
        let _ = input_attach(bootstrap::MAIN_PORT);
        println!("{base} exited with code {code}");
        Ok(())
    }
}

/// Whether `name` names a bundle (`Name.app`).
fn is_bundle(name: &str) -> bool {
    let name = name.trim_end_matches('/');
    name.len() > 4
        && name.is_char_boundary(name.len() - 4)
        && name[name.len() - 4..].eq_ignore_ascii_case(".app")
}

fn help() {
    println!("commands:");
    println!("  ls [path]        list a folder");
    println!("  cat file         show a file");
    println!("  cp from to       copy a file");
    println!("  mv from to       move or rename");
    println!("  mkdir path       make a folder");
    println!("  rm path          remove a file or an empty folder");
    println!("  cd [path]        change the current folder (no path: the volume's root)");
    println!("  pwd              show the current folder");
    println!(
        "  run prog [args]  run a program or Name.app (current folder, then Initrd:/); Ctrl-C stops it"
    );
    println!("  ps, mem, uptime  processes, memory, time since boot");
    println!("  vols             volumes");
    println!("  ifconfig         the network card and address");
    println!("  ping host [n]    send ICMP echo requests");
    println!("  fetch url [file] get http://host[:port]/path (print it or save it)");
    println!("  shutdown, reboot turn the computer off, restart it");
    println!("  exit             leave the shell (init starts a new one)");
    println!("paths: Volume:/a/b, /a/b (this volume) or a/b, with . and ..");
}

fn ps() -> Result<(), String> {
    let list = process::list().map_err(|e| String::from(describe(e)))?;
    println!("{:>5}  {:<16} {:>7}  state", "pid", "name", "threads");
    for p in list.iter().filter(|p| p.state == 1) {
        println!("{:>5}  {:<16} {:>7}  running", p.pid, p.name(), p.threads);
    }
    let ended = list.iter().filter(|p| p.state != 1).count();
    println!("({ended} ended)");
    Ok(())
}

fn mem() -> Result<(), String> {
    let m = process::memory().map_err(|e| String::from(describe(e)))?;
    println!(
        "memory: {} MiB free of {} MiB; kernel heap {} KiB used of {} KiB",
        m.free_frames * 4 / 1024,
        m.total_frames * 4 / 1024,
        m.heap_used / 1024,
        m.heap_size / 1024
    );
    Ok(())
}

fn uptime() -> Result<(), String> {
    let (ns, ticks) = process::uptime().map_err(|e| String::from(describe(e)))?;
    println!(
        "uptime: {}.{:03} s, {} ticks",
        ns / 1_000_000_000,
        ns / 1_000_000 % 1000,
        ticks
    );
    Ok(())
}

fn vols() -> Result<(), String> {
    let mut i = 0;
    while let Some(v) = fs::volume_list(i).map_err(|e| String::from(describe(e)))? {
        let kind = match v.kind {
            libstela::abi::volume_kind::EHFS => "eHFS",
            libstela::abi::volume_kind::FAT => "FAT",
            libstela::abi::volume_kind::INITRD => "initrd",
            libstela::abi::volume_kind::TMP => "memory",
            _ => "?",
        };
        println!(
            "{:<10} {:<7} {}  {} KiB, {} KiB free",
            v.name(),
            kind,
            if v.flags & 1 != 0 { "ro" } else { "rw" },
            v.total / 1024,
            v.free / 1024
        );
        i += 1;
    }
    Ok(())
}

/// The volume the working directory handle `[3]` is on, by its index.
fn start_volume() -> String {
    let index = fs::stat(bootstrap::CWD).map(|s| s.volume);
    let mut i = 0;
    while let Ok(Some(v)) = fs::volume_list(i) {
        if Ok(v.index) == index {
            return String::from(v.name());
        }
        i += 1;
    }
    String::from("Initrd")
}

fn main() -> i64 {
    let mut shell = Shell {
        cwd: Location::root(&start_volume()),
        window_server: None,
    };
    // Report in to whoever started us (init attaches the keyboard).
    let reported = handle_duplicate(bootstrap::MAIN_PORT, Rights::SEND | Rights::TRANSFER)
        .and_then(|h| {
            let mut msg = Message::with_data(tag::SHELL_READY, b"");
            msg.push_handle(h);
            port_send(bootstrap::BOOTSTRAP, &msg)
        });
    if reported.is_err() {
        // Nobody to report to: take the keyboard ourselves.
        let _ = input_attach(bootstrap::MAIN_PORT);
    }
    println!("STELA shell; type help for the commands");
    shell.prompt();
    let mut editor = LineEditor::new(200);
    let mut msg = Message::new();
    loop {
        if port_receive(bootstrap::MAIN_PORT, &mut msg, libstela::ipc::FOREVER).is_err() {
            return 1;
        }
        if msg.tag == tag::WINDOW_SERVER
            && let Some(&ws) = msg.carried().first()
        {
            if let Some(old) = shell.window_server.replace(ws) {
                let _ = handle_close(old);
            }
            continue;
        }
        if msg.tag != tag::INPUT_KEY {
            for &h in msg.carried() {
                let _ = handle_close(h);
            }
            continue;
        }
        let Some(c) = KeyEvent::decode(msg.payload()).and_then(|e| key_to_char(&e)) else {
            continue;
        };
        match editor.feed(c) {
            Edit::None => {}
            Edit::Echo(c) => print!("{c}"),
            Edit::Erase(n) => {
                for _ in 0..n {
                    print!("\u{8} \u{8}");
                }
            }
            Edit::Done(line) => {
                println!();
                if !shell.run_line(&line) {
                    println!("bye");
                    return 0;
                }
                shell.prompt();
            }
        }
    }
}
