// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The fuzz targets and their seeds (`docs/testing.md` §3).

use crate::{BLOCK, Overlay, Rng, Target, mutate, mutate_in_place, note};
use block::BlockDevice;
use block::mem::MemDisk;
use kernel::fs::fat::{Dir, FatVolume};
use net::stack::{Config, Kind, Stack};
use net::{dhcp, wire};
use std::path::Path;

/// Every target, in the order they run.
pub fn all(build: &Path) -> Vec<Target> {
    // The toolbox would load its font through STELA's file system calls:
    // give it the file from the repository instead.
    let font = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../third_party/fusion-pixel/fusion-pixel-12-zh_tw.bin"
    ))
    .ok()
    .and_then(toolbox::cjk::CjkFont::parse);
    toolbox::cjk::set_font(font);
    let images = build.join("images");
    vec![
        ehfs_target(),
        disk_target("fat", &images.join("fat-test.img"), 20_000, fat_run, false),
        disk_target("gpt", &images.join("gpt-test.img"), 30_000, gpt_run, true),
        bytes_target("mbr", mbr_seeds(), 100_000, 512, |d| {
            let _ = block::mbr::read(d);
        }),
        bytes_target("ustar", vec![ustar_seed()], 50_000, 1 << 16, ustar_run),
        net_target(),
        bytes_target("http", http_seeds(), 100_000, 4096, http_run),
        bytes_target("dns", dns_seeds(), 100_000, 512, dns_run),
        bytes_target(
            "html",
            vec![HTML.as_bytes().to_vec()],
            20_000,
            8192,
            html_run,
        ),
        bytes_target("config", config_seeds(), 100_000, 2048, config_run),
        bytes_target("path", path_seeds(), 100_000, 512, path_run),
    ]
}

/// A target over byte strings: a seed changed by [`mutate`].
fn bytes_target(
    name: &'static str,
    seeds: Vec<Vec<u8>>,
    inputs: u64,
    max: usize,
    run: fn(&[u8]),
) -> Target {
    Target {
        name,
        inputs,
        seconds: 2,
        run: Box::new(move |rng| {
            let mut d = seeds[rng.below(seeds.len())].clone();
            mutate(rng, &mut d, max);
            note(&d);
            run(&d);
        }),
    }
}

/// A target over a disk image (missing: no target runs; `make
/// host-images` makes them). `fix` repairs the GPT checksums half the
/// time, so the changes get past them.
fn disk_target(
    name: &'static str,
    path: &Path,
    inputs: u64,
    run: fn(&Overlay),
    fix: bool,
) -> Target {
    let image = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("fuzz: {}: {e} (make host-images)", path.display());
        std::process::exit(1);
    });
    // The blocks the seed's run reads: the ones worth changing.
    let hot = {
        let o = Overlay::new(&image);
        run(&o);
        o.reads()
    };
    Target {
        name,
        inputs,
        seconds: if name == "fat" { 15 } else { 5 },
        run: Box::new(move |rng| {
            let o = Overlay::new(&image);
            o.mutate(rng, &hot);
            if fix && rng.one_in(2) {
                gpt_fix(&o);
            }
            note(&o.changes());
            run(&o);
        }),
    }
}

// ---- eHFS ----

const NOW: u64 = 1_760_000_000_000_000_000;

