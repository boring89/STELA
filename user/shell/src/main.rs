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
//!
//! P11 (`docs/userland.md` §6): a line with `|`, `<`, `>`, `>>` or `&`
//! runs programs (no built-in commands) connected by pipes: each one's
//! `[1]` writes into the next one's `[5]`; `< file` and `> file` are fed
//! and drained by the shell through pipes; `&` starts a job in the
//! background (`jobs`, `fg N`, `kill N`).
//!
//! P11 K44: Tab completes the word before the cursor (a command or
//! program name first, a file or folder name after it); arguments with
//! `*` or `?` become the names they match in their folder, sorted (the
//! word stays as it is when nothing matches).

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
use libstela::line::{Edit, Key, LineEditor};
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
    /// Jobs started with `&`.
    jobs: Vec<Job>,
    next_job: u32,
}

/// A pipeline running in the background.
struct Job {
    id: u32,
    /// Its processes, the last one's exit code is the job's.
    procs: Vec<Owned>,
    text: String,
}

/// A program ready to start: its file, the folder it is in, its
/// arguments.
struct Prepared {
    file: Owned,
    folder: Option<Owned>,
    args: Vec<String>,
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
        if libstela::pipeline::is_pipeline(line) {
            if let Err(why) = self.pipeline(line) {
                println!("{why}");
            }
            return true;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let expanded = self.expand(&words);
        let words: Vec<&str> = expanded.iter().map(String::as_str).collect();
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
            ("cp", ["-r", a, b]) => self.cp_tree(a, b),
            ("mv", [a, b]) => self.mv(a, b),
            ("mkdir", [p]) => self.mkdir(p),
            ("rm", [p]) => self.rm(p),
            ("run", [program, rest @ ..]) => self.run(program, rest),

            ("ps", []) => ps(),
            ("jobs", []) => {
                self.list_jobs();
                Ok(())
            }
            ("fg", [n]) => self.fg(n),
            ("kill", [n]) => self.kill_job(n),
            ("mem", []) => mem(),
            ("uptime", []) => uptime(),
            ("vols", []) => vols(),
            ("shutdown", []) => Err(String::from(describe(process::shutdown()))),
            ("reboot", []) => Err(String::from(describe(process::reboot()))),
            // Anything else is a program: in the working folder, in
            // Initrd:/ (the network and sound tools, hwinfo) or in
            // Initrd:/bin/ (P11: the command-line tools, docs/apps.md).
            (name, rest) => match self.find_program(name) {
                Ok(_) => self.run(name, rest),
                Err(_) => Err(String::from("unknown command or wrong arguments; try help")),
            },
        };
        if let Err(why) = result {
            println!("{command}: {why}");
        }
        true
    }

    /// The names in the folder of `word` (relative to the current one)
    /// that `keep` accepts, folders with a `/`; sorted.
    fn names_in(&self, folder: &str, keep: impl Fn(&str) -> bool) -> Vec<String> {
        let folder = if folder.is_empty() { "." } else { folder };
        let Ok(loc) = self.resolve(folder) else {
            return Vec::new();
        };
        let Ok(dir) = open_at(&loc, open::READ) else {
            return Vec::new();
        };
        let mut names: Vec<String> = fs::entries(dir.0)
            .unwrap_or_default()
            .iter()
            .filter(|e| keep(e.name()))
            .map(|e| {
                let mut n = String::from(e.name());
                if e.kind == kind::DIRECTORY {
                    n.push('/');
                }
                n
            })
            .collect();
        names.sort_by_key(|n| n.to_ascii_lowercase());
        names
    }

    /// The arguments with `*` or `?` replaced by the names they match
    /// (the command word stays); hidden names only for a pattern that
    /// starts with `.`.
    fn expand(&self, words: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for (i, w) in words.iter().enumerate() {
            let (folder, pattern) = path::split_last(w);
            if i == 0 || !pattern.contains(['*', '?']) || folder.contains(['*', '?']) {
                out.push(String::from(*w));
                continue;
            }
            let found = self.names_in(folder, |n| {
                (!n.starts_with('.') || pattern.starts_with('.')) && path::glob(pattern, n)
            });
            if found.is_empty() {
                out.push(String::from(*w));
            }
            for n in found {
                out.push(alloc::format!("{folder}{}", n.trim_end_matches('/')));
            }
        }
        out
    }

    /// What `word` may become: a command or program for the first word,
    /// else a file or folder (folders end in `/`).
    fn completions(&self, word: &str, first: bool) -> Vec<String> {
        let (folder, start) = path::split_last(word);
        let starts = |n: &str| {
            n.len() >= start.len()
                && n.is_char_boundary(start.len())
                && n[..start.len()].eq_ignore_ascii_case(start)
        };
        if !first || !folder.is_empty() {
            return self.names_in(folder, |n| {
                starts(n) && (!n.starts_with('.') || start.starts_with('.'))
            });
        }
        let mut names: Vec<String> = BUILTINS
            .iter()
            .filter(|n| starts(n))
            .map(|n| String::from(*n))
            .collect();
        // Programs: here and on the search path (bundles too).
        for folder in [
            ".",
            "Initrd:/",
            "Initrd:/bin",
            "System:/Programs",
            "System:/bin",
        ] {
            for n in self.names_in(folder, starts) {
                if !n.ends_with('/') || is_bundle(&n) {
                    names.push(String::from(n.trim_end_matches('/')));
                }
            }
        }
        names.sort_by_key(|n| n.to_ascii_lowercase());
        names.dedup();
        names
    }

    /// Tab: completes the word before the cursor; with several choices,
    /// adds what they share, or lists them and shows the line again.
    fn complete(&self, editor: &mut LineEditor) {
        let (word, first) = editor.word();
        let names = self.completions(&word, first);
        let typed = path::split_last(&word).1.chars().count();
        let rest = |n: &str| n.chars().skip(typed).collect::<String>();
        let insert = match names.as_slice() {
            [] => return,
            [one] if one.ends_with('/') => rest(one),
            [one] => alloc::format!("{} ", rest(one)),
            _ => rest(&path::common_prefix(&names)),
        };
        if !insert.is_empty() {
            if let Edit::Show(s) = editor.insert(&insert) {
                print!("{s}");
            }
            return;
        }
        println!();
        println!("{}", names.join("  "));
        self.prompt();
        print!("{}", editor.redraw());
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
        let mut cookie = 0;
        let mut index = 0;
        while let Some((e, next)) =
            fs::read_dir(dir.0, cookie).map_err(|e| String::from(describe(e)))?
        {
            if e.kind == kind::DIRECTORY {
                println!("{:>10}  {}/", "", e.name());
            } else {
                println!("{:>10}  {}", e.size, e.name());
            }
            cookie = next;
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

    /// `cp -r`: a folder and everything in it (P11); a file as `cp`.
    fn cp_tree(&self, from: &str, to: &str) -> Result<(), String> {
        let a = self.resolve(from)?;
        let b = self.resolve(to)?;
        let src = open_at(&a, open::READ).map_err(|e| alloc::format!("{a}: {}", describe(e)))?;
        let st = fs::stat(src.0).map_err(|e| String::from(describe(e)))?;
        if st.kind != kind::DIRECTORY {
            return self.cp(from, to);
        }
        if a.volume.eq_ignore_ascii_case(&b.volume) && b.parts.starts_with(&a.parts) {
            return Err(alloc::format!("{b}: inside {a}"));
        }
        let parent = open_at(&b.parent(), open::READ | open::WRITE)
            .map_err(|e| alloc::format!("{}: {}", b.parent(), describe(e)))?;
        let (files, bytes) = copy_tree(src.0, parent.0, b.name(), 0)?;
        println!("{files} file(s), {bytes} bytes copied");
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
    /// (or the given path), then in `Initrd:/`; with the folder it is in
    /// (the program's `[4]`).
    fn find_program(&self, name: &str) -> Result<(Owned, Option<Owned>), String> {
        let runnable = |h: Handle| {
            fs::stat(h).is_ok_and(|s| {
                s.kind == kind::FILE || (s.kind == kind::DIRECTORY && is_bundle(name))
            })
        };
        if let Ok(loc) = self.resolve(name)
            && let Ok(h) = open_at(&loc, open::READ)
            && runnable(h.0)
        {
            let folder = open_at(&loc.parent(), open::READ).ok();
            return Ok((h, folder));
        }
        if !name.contains(':') && !name.starts_with('/') {
            // The search path (P11: Initrd:/, Initrd:/bin/; P14 K4: then
            // System:/Programs/, System:/bin/).
            for (volume, dir) in SEARCH_PATH {
                if !dir.is_empty() && name.contains('/') {
                    continue;
                }
                let Ok(root) = fs::volume_open(volume).map(Owned) else {
                    continue;
                };
                let base = if dir.is_empty() {
                    root
                } else {
                    match fs::open(root.0, dir, open::READ) {
                        Ok(h) => Owned(h),
                        Err(_) => continue,
                    }
                };
                if let Ok(h) = fs::open(base.0, name, open::READ) {
                    let h = Owned(h);
                    if runnable(h.0) {
                        let folder = match name.trim_end_matches('/').rsplit_once('/') {
                            Some((d, _)) => fs::open(base.0, d, open::READ).ok().map(Owned),
                            None => Some(base),
                        };
                        return Ok((h, folder));
                    }
                }
            }
            // And last (P12): the installed packages' bin folders,
            // Data:/Programs/NAME/bin/ (docs/pkg.md).
            if !name.contains('/')
                && let Some(found) = find_in_programs(name, &runnable)
            {
                return Ok(found);
            }
        }
        Err(alloc::format!("{name}: no such program"))
    }

    /// The program `program` with arguments `rest`, found and named.
    fn prepare(&self, program: &str, rest: &[String]) -> Result<Prepared, String> {
        let (file, folder) = self.find_program(program)?;
        let base = program.rsplit(['/', ':']).next().unwrap_or(program);
        let base = base.trim_end_matches('/');
        let base = if is_bundle(base) {
            &base[..base.len() - 4]
        } else {
            base
        };
        let mut args = vec![String::from(base)];
        args.extend(rest.iter().cloned());
        Ok(Prepared { file, folder, args })
    }

    /// Our terminal for a program: ours if we run in one, else a new one
    /// that prints on the console.
    fn program_tty(&self) -> Result<Owned, String> {
        if handle_kind(bootstrap::CONSOLE) == Ok(object_kind::TTY) {
            handle_duplicate(
                bootstrap::CONSOLE,
                Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
            )
            .map(Owned)
            .map_err(|e| String::from(describe(e)))
        } else {
            tty::create()
                .map(Owned)
                .map_err(|e| String::from(describe(e)))
        }
    }

    /// Starts `p` with `out` as its `[1]` and `input` as its `[5]`.
    fn start(&self, p: &Prepared, out: Handle, input: Handle) -> Result<Owned, String> {
        let cwd = open_at(&self.cwd, open::READ | open::WRITE)
            .or_else(|_| open_at(&self.cwd, open::READ))
            .map_err(|e| alloc::format!("{}: {}", self.cwd, describe(e)))?;
        // A pipe's writing end (with WRITE) or a terminal (without).
        let out = match pass(out, Rights::SEND | Rights::WRITE | Rights::DUPLICATE) {
            Handle::NONE => pass(out, Rights::SEND | Rights::DUPLICATE),
            h => h,
        };
        let handles = [
            out,
            self.window_server.map_or(Handle::NONE, |ws| {
                pass(ws, Rights::SEND | Rights::DUPLICATE)
            }),
            process::pass_dir(cwd.0),
            p.folder
                .as_ref()
                .map_or(Handle::NONE, |f| process::pass_dir(f.0)),
            if input == Handle::NONE {
                Handle::NONE
            } else {
                pass(input, Rights::READ | Rights::WAIT | Rights::DUPLICATE)
            },
        ];
        let mut caps = process::inherited();
        if let Some(i) = process::pass_capability(libstela::abi::capability::INPUT) {
            caps.push((libstela::abi::capability::INPUT, i));
        }
        let args: Vec<&str> = p.args.iter().map(String::as_str).collect();
        process::spawn_with(p.file.0, &args, &handles, &caps)
            .map(Owned)
            .map_err(|e| alloc::format!("{}: {}", p.args[0], describe(e)))
    }

    /// A line with pipes, redirections or `&`.
    fn pipeline(&mut self, line: &str) -> Result<(), String> {
        use libstela::pipe;
        let p = libstela::pipeline::parse(line).map_err(|e| String::from(e.describe()))?;
        if p.background && (p.input.is_some() || p.output.is_some()) {
            return Err(String::from("< and > need the foreground"));
        }
        let mut prepared = Vec::new();
        for s in &p.stages {
            let words: Vec<&str> = s.args.iter().map(String::as_str).collect();
            let args = self.expand(&words);
            let (name, rest) = match args.split_first() {
                Some((run, rest)) if run == "run" && !rest.is_empty() => (&rest[0], &rest[1..]),
                Some((name, rest)) => (name, rest),
                None => return Err(String::from("nothing to run")),
            };
            prepared.push(self.prepare(name, rest)?);
        }
        // The files: what we feed in and what we fill.
        let input = match &p.input {
            Some(path) => {
                let loc = self.resolve(path)?;
                Some(
                    open_at(&loc, open::READ)
                        .map_err(|e| alloc::format!("{loc}: {}", describe(e)))?,
                )
            }
            None => None,
        };
        let output = match &p.output {
            Some((path, append)) => {
                let loc = self.resolve(path)?;
                let mut flags = open::READ | open::WRITE | open::CREATE;
                if !append {
                    flags |= open::TRUNCATE;
                }
                let f =
                    open_at(&loc, flags).map_err(|e| alloc::format!("{loc}: {}", describe(e)))?;
                let at = if *append {
                    fs::stat(f.0).map(|s| s.size).unwrap_or(0)
                } else {
                    0
                };
                Some((f, at))
            }
            None => None,
        };
        let tty = self.program_tty()?;
        let pipe = |_: ()| pipe::create().map(|(r, w)| (Owned(r), Owned(w)));
        let err = |e: Error| String::from(describe(e));
        // Pipes between the stages, and around the files.
        let mut links = Vec::new();
        for _ in 1..prepared.len() {
            links.push(pipe(()).map_err(err)?);
        }
        let feed = if input.is_some() {
            Some(pipe(()).map_err(err)?)
        } else {
            None
        };
        let drain = if output.is_some() {
            Some(pipe(()).map_err(err)?)
        } else {
            None
        };
        let mut procs = Vec::new();
        let last = prepared.len() - 1;
        for (i, prog) in prepared.iter().enumerate() {
            let out = if i < last {
                links[i].1.0
            } else {
                drain.as_ref().map_or(tty.0, |d| d.1.0)
            };
            let input = if i > 0 {
                links[i - 1].0.0
            } else {
                feed.as_ref().map_or(Handle::NONE, |f| f.0.0)
            };
            match self.start(prog, out, input) {
                Ok(h) => procs.push(h),
                Err(e) => {
                    for h in &procs {
                        let _ = process::kill(h.0);
                    }
                    return Err(e);
                }
            }
        }
        // Only the children hold the pipes between them now.
        drop(links);
        let (feed_w, drain_r) = (
            feed.map(|(r, w)| {
                drop(r);
                w
            }),
            drain.map(|(r, w)| {
                drop(w);
                r
            }),
        );
        let text = String::from(line.trim().trim_end_matches('&').trim());
        if p.background {
            let id = self.next_job;
            self.next_job += 1;
            println!("[{id}] {text}");
            self.jobs.push(Job { id, procs, text });
            return Ok(());
        }
        let base = prepared[last].args[0].clone();
        let code = self.foreground(&procs, &tty, input.zip(feed_w), output.zip(drain_r))?;
        println!("{base} exited with code {code}");
        Ok(())
    }

    /// Waits for the processes (the last one's exit code), passing keys
    /// on, feeding `input` into its pipe and draining the output pipe
    /// into its file.
    fn foreground(
        &self,
        procs: &[Owned],
        tty: &Owned,
        mut input: Option<(Owned, Owned)>,
        mut output: Option<((Owned, u64), Owned)>,
    ) -> Result<i64, String> {
        use libstela::pipe;
        let Some(last) = procs.last() else {
            return Err(String::from("nothing to run"));
        };
        let mut in_at = 0u64;
        let mut chunk = vec![0u8; 4096];
        let mut code = None;
        loop {
            // Feed: as much as the pipe takes now.
            if let Some((file, w)) = &input {
                let n = fs::read(file.0, &mut chunk, in_at).unwrap_or(0);
                if n == 0 {
                    input = None; // the end: closing the pipe tells the reader
                } else {
                    match pipe::write(w.0, &chunk[..n], 0) {
                        Ok(k) => in_at += k as u64,
                        Err(Error::TimedOut) => {}
                        Err(_) => input = None,
                    }
                }
            }
            // Drain: what came out so far.
            if let Some(((file, at), r)) = &mut output {
                loop {
                    match pipe::read(r.0, &mut chunk, 0) {
                        Ok(0) => {
                            output = None;
                            break;
                        }
                        Ok(n) => {
                            let _ = fs::write(file.0, &chunk[..n], *at);
                            *at += n as u64;
                        }
                        Err(_) => break,
                    }
                }
            }
            if code.is_none() {
                match process::wait(last.0, 10_000_000) {
                    Ok(c) => code = Some(c),
                    Err(Error::TimedOut) => {}
                    Err(e) => return Err(String::from(describe(e))),
                }
            }
            if let Some(c) = code
                && output.is_none()
            {
                // The others: a moment to finish, then they go.
                for p in procs {
                    if process::wait(p.0, 1_000_000_000).is_err() {
                        let _ = process::kill(p.0);
                    }
                }
                let _ = input_attach(bootstrap::MAIN_PORT);
                return Ok(c);
            }
            if code.is_some() {
                // Wait for the rest of the output.
                libstela::sleep_ms(1);
            }
            let mut msg = Message::new();
            while port_receive(bootstrap::MAIN_PORT, &mut msg, 0).is_ok() {
                if msg.tag == tag::INPUT_TEXT {
                    let _ = port_send(tty.0, &msg);
                    continue;
                }
                if msg.tag != tag::INPUT_KEY {
                    continue;
                }
                if KeyEvent::decode(msg.payload()).and_then(|e| key_to_char(&e)) == Some('\u{3}') {
                    println!("^C");
                    for p in procs {
                        let _ = process::kill(p.0);
                    }
                } else {
                    let _ = port_send(tty.0, &msg);
                }
            }
        }
    }

    fn list_jobs(&mut self) {
        for job in &self.jobs {
            let last = job.procs.last().map(|p| p.0);
            match last.map(|h| process::wait(h, 0)) {
                Some(Ok(code)) => println!("[{}] done ({code})  {}", job.id, job.text),
                _ => println!("[{}] running  {}", job.id, job.text),
            }
        }
        // Finished jobs are listed once.
        self.jobs.retain(|j| {
            j.procs
                .last()
                .is_none_or(|p| process::wait(p.0, 0) == Err(Error::TimedOut))
        });
    }

    fn take_job(&mut self, n: &str) -> Result<Job, String> {
        let id: u32 = n
            .trim_start_matches('%')
            .parse()
            .map_err(|_| String::from("no such job"))?;
        let at = self
            .jobs
            .iter()
            .position(|j| j.id == id)
            .ok_or_else(|| String::from("no such job"))?;
        Ok(self.jobs.remove(at))
    }

    fn fg(&mut self, n: &str) -> Result<(), String> {
        let job = self.take_job(n)?;
        println!("{}", job.text);
        let tty = self.program_tty()?;
        let code = self.foreground(&job.procs, &tty, None, None)?;
        println!("[{}] exited with code {code}", job.id);
        Ok(())
    }

    fn kill_job(&mut self, n: &str) -> Result<(), String> {
        let job = self.take_job(n)?;
        for p in &job.procs {
            let _ = process::kill(p.0);
        }
        let code = job
            .procs
            .last()
            .and_then(|p| process::wait(p.0, 1_000_000_000).ok())
            .unwrap_or(-1);
        println!("[{}] killed ({code})", job.id);
        Ok(())
    }

    fn run(&self, program: &str, rest: &[&str]) -> Result<(), String> {
        let (file, folder) = self.find_program(program)?;
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
            process::pass_dir(cwd.0),
            folder
                .as_ref()
                .map_or(Handle::NONE, |f| process::pass_dir(f.0)),
        ];
        // In text mode the shell has INPUT (for `run keyecho`) and passes it.
        let mut caps = process::inherited();
        if let Some(input) = process::pass_capability(libstela::abi::capability::INPUT) {
            caps.push((libstela::abi::capability::INPUT, input));
        }
        let child = Owned(
            process::spawn_with(file.0, &args, &handles, &caps)
                .map_err(|e| String::from(describe(e)))?,
        );
        // Wait, watching the keyboard for Ctrl-C (unless the program took
        // the keyboard for itself); other keys go to the program's tty.
        // A Linux program gets Ctrl-C through its tty (SIGINT, P12); a
        // second one within 2 s, or Ctrl-C for a STELA program, kills.
        let linux = is_linux_program(file.0);
        let mut interrupted: Option<u64> = None;
        let code = loop {
            match process::wait(child.0, 20_000_000) {
                Ok(code) => break code,
                Err(Error::TimedOut) => {}
                Err(e) => return Err(String::from(describe(e))),
            }
            let mut msg = Message::new();
            while port_receive(bootstrap::MAIN_PORT, &mut msg, 0).is_ok() {
                if msg.tag == tag::INPUT_TEXT {
                    let _ = port_send(tty.0, &msg);
                    continue;
                }
                if msg.tag != tag::INPUT_KEY {
                    continue;
                }
                if KeyEvent::decode(msg.payload()).and_then(|e| key_to_char(&e)) == Some('\u{3}') {
                    let now = libstela::now_ns();
                    if linux && interrupted.is_none_or(|t| now - t > 2_000_000_000) {
                        interrupted = Some(now);
                        let _ = port_send(tty.0, &msg);
                    } else {
                        println!("^C");
                        let _ = process::kill(child.0);
                    }
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

/// `name` in one of the installed packages' `bin` folders
/// (`Data:/Programs/*/bin/`, in name order).
fn find_in_programs(
    name: &str,
    runnable: &dyn Fn(Handle) -> bool,
) -> Option<(Owned, Option<Owned>)> {
    let data = Owned(fs::volume_open("Data").ok()?);
    let programs = Owned(fs::open(data.0, "Programs", open::READ).ok()?);
    let mut packages: Vec<String> = fs::entries(programs.0)
        .ok()?
        .iter()
        .filter(|e| e.kind == kind::DIRECTORY)
        .map(|e| String::from(e.name()))
        .collect();
    packages.sort();
    for p in packages {
        let Ok(bin) = fs::open(programs.0, &alloc::format!("{p}/bin"), open::READ) else {
            continue;
        };
        let bin = Owned(bin);
        if let Ok(h) = fs::open(bin.0, name, open::READ) {
            let h = Owned(h);
            if runnable(h.0) {
                return Some((h, Some(bin)));
            }
        }
    }
    None
}

/// Whether the program file has no STELA ELF note (so it runs as a Linux
/// program, `docs/linux.md` §2): the note is looked for in the PT_NOTE
/// segments.
fn is_linux_program(file: Handle) -> bool {
    let mut head = [0u8; 64];
    if libstela::fs::read(file, &mut head, 0) != Ok(64) || &head[..4] != b"\x7fELF" {
        return false;
    }
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u64_at = |b: &[u8], i: usize| {
        let mut w = [0u8; 8];
        w.copy_from_slice(&b[i..i + 8]);
        u64::from_le_bytes(w)
    };
    let phoff = u64_at(&head, 32);
    let phentsize = u16_at(&head, 54) as u64;
    let phnum = u16_at(&head, 56) as u64;
    for i in 0..phnum.min(32) {
        let mut ph = [0u8; 56];
        if libstela::fs::read(file, &mut ph, phoff + i * phentsize) != Ok(56) {
            return false;
        }
        // PT_NOTE.
        if u32::from_le_bytes([ph[0], ph[1], ph[2], ph[3]]) != 4 {
            continue;
        }
        let size = u64_at(&ph, 32).min(4096) as usize;
        let mut notes = vec![0u8; size];
        if libstela::fs::read(file, &mut notes, u64_at(&ph, 8)) != Ok(size) {
            return false;
        }
        let mut at = 0;
        while at + 12 <= notes.len() {
            let word = |i: usize| {
                u32::from_le_bytes([notes[i], notes[i + 1], notes[i + 2], notes[i + 3]]) as usize
            };
            let (namesz, descsz, kind) = (word(at), word(at + 4), word(at + 8));
            let name = notes.get(at + 12..at + 12 + namesz).unwrap_or(&[]);
            if name == b"STELA\0" && kind == 1 {
                return false;
            }
            at += 12 + namesz.next_multiple_of(4) + descsz.next_multiple_of(4);
        }
    }
    true
}

/// The built-in commands (for Tab).
const BUILTINS: [&str; 20] = [
    "cat", "cd", "cp", "exit", "fg", "help", "jobs", "kill", "ls", "mem", "mkdir", "mv", "ps",
    "pwd", "reboot", "rm", "run", "shutdown", "uptime", "vols",
];

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
    println!("  cp from to       copy a file (cp -r: a folder and what is in it)");
    println!("  mv from to       move or rename");
    println!("  mkdir path       make a folder");
    println!("  rm path          remove a file or an empty folder");
    println!("  cd [path]        change the current folder (no path: the volume's root)");
    println!("  pwd              show the current folder");
    println!(
        "  run prog [args]  run a program or Name.app (current folder, then the search path); Ctrl-C stops it"
    );
    println!("  ps, mem, uptime  processes, memory, time since boot");
    println!("  a | b, a > f, a >> f, a < f, a &   pipes, files, background (programs only)");
    println!("  jobs, fg N, kill N                 background jobs");
    println!("  vols             volumes");
    println!("  ifconfig         the network card and address");
    println!("  ping host [n]    send ICMP echo requests");
    println!("  fetch url [file] get http://host[:port]/path (print it or save it)");
    println!("  tone [Hz [ms]]   play a square wave (default 440 Hz, 1000 ms)");
    println!("  hwinfo           what hardware STELA found");
    println!(
        "  other names      programs here, in Initrd:/, Initrd:/bin/, System:/Programs/, System:/bin/: echo cat grep"
    );
    println!("                   wc head tail sort uniq find du df date sleep touch stat");
    println!("                   hexdump tar true false (docs/apps.md)");
    println!("  shutdown, reboot turn the computer off, restart it");
    println!("  exit             leave the shell (init starts a new one)");
    println!("paths: Volume:/a/b, /a/b (this volume) or a/b, with . and ..");
    println!("Tab completes a name; * and ? in an argument match names (*.txt)");
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

/// Where the history is kept (P11).
const HISTORY_FILE: &str = ".shell_history";

/// The lines of `Data:/.shell_history` into the editor's history.
fn load_history(editor: &mut LineEditor) {
    let Ok(data) = fs::volume_open("Data").map(Owned) else {
        return;
    };
    let Ok(file) = fs::open(data.0, HISTORY_FILE, open::READ).map(Owned) else {
        return;
    };
    let size = fs::stat(file.0).map(|s| s.size).unwrap_or(0).min(64 * 1024) as usize;
    let mut buf = vec![0u8; size];
    let n = fs::read(file.0, &mut buf, 0).unwrap_or(0);
    for line in String::from_utf8_lossy(&buf[..n]).lines() {
        editor.remember(line);
    }
}

/// Writes the history back (the whole file: at most HISTORY lines).
fn save_history(editor: &LineEditor) {
    let Ok(data) = fs::volume_open("Data").map(Owned) else {
        return;
    };
    let flags = open::READ | open::WRITE | open::CREATE | open::TRUNCATE;
    let Ok(file) = fs::open(data.0, HISTORY_FILE, flags).map(Owned) else {
        return;
    };
    let mut text = String::new();
    for line in editor.history() {
        text.push_str(line);
        text.push('\n');
    }
    let _ = fs::write(file.0, text.as_bytes(), 0);
}

/// The volume the working directory handle `[3]` is on, by its index.
/// Where programs are looked for after the current folder (P14 K4):
/// (volume, folder); `Data:/Programs/NAME/bin/` comes last.
const SEARCH_PATH: [(&str, &str); 4] = [
    ("Initrd", ""),
    ("Initrd", "bin"),
    ("System", "Programs"),
    ("System", "bin"),
];

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
        jobs: Vec::new(),
        next_job: 1,
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
    load_history(&mut editor);
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
        if msg.tag == tag::INPUT_TEXT {
            // P13 K3: text from the input method, typed at the prompt.
            let text = String::from_utf8_lossy(msg.payload()).into_owned();
            for c in text.chars().filter(|c| !c.is_control()) {
                if let Edit::Show(s) = editor.feed_key(Key::Char(c)) {
                    print!("{s}");
                }
            }
            continue;
        }
        if msg.tag != tag::INPUT_KEY {
            for &h in msg.carried() {
                let _ = handle_close(h);
            }
            continue;
        }
        let Some(event) = KeyEvent::decode(msg.payload()) else {
            continue;
        };
        if !event.pressed {
            continue;
        }
        use libstela::abi::key::KeyCode;
        let key = match event.code {
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::Delete => Key::Delete,
            _ => match key_to_char(&event) {
                Some(c) => Key::Char(c),
                None => continue,
            },
        };
        match editor.feed_key(key) {
            Edit::None => {}
            Edit::Show(s) => print!("{s}"),
            Edit::Complete => shell.complete(&mut editor),
            Edit::Done(line) => {
                println!();
                save_history(&editor);
                if !shell.run_line(&line) {
                    println!("bye");
                    return 0;
                }
                shell.prompt();
            }
        }
    }
}

/// Copies folder `src` into `dst_parent` as `name` (made if needed);
/// returns the files and bytes copied.
fn copy_tree(
    src: Handle,
    dst_parent: Handle,
    name: &str,
    depth: usize,
) -> Result<(u64, u64), String> {
    if depth > 64 {
        return Err(String::from("folders nested too deep"));
    }
    let dst = fs::open(
        dst_parent,
        name,
        open::READ | open::WRITE | open::CREATE | open::DIRECTORY,
    )
    .map(Owned)
    .map_err(|e| alloc::format!("{name}: {}", describe(e)))?;
    let (mut files, mut bytes) = (0, 0);
    for e in fs::entries(src).map_err(|e| String::from(describe(e)))? {
        let n = e.name();
        let item = Owned(
            fs::open(src, n, open::READ).map_err(|e| alloc::format!("{n}: {}", describe(e)))?,
        );
        if e.kind == kind::DIRECTORY {
            let (f, b) = copy_tree(item.0, dst.0, n, depth + 1)?;
            files += f;
            bytes += b;
            continue;
        }
        let out = Owned(
            fs::open(
                dst.0,
                n,
                open::READ | open::WRITE | open::CREATE | open::TRUNCATE,
            )
            .map_err(|e| alloc::format!("{n}: {}", describe(e)))?,
        );
        let mut buf = vec![0u8; 64 * 1024];
        let mut at = 0u64;
        loop {
            let got = fs::read(item.0, &mut buf, at)
                .map_err(|e| alloc::format!("{n}: {}", describe(e)))?;
            if got == 0 {
                break;
            }
            let mut done = 0;
            while done < got {
                match fs::write(out.0, &buf[done..got], at + done as u64) {
                    Ok(0) | Err(_) => return Err(alloc::format!("{n}: cannot write")),
                    Ok(w) => done += w,
                }
            }
            at += got as u64;
        }
        files += 1;
        bytes += at;
    }
    Ok((files, bytes))
}