/// A small volume: folders three deep, files from empty to three extents,
/// a resource fork, attributes; enough names for a few catalog nodes.
fn ehfs_seed() -> Vec<u8> {
    let disk = MemDisk::new(2048);
    let options = ehfs::FormatOptions {
        name: "Data",
        uuid: [7; 16],
        now: NOW,
        journal_blocks: 64,
    };
    ehfs::format(&disk, &options).expect("format");
    let mut v = ehfs::Volume::mount(&disk, false).expect("mount");
    let docs = v.create(ehfs::ROOT, b"docs", true, NOW).expect("docs");
    let deep = v.create(docs.cnid, b"deep", true, NOW).expect("deep");
    let deeper = v
        .create(deep.cnid, "中文".as_bytes(), true, NOW)
        .expect("deeper");
    for (i, size) in [0usize, 10, 5000, 70_000, 300_000].iter().enumerate() {
        let parent = [ehfs::ROOT, docs.cnid, deep.cnid, deeper.cnid, docs.cnid][i];
        let f = v
            .create(parent, format!("file{i}.txt").as_bytes(), false, NOW)
            .expect("file");
        let data: Vec<u8> = (0..*size).map(|n| (n * 7 % 251) as u8).collect();
        v.write(f.cnid, false, 0, &data, NOW).expect("write");
        if i == 2 {
            v.write(f.cnid, true, 0, b"resource", NOW).expect("rsrc");
            v.attr_set(f.cnid, b"stela.type", b"TEXT").expect("attr");
        }
    }
    for i in 0..60 {
        v.create(docs.cnid, format!("name {i:03}").as_bytes(), false, NOW)
            .expect("name");
    }
    v.unmount().expect("unmount");
    disk.to_bytes()
}

fn ehfs_walk<D: BlockDevice>(v: &mut ehfs::Volume<D>, dir: u64, depth: usize, left: &mut usize) {
    let Ok(list) = v.list(dir) else {
        return;
    };
    let mut buf = vec![0u8; 16 * 1024];
    for e in list {
        if *left == 0 {
            return;
        }
        *left -= 1;
        if e.is_dir {
            let _ = v.lookup(dir, &e.name);
            if depth < 8 {
                ehfs_walk(v, e.cnid, depth + 1, left);
            }
            continue;
        }
        // Every catalog lookup reads 16 KiB nodes and checks their CRC:
        // only the forks and attributes that are there.
        for resource in [false, true] {
            let empty = if resource {
                e.rsrc_fork == [0; ehfs::format::FORK_SIZE]
            } else {
                e.size() == 0
            };
            let mut at = 0u64;
            for _ in 0..if empty { 0 } else { 2 } {
                match v.read(e.cnid, resource, at, &mut buf) {
                    Ok(n) if n > 0 => at += n as u64,
                    _ => break,
                }
            }
        }
        // Attributes: on the files with a resource fork (the seed's
        // file with attributes has one), as each look costs a tree walk.
        if e.rsrc_fork != [0; ehfs::format::FORK_SIZE]
            && let Ok(names) = v.attr_names(e.cnid)
        {
            for n in names {
                let _ = v.attr_get(e.cnid, &n);
            }
        }
    }
}

fn ehfs_run(o: &Overlay) {
    // Read-only, as a damaged volume is first looked at; and once in a
    // while for writing (the journal is replayed then).
    let Ok(mut v) = ehfs::Volume::mount(o, true) else {
        return;
    };
    let _ = (v.name(), v.counts(), v.free_blocks());
    let mut left = 500;
    ehfs_walk(&mut v, ehfs::ROOT, 0, &mut left);
}

/// The first blocks of the B-tree nodes in `image` (a node is
/// `NODE_BLOCKS` blocks whose first 4 bytes are the CRC-32C of the rest).
fn ehfs_nodes(image: &[u8]) -> Vec<u64> {
    let size = ehfs::format::NODE_SIZE;
    (0..image.len() / BLOCK)
        .filter(|&b| {
            let n = &image[b * BLOCK..];
            n.len() >= size
                && n[..4] != [0; 4]
                && u32::from_le_bytes([n[0], n[1], n[2], n[3]])
                    == ehfs::crc32c::crc32c(0, &n[4..size])
        })
        .map(|b| b as u64)
        .collect()
}

/// Seals again what the changes broke: the nodes with a changed block
/// and the superblock (`docs/ehfs.md`), so the changes reach the parsers.
fn ehfs_reseal(o: &Overlay, nodes: &[u64]) {
    use ehfs::format::{NODE_BLOCKS, NODE_SIZE, SB_OFFSET, sb};
    let changed = o.changed_blocks();
    for &start in nodes {
        if !changed
            .iter()
            .any(|&b| (start..start + NODE_BLOCKS).contains(&b))
        {
            continue;
        }
        let mut n = vec![0u8; NODE_SIZE];
        if o.read(start, &mut n).is_ok() {
            let crc = ehfs::crc32c::crc32c(0, &n[4..]);
            o.with_block(start, |b| b[..4].copy_from_slice(&crc.to_le_bytes()));
        }
    }
    if changed.contains(&0) {
        o.with_block(0, |b| {
            let s = &mut b[SB_OFFSET..];
            let crc = ehfs::crc32c::crc32c(0, &s[..sb::CRC]);
            s[sb::CRC..sb::CRC + 4].copy_from_slice(&crc.to_le_bytes());
        });
    }
}

fn ehfs_target() -> Target {
    let image = ehfs_seed();
    let nodes = ehfs_nodes(&image);
    let hot = {
        let o = Overlay::new(&image);
        ehfs_run(&o);
        o.reads()
    };
    Target {
        name: "ehfs",
        inputs: 20_000,
        seconds: 25,
        run: Box::new(move |rng| {
            let o = Overlay::new(&image);
            o.mutate(rng, &hot);
            if rng.one_in(4) {
                ehfs_reseal(&o, &nodes);
            }
            note(&o.changes());
            ehfs_run(&o);
            if rng.one_in(16)
                && let Ok(mut v) = ehfs::Volume::mount(&o, false)
            {
                let mut left = 100;
                ehfs_walk(&mut v, ehfs::ROOT, 0, &mut left);
                let _ = v.create(ehfs::ROOT, b"new", false, NOW);
                let _ = v.unmount();
            }
        }),
    }
}

// ---- FAT ----

fn fat_walk<D: BlockDevice>(f: &FatVolume<D>, dir: Dir, depth: usize, left: &mut usize) {
    let Ok(list) = f.read_dir(dir) else {
        return;
    };
    let mut buf = vec![0u8; 16 * 1024];
    for e in list {
        if *left == 0 {
            return;
        }
        *left -= 1;
        let _ = f.lookup(dir, &e.name);
        if e.is_dir {
            if depth < 8 && e.name != "." && e.name != ".." {
                fat_walk(f, f.dir_of(&e), depth + 1, left);
            }
            continue;
        }
        let mut at = 0u64;
        for _ in 0..4 {
            match f.read_file(&e, at, &mut buf) {
                Ok(n) if n > 0 => at += n as u64,
                _ => break,
            }
        }
    }
}

fn fat_run(o: &Overlay) {
    let Ok(f) = FatVolume::mount(o) else {
        return;
    };
    let _ = (f.kind(), f.label(), f.size(), f.cluster_bytes());
    let mut left = 500;
    fat_walk(&f, f.root(), 0, &mut left);
    let _ = f.resolve("EFI/BOOT/BOOTX64.EFI");
}

// ---- GPT and MBR ----

fn gpt_run(o: &Overlay) {
    if let Ok(g) = block::gpt::read(o) {
        for p in &g.partitions {
            let _ = g.blocks_of(p);
        }
    }
}

/// Recomputes the primary header's two CRCs (512-byte sectors: the
/// header in LBA 1, UEFI Specification 2.10 §5.3.2).
fn gpt_fix(o: &Overlay) {
    let mut first = vec![0u8; 8 * BLOCK];
    if o.read(0, &mut first).is_err() {
        return;
    }
    let h = &first[512..1024];
    let le32 = |at: usize| u32::from_le_bytes([h[at], h[at + 1], h[at + 2], h[at + 3]]);
    let size = (le32(12) as usize).clamp(92, 512);
    let lba = u64::from_le_bytes(h[72..80].try_into().unwrap_or([0; 8]));
    let bytes = le32(80) as usize * le32(84) as usize;
    let array_crc = lba
        .checked_mul(512)
        .map(|s| s as usize)
        .and_then(|s| first.get(s..s.checked_add(bytes)?))
        .map(block::crc32::crc32);
    o.with_block(0, |b| {
        let h = &mut b[512..1024];
        if let Some(c) = array_crc {
            h[88..92].copy_from_slice(&c.to_le_bytes());
        }
        h[16..20].fill(0);
        let c = block::crc32::crc32(&h[..size]);
        h[16..20].copy_from_slice(&c.to_le_bytes());
    });
}

fn mbr_seeds() -> Vec<Vec<u8>> {
    let mut m = vec![0u8; 512];
    // Two partitions: FAT32 LBA at 2048, Linux after it.
    for (i, (kind, start, count)) in [(0x0Cu8, 2048u32, 65536u32), (0x83, 67584, 4096)]
        .iter()
        .enumerate()
    {
        let e = &mut m[446 + 16 * i..462 + 16 * i];
        e[4] = *kind;
        e[8..12].copy_from_slice(&start.to_le_bytes());
        e[12..16].copy_from_slice(&count.to_le_bytes());
    }
    m[510] = 0x55;
    m[511] = 0xAA;
    let mut protective = m.clone();
    protective[446 + 4] = 0xEE;
    vec![m, protective]
}

// ---- ustar ----

fn tar_member(out: &mut Vec<u8>, path: &str, dir: bool, data: &[u8]) {
    let mut h = [0u8; 512];
    let (prefix, name) = if path.len() > 100 {
        path.split_at(path.len() - 50)
    } else {
        ("", path)
    };
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[345..345 + prefix.len().min(155)]
        .copy_from_slice(&prefix.as_bytes()[..prefix.len().min(155)]);
    h[100..108].copy_from_slice(b"0000644\0");
    h[108..116].copy_from_slice(b"0000000\0");
    h[116..124].copy_from_slice(b"0000000\0");
    h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
    h[136..148].copy_from_slice(b"00000000000\0");
    h[148..156].fill(b' ');
    h[156] = if dir { b'5' } else { b'0' };
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    out.extend_from_slice(&h);
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(512), 0);
}

fn ustar_seed() -> Vec<u8> {
    let mut t = Vec::new();
    tar_member(&mut t, "bin/", true, b"");
    tar_member(&mut t, "bin/echo", false, b"\x7fELF echo");
    tar_member(&mut t, "Terminal.app/", true, b"");
    tar_member(&mut t, "Terminal.app/terminal", false, &[0x55; 700]);
    let long = format!("{}/file.txt", "deep/".repeat(24));
    tar_member(&mut t, &long, false, b"long name");
    tar_member(&mut t, "welcome.txt", false, b"Welcome to STELA.\n");
    t.extend_from_slice(&[0; 1024]);
    t
}

fn tree_walk(t: &kernel::initrd::Tree, dir: u64, depth: usize) {
    if depth > 32 {
        return;
    }
    let children: Vec<(u64, String, bool)> = t
        .children(dir)
        .map(|n| (n.id, n.name.clone(), n.is_dir))
        .collect();
    for (id, name, is_dir) in children {
        let _ = t.lookup(dir, &name);
        let _ = t.get(id);
        if is_dir && id != dir {
            tree_walk(t, id, depth + 1);
        }
    }
}

fn ustar_run(d: &[u8]) {
    let _ = kernel::initrd::entries(d).count();
    let _ = kernel::initrd::find(d, "bin/echo");
    let t = kernel::initrd::Tree::new(d);
    tree_walk(&t, kernel::initrd::TREE_ROOT, 0);
}

// ---- Network ----

const A_MAC: wire::Mac = [0x52, 0x54, 0, 0, 0, 0xA];
const B_MAC: wire::Mac = [0x52, 0x54, 0, 0, 0, 0xB];
const A_IP: wire::Ip = [10, 0, 2, 15];
const B_IP: wire::Ip = [10, 0, 2, 2];

fn configured(mac: wire::Mac, ip: wire::Ip, seed: u64) -> Stack {
    let mut s = Stack::new(mac, seed);
    s.configure(Config {
        ip,
        mask: [255, 255, 255, 0],
        gateway: B_IP,
        dns: [10, 0, 2, 3],
    });
    s
}

/// The checksums of an IPv4 frame recomputed (header, then TCP, UDP or
/// ICMP), so a changed frame gets past them.
fn fix_checksums(f: &mut [u8]) {
    if f.len() < wire::ETH_HEADER + wire::IP_HEADER || f[12..14] != [8, 0] {
        return;
    }
    let ip = &mut f[wire::ETH_HEADER..];
    let ihl = (usize::from(ip[0] & 0xF) * 4).clamp(20, ip.len().min(60));
    let total = usize::from(u16::from_be_bytes([ip[2], ip[3]])).clamp(ihl, ip.len());
    ip[10..12].fill(0);
    let sum = wire::checksum(&ip[..ihl], 0);
    ip[10..12].copy_from_slice(&sum.to_be_bytes());
    let src = [ip[12], ip[13], ip[14], ip[15]];
    let dst = [ip[16], ip[17], ip[18], ip[19]];
    let proto = ip[9];
    let p = &mut ip[ihl..total];
    let at = match proto {
        wire::PROTO_TCP if p.len() >= 20 => 16,
        wire::PROTO_UDP if p.len() >= 8 => 6,
        wire::PROTO_ICMP if p.len() >= 4 => 2,
        _ => return,
    };
    p[at..at + 2].fill(0);
    let initial = if proto == wire::PROTO_ICMP {
        0
    } else {
        wire::pseudo_sum(src, dst, proto, p.len())
    };
    let sum = wire::checksum(p, initial);
    p[at..at + 2].copy_from_slice(&sum.to_be_bytes());
}

/// A DHCP offer or ack for the request in `frame` (as a server would).
fn dhcp_reply(frame: &[u8], kind: u8) -> Option<Vec<u8>> {
    let eth = wire::parse_eth(frame)?;
    let ip = wire::parse_ipv4(eth.payload)?;
    let udp = wire::parse_udp(ip.src, ip.dst, ip.payload)?;
    let req = udp.data;
    let mut p = vec![2, 1, 6, 0];
    p.extend_from_slice(req.get(4..8)?);
    p.extend_from_slice(&[0; 8]);
    p.extend_from_slice(&A_IP);
    p.extend_from_slice(&[0; 8]);
    p.extend_from_slice(req.get(28..44)?);
    p.extend_from_slice(&[0; 192]);
    p.extend_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    p.extend_from_slice(&[53, 1, kind, 54, 4, 10, 0, 2, 2, 1, 4, 255, 255, 255, 0]);
    p.extend_from_slice(&[3, 4, 10, 0, 2, 2, 6, 4, 10, 0, 2, 3, 51, 4, 0, 0, 1, 0, 255]);
    let udp = wire::build_udp(
        B_IP,
        wire::BROADCAST_IP,
        dhcp::SERVER_PORT,
        dhcp::CLIENT_PORT,
        &p,
    );
    let ip = wire::build_ipv4(B_IP, wire::BROADCAST_IP, wire::PROTO_UDP, 1, &udp);
    Some(wire::build_eth(
        wire::BROADCAST_MAC,
        B_MAC,
        wire::ETHERTYPE_IPV4,
        &ip,
    ))
}

/// The frames of a clean conversation: ARP, ping, UDP, a TCP connection
/// with data both ways and its close, and DHCP.
fn net_seeds() -> Vec<Vec<u8>> {
    let mut seen = Vec::new();
    let mut a = configured(A_MAC, A_IP, 1);
    let mut b = configured(B_MAC, B_IP, 2);
    let (conn, ping, udp, listener) = setup(&mut a, &mut b);
    let mut accepted = None;
    let mut buf = [0u8; 2048];
    for round in 0..40u64 {
        let now = round * 300;
        let fa = a.take_output();
        let fb = b.take_output();
        for f in fa {
            b.receive(&f, now);
            seen.push(f);
        }
        for f in fb {
            a.receive(&f, now);
            seen.push(f);
        }
        talk(
            &mut a,
            &mut b,
            (conn, ping, udp, listener),
            &mut accepted,
            round,
            now,
            &mut buf,
        );
        a.poll(now);
        b.poll(now);
    }
    let mut c = Stack::new([0x52, 0x54, 0, 0, 0, 0xC], 3);
    c.start_dhcp(0);
    for f in c.take_output() {
        seen.extend(dhcp_reply(&f, 2));
        seen.extend(dhcp_reply(&f, 5));
        seen.push(f);
    }
    seen
}

type Sockets = (
    net::stack::SocketId,
    net::stack::SocketId,
    net::stack::SocketId,
    net::stack::SocketId,
);

/// B listens on TCP 80 and UDP 53; A connects, pings and sends a datagram.
fn setup(a: &mut Stack, b: &mut Stack) -> Sockets {
    let listener = b.socket(Kind::Tcp).expect("socket");
    let _ = b.bind(listener, 80);
    let _ = b.listen(listener, 4);
    let udp = b.socket(Kind::Udp).expect("socket");
    let _ = b.bind(udp, 53);
    let conn = a.socket(Kind::Tcp).expect("socket");
    let _ = a.connect(conn, B_IP, 80, 0);
    let ping = a.socket(Kind::Icmp).expect("socket");
    let _ = a.send(ping, b"stela", Some((B_IP, 1)), 0);
    let q = a.socket(Kind::Udp).expect("socket");
    let _ = a.send(q, b"query", Some((B_IP, 53)), 0);
    (conn, ping, udp, listener)
}

/// One round of the applications: accept, send and read, close late.
#[allow(clippy::too_many_arguments)]
fn talk(
    a: &mut Stack,
    b: &mut Stack,
    (conn, ping, udp, listener): Sockets,
    accepted: &mut Option<net::stack::SocketId>,
    round: u64,
    now: u64,
    buf: &mut [u8],
) {
    if accepted.is_none() {
        *accepted = b.accept(listener).ok().flatten();
    }
    if let Some(s) = *accepted {
        let _ = b.send(s, &[round as u8; 700], None, now);
        let _ = b.recv(s, buf);
        if round == 30 {
            b.close(s, now);
        }
    }
    let _ = a.send(conn, b"hello, stela", None, now);
    let _ = a.recv(conn, buf);
    let _ = a.recv(ping, buf);
    let _ = b.recv(udp, buf);
    if round == 25 {
        a.close(conn, now);
    }
}

fn parse_all(f: &[u8]) {
    let Some(e) = wire::parse_eth(f) else {
        return;
    };
    let _ = wire::parse_arp(e.payload);
    if let Some(ip) = wire::parse_ipv4(e.payload) {
        let _ = wire::parse_echo(ip.payload);
        let _ = wire::parse_tcp(ip.src, ip.dst, ip.payload);
        if let Some(u) = wire::parse_udp(ip.src, ip.dst, ip.payload) {
            let _ = dhcp::parse(u.data, A_MAC);
        }
    }
}

fn net_target() -> Target {
    let seeds = net_seeds();
    Target {
        name: "net",
        inputs: 100_000,
        seconds: 4,
        run: Box::new(move |rng| {
            if rng.one_in(16) {
                note(&rng.0.to_le_bytes());
                conversation(rng);
                return;
            }
            let mut f = seeds[rng.below(seeds.len())].clone();
            mutate(rng, &mut f, 1600);
            if rng.one_in(2) {
                fix_checksums(&mut f);
            }
            note(&f);
            parse_all(&f);
            let mut s = configured(A_MAC, A_IP, 1);
            let l = s.socket(Kind::Tcp).expect("socket");
            let _ = s.bind(l, 80);
            let _ = s.listen(l, 4);
            s.receive(&f, 0);
            s.poll(5000);
            let _ = s.take_output();
            let mut d = Stack::new(A_MAC, 1);
            d.start_dhcp(0);
            d.receive(&f, 10);
            d.poll(3000);
        }),
    }
}

/// Two stacks talking while the frames between them are changed,
/// dropped and repeated.
fn conversation(rng: &mut Rng) {
    let mut a = configured(A_MAC, A_IP, rng.next_u64());
    let mut b = configured(B_MAC, B_IP, rng.next_u64());
    let sockets = setup(&mut a, &mut b);
    let mut accepted = None;
    let mut buf = [0u8; 2048];
    let mut now = 0;
    for round in 0..40 {
        let fa = a.take_output();
        let fb = b.take_output();
        for (to, frames) in [(&mut b, fa), (&mut a, fb)] {
            for mut f in frames {
                match rng.below(16) {
                    0 => continue,
                    1 => to.receive(&f, now),
                    2..=5 => {
                        if rng.one_in(2) {
                            mutate(rng, &mut f, 1600);
                        } else {
                            mutate_in_place(rng, &mut f);
                        }
                        if rng.one_in(4) {
                            continue;
                        }
                        fix_checksums(&mut f);
                    }
                    _ => {}
                }
                to.receive(&f, now);
            }
        }
        talk(&mut a, &mut b, sockets, &mut accepted, round, now, &mut buf);
        now += 50 + rng.below(1500) as u64;
        a.poll(now);
        b.poll(now);
    }
}

// ---- HTTP and DNS ----

fn http_seeds() -> Vec<Vec<u8>> {
    [
        &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 5\r\n\r\nhello"[..],
        b"HTTP/1.1 302 Found\r\nLocation: ../next/./a.html?b=1#c\r\n\r\n",
        b"HTTP/1.0 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n7;x=1\r\n, world\r\n0\r\n\r\n",
        b"http://10.0.2.2:8642/docs/index.html",
    ]
    .iter()
    .map(|s| s.to_vec())
    .collect()
}

fn http_run(d: &[u8]) {
    use libstela::http;
    if let Some((status, headers, at)) = http::parse_head(d) {
        let r = http::Response {
            status,
            headers,
            body: d.get(at..).unwrap_or(&[]).to_vec(),
            url: String::new(),
        };
        let _ = r.is_html();
        if let Some(to) = r.header("location") {
            let _ = http::join("http://10.0.2.2:8642/docs/index.html", to);
        }
        let _ = http::dechunk(&r.body);
    }
    let _ = http::dechunk(d);
    let _ = http::looks_like_html(d);
    let _ = libstela::net::parse_response(d);
    let text = String::from_utf8_lossy(d);
    let _ = libstela::net::parse_url(&text);
    let mut mid = text.len() / 2;
    while !text.is_char_boundary(mid) {
        mid += 1;
    }
    let (a, b) = text.split_at(mid);
    let _ = http::join(a, b);
    let _ = http::join("Data:/web/a.html", &text);
    let _ = http::join(&text, "x/../y.html");
}

fn dns_seeds() -> Vec<Vec<u8>> {
    let mut r = libstela::net::dns_query(7, "stela.test").expect("query");
    let query = r.clone();
    r[2] = 0x81;
    r[3] = 0x80;
    r[7] = 2;
    r.extend_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 2, 0xC0, 12]);
    r.extend_from_slice(&[0xC0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 93, 184, 216, 34]);
    vec![r, query, b"www.example.com".to_vec()]
}

fn dns_run(d: &[u8]) {
    let _ = libstela::net::dns_answer(d, 7);
    let _ = libstela::net::dns_query(7, &String::from_utf8_lossy(d));
}

// ---- HTML ----

const HTML: &str = "<!DOCTYPE html><html><head><title>Test &amp; page</title>\
<style>p { color: red }</style><script>var x = '<p>';</script></head><body>\
<h1>Heading</h1><p>A <b>bold</b> and <i>italic</i> <a href=\"page2.html\">link</a>, \
&lt;tags&gt; &#x4E2D;&#25991; &nbsp;&copy;&unknown; 中文字</p>\
<ul><li>one</li><li>two<ol><li>nested</li></ol></li></ul><pre>  pre\n\ttext\n</pre>\
<table><tr><td>cell</td><td>cell 2</td></tr></table><hr><br>\
<img src=\"x.png\" alt=\"an image\"><!-- a comment --><blockquote>quote</blockquote>\
<form><input type=text></form></body></html>";

fn html_run(d: &[u8]) {
    let s = String::from_utf8_lossy(d);
    let page = periplus::parse(&s);
    let _ = periplus::layout(&page, 80);
    let _ = periplus::layout(&page, 9 + d.len() % 40);
    let _ = periplus::decode(&s);
    let _ = periplus::tokens(&s);
    let _ = periplus::layout(&periplus::plain(&s), 40);
}

// ---- Settings ----

fn config_seeds() -> Vec<Vec<u8>> {
    [
        &b"# boot options\ntextmode\nnousb   # no USB\n  verbose\nwaitkey\nclassic=off\n"[..],
        b"ram=64\nresolution=1024x768\ndepth=8\nscale=auto\nfs=on\ndisk2=Data:/Classic/disk2.img,ro\n# comment\n",
        b"+08:00",
        b"-05:30",
    ]
    .iter()
    .map(|s| s.to_vec())
    .collect()
}

fn config_run(d: &[u8]) {
    let mut out = [0u8; 256];
    let n = bootinfo::parse_config(d, &mut out);
    assert!(n <= out.len());
    let s = String::from_utf8_lossy(d);
    let _ = classic::config::Config::parse(&s);
    if let Some(m) = abi::time::parse_offset(&s) {
        let _ = abi::time::offset_text(m);
        let mut tz = [0u8; 12];
        let _ = abi::time::posix_tz(m, &mut tz);
    }
    let _ = abi::key::KeyEvent::decode(d);
    let _ = abi::key::MouseEvent::decode(d);
}

// ---- Paths and the command line ----

fn path_seeds() -> Vec<Vec<u8>> {
    [
        &b"Data:/a/b/../c/./d"[..],
        b"/x/./y/../../..",
        b"rel/../../z*.t?t",
        b"Initrd:/bin/echo",
        b"cat < in.txt | grep -i stela | wc -l >> out.txt &",
        b"ls Data:/docs/no\t",
    ]
    .iter()
    .map(|s| s.to_vec())
    .collect()
}

fn path_run(d: &[u8]) {
    use libstela::line::{Key, LineEditor};
    use libstela::path;
    let s = String::from_utf8_lossy(d);
    let cwd = path::Location::root("Data");
    if let Ok(loc) = path::resolve(&cwd, &s) {
        let _ = (loc.relative(), loc.name(), loc.parent());
    }
    let (folder, last) = path::split_last(&s);
    let _ = path::glob(last, folder);
    let _ = path::glob(&s, "welcome.txt");
    let _ = path::common_prefix(&[s.to_string(), folder.to_string(), last.to_string()]);
    let _ = libstela::pipeline::is_pipeline(&s);
    let _ = libstela::pipeline::parse(&s);
    let mut e = LineEditor::new(64);
    for (i, c) in s.chars().enumerate() {
        let key = match (c as u32 + i as u32) % 23 {
            0 => Key::Left,
            1 => Key::Right,
            2 => Key::Up,
            3 => Key::Down,
            4 => Key::Home,
            5 => Key::End,
            6 => Key::Delete,
            _ => Key::Char(c),
        };
        let _ = e.feed_key(key);
        if i % 7 == 0 {
            let _ = e.word();
            let _ = e.insert("ab/");
            let _ = e.redraw();
        }
        if i % 11 == 0 {
            e.remember(&s);
        }
    }
}
