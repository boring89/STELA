// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host tests on in-memory disks. Every test ends with fsck.

use super::*;
use block::BlockDevice;
use block::mem::MemDisk;
use std::collections::BTreeMap;

const NOW: u64 = 1_760_000_000_000_000_000;

fn options() -> FormatOptions<'static> {
    FormatOptions {
        name: "Data",
        uuid: [7; 16],
        now: NOW,
        journal_blocks: 0,
    }
}

fn fresh(blocks: u64) -> MemDisk {
    let disk = MemDisk::new(blocks);
    format(&disk, &options()).unwrap();
    disk
}

fn fsck<D: BlockDevice>(v: &mut Volume<D>) {
    let r = v.check();
    assert!(
        r.clean(),
        "fsck: {} problem(s), first: {}",
        r.errors,
        r.first
    );
}

#[test]
fn format_and_mount() {
    let disk = fresh(4096);
    let mut v = Volume::mount(&disk, true).unwrap();
    assert_eq!(v.name(), "Data");
    // P14 K4: the name read without mounting.
    let mut block0 = vec![0u8; 4096];
    disk.read(0, &mut block0).unwrap();
    assert_eq!(crate::label(&block0).as_deref(), Some("Data"));
    assert_eq!(crate::label(&[0u8; 4096]), None);
    assert_eq!(crate::block_count(&block0), Some(4096));
    assert_eq!(crate::block_count(&[0u8; 4096]), None);
    assert_eq!(v.state(), format::state::CLEAN);
    assert_eq!(v.counts(), (0, 0));
    let root = v.get(ROOT).unwrap();
    assert!(root.is_dir);
    assert_eq!(root.name, b"Data");
    assert_eq!(root.parent, format::cnid::ROOT_PARENT);
    assert!(v.list(ROOT).unwrap().is_empty());
    fsck(&mut v);
    // Mounted for writing, the state says so until unmount.
    let mut w = Volume::mount(&disk, false).unwrap();
    drop(w.list(ROOT));
    let v2 = Volume::mount(&disk, true).unwrap();
    assert_eq!(v2.state(), format::state::MOUNTED);
    w.unmount().unwrap();
    assert_eq!(
        Volume::mount(&disk, true).unwrap().state(),
        format::state::CLEAN
    );
    // Too small, a bad name, a damaged superblock.
    assert_eq!(format(MemDisk::new(100), &options()), Err(Error::Invalid));
    let bad = FormatOptions {
        name: "a/b",
        ..options()
    };
    assert_eq!(format(MemDisk::new(4096), &bad), Err(Error::BadName));
    let mut bytes = disk.to_bytes();
    bytes[1024 + 100] ^= 1;
    assert!(Volume::mount(&MemDisk::from_bytes(bytes), true).is_err());
    // An unknown incompat flag: refused.
    let mut bytes = disk.to_bytes();
    bytes[1024 + 64] = 1;
    let crc = crc32c::crc32c(0, &bytes[1024..1024 + 3068]);
    bytes[1024 + 3068..1024 + 3072].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(
        Volume::mount(&MemDisk::from_bytes(bytes), true).err(),
        Some(Error::Unsupported)
    );
}

#[test]
fn folders_files_and_names() {
    let disk = fresh(8192);
    let mut v = Volume::mount(&disk, false).unwrap();
    let docs = v.create(ROOT, b"Documents", true, NOW).unwrap();
    let f = v
        .create(docs.cnid, "Read Me.txt".as_bytes(), false, NOW)
        .unwrap();
    assert_eq!(
        v.create(docs.cnid, b"READ ME.TXT", false, NOW).err(),
        Some(Error::Exists)
    );
    assert_eq!(
        v.create(f.cnid, b"x", false, NOW).err(),
        Some(Error::NotDir)
    );
    assert_eq!(
        v.create(docs.cnid, b"a/b", false, NOW).err(),
        Some(Error::BadName)
    );
    // Lookups ignore case and normalization; the name keeps its spelling.
    let found = v.lookup(docs.cnid, b"read me.TXT").unwrap();
    assert_eq!(found.cnid, f.cnid);
    assert_eq!(found.name, b"Read Me.txt");
    let cafe = v.create(docs.cnid, "Café".as_bytes(), true, NOW).unwrap();
    assert_eq!(
        v.lookup(docs.cnid, "CAFE\u{301}".as_bytes()).unwrap().cnid,
        cafe.cnid
    );
    assert_eq!(v.get(f.cnid).unwrap().parent, docs.cnid);
    assert_eq!(v.get(docs.cnid).unwrap().valence, 2);
    assert_eq!(v.counts(), (1, 2));
    // Rename (only the case), move, and no folder into itself.
    v.rename(docs.cnid, b"read me.txt", docs.cnid, b"README.txt", NOW)
        .unwrap();
    assert_eq!(v.get(f.cnid).unwrap().name, b"README.txt");
    v.rename(docs.cnid, b"readme.txt", ROOT, b"moved.txt", NOW)
        .unwrap();
    assert_eq!(v.get(f.cnid).unwrap().parent, ROOT);
    assert_eq!(v.get(docs.cnid).unwrap().valence, 1);
    assert_eq!(
        v.rename(ROOT, b"Documents", cafe.cnid, b"x", NOW).err(),
        Some(Error::Invalid)
    );
    assert_eq!(
        v.remove(ROOT, b"Documents", NOW).err(),
        Some(Error::NotEmpty)
    );
    let names: Vec<_> = v.list(ROOT).unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, [b"Documents".to_vec(), b"moved.txt".to_vec()]);
    v.remove(docs.cnid, "café".as_bytes(), NOW).unwrap();
    v.remove(ROOT, b"Documents", NOW).unwrap();
    assert_eq!(v.lookup(ROOT, b"documents").err(), Some(Error::NotFound));
    assert_eq!(v.get(docs.cnid).err(), Some(Error::NotFound));
    fsck(&mut v);
    v.unmount().unwrap();
    let mut r = Volume::mount(&disk, true).unwrap();
    assert_eq!(r.counts(), (1, 0));
    fsck(&mut r);
}

#[test]
fn many_files_split_and_shrink_the_tree() {
    let disk = fresh(16384);
    let mut v = Volume::mount(&disk, false).unwrap();
    let dir = v.create(ROOT, b"many", true, NOW).unwrap();
    for i in 0..2000 {
        let name = format!("file number {i:04} with a long name to fill nodes");
        v.create(dir.cnid, name.as_bytes(), false, NOW).unwrap();
    }
    assert_eq!(v.list(dir.cnid).unwrap().len(), 2000);
    fsck(&mut v);
    // Remove in an interleaved order, checking now and then.
    for i in (0..2000).step_by(2).chain((1..2000).step_by(2)) {
        let name = format!("file number {i:04} with a long name to fill nodes");
        v.remove(dir.cnid, name.as_bytes(), NOW).unwrap();
        if i % 500 == 1 {
            fsck(&mut v);
        }
    }
    assert!(v.list(dir.cnid).unwrap().is_empty());
    fsck(&mut v);
}

fn pattern(n: usize, seed: u8) -> Vec<u8> {
    (0..n)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

#[test]
fn reading_writing_and_extents() {
    let disk = fresh(16384);
    let mut v = Volume::mount(&disk, false).unwrap();
    let f = v.create(ROOT, b"data.bin", false, NOW).unwrap();
    // Across blocks, at an odd offset, with a gap that reads as zeros.
    let data = pattern(10_000, 1);
    v.write(f.cnid, false, 3, &data, NOW).unwrap();
    let mut back = vec![0u8; 10_003];
    assert_eq!(v.read(f.cnid, false, 0, &mut back).unwrap(), 10_003);
    assert_eq!(&back[..3], &[0, 0, 0]);
    assert_eq!(&back[3..], &data[..]);
    v.write(f.cnid, false, 20_000, b"end", NOW).unwrap();
    let mut gap = vec![1u8; 9_997];
    v.read(f.cnid, false, 10_003, &mut gap).unwrap();
    assert!(gap.iter().all(|&b| b == 0));
    assert_eq!(v.get(f.cnid).unwrap().size(), 20_003);
    // A fragmented file: interleave two growing files so that their runs
    // alternate, past 8 runs (into the extents tree).
    let a = v.create(ROOT, b"a", false, NOW).unwrap();
    let b = v.create(ROOT, b"b", false, NOW).unwrap();
    let mut expect_a = Vec::new();
    for i in 0..40u8 {
        let chunk = pattern(4096 * 3, i);
        v.write(a.cnid, false, expect_a.len() as u64, &chunk, NOW)
            .unwrap();
        expect_a.extend_from_slice(&chunk);
        v.write(b.cnid, false, i as u64 * 4096, &pattern(4096, i), NOW)
            .unwrap();
    }
    let mut got = vec![0u8; expect_a.len()];
    v.read(a.cnid, false, 0, &mut got).unwrap();
    assert!(got == expect_a);
    fsck(&mut v);
    // Big write (more than one 256-block transaction).
    let big = pattern(1_500_000, 9);
    let c = v.create(ROOT, b"big", false, NOW).unwrap();
    v.write(c.cnid, false, 0, &big, NOW).unwrap();
    let mut got = vec![0u8; big.len()];
    v.read(c.cnid, false, 0, &mut got).unwrap();
    assert!(got == big);
    // Shrink, grow with zeros, the resource fork, and removal returns
    // every block.
    let free_before = v.free_blocks();
    v.set_length(a.cnid, false, 5_000, NOW).unwrap();
    let mut got = vec![0u8; 5_000];
    v.read(a.cnid, false, 0, &mut got).unwrap();
    assert!(got[..] == expect_a[..5_000]);
    v.set_length(a.cnid, false, 9_000, NOW).unwrap();
    let mut tail = vec![1u8; 4_000];
    v.read(a.cnid, false, 5_000, &mut tail).unwrap();
    assert!(tail.iter().all(|&x| x == 0));
    v.write(a.cnid, true, 0, b"resource", NOW).unwrap();
    let mut r = [0u8; 8];
    v.read(a.cnid, true, 0, &mut r).unwrap();
    assert_eq!(&r, b"resource");
    assert!(v.free_blocks() > free_before);
    fsck(&mut v);
    for name in [&b"a"[..], b"b", b"big", b"data.bin"] {
        v.remove(ROOT, name, NOW).unwrap();
    }
    let fresh_free = Volume::mount(&fresh(16384), true).unwrap().free_blocks();
    // The extents tree may have grown, so allow its nodes.
    assert!(v.free_blocks() <= fresh_free && v.free_blocks() + 64 >= fresh_free);
    fsck(&mut v);
}

#[test]
fn attributes_info_and_ids() {
    let disk = fresh(4096);
    let mut v = Volume::mount(&disk, false).unwrap();
    let f = v.create(ROOT, b"f", false, NOW).unwrap();
    v.attr_set(f.cnid, b"user.color", b"blue").unwrap();
    v.attr_set(f.cnid, b"com.apple.FinderInfo", &[1; 32])
        .unwrap();
    v.attr_set(f.cnid, b"user.color", b"green!").unwrap();
    assert_eq!(v.attr_get(f.cnid, b"user.color").unwrap(), b"green!");
    assert_eq!(
        v.attr_get(f.cnid, b"User.Color").err(),
        Some(Error::NotFound)
    );
    assert_eq!(
        v.attr_set(f.cnid, b"big", &[0; ATTR_VALUE_MAX + 1]).err(),
        Some(Error::Invalid)
    );
    v.attr_set(f.cnid, b"big", &[5; ATTR_VALUE_MAX]).unwrap();
    assert_eq!(v.attr_names(f.cnid).unwrap().len(), 3);
    v.attr_remove(f.cnid, b"big").unwrap();
    assert_eq!(v.attr_names(f.cnid).unwrap().len(), 2);
    // Finder information and times.
    let mut e = v.get(f.cnid).unwrap();
    e.finder[..8].copy_from_slice(b"TEXTttxt");
    e.flags = 1;
    e.times[4] = 42;
    v.set_info(&e).unwrap();
    let back = v.get(f.cnid).unwrap();
    assert_eq!(&back.finder[..8], b"TEXTttxt");
    assert_eq!((back.flags, back.times[4]), (1, 42));
    // CNIDs are not reused.
    v.remove(ROOT, b"f", NOW).unwrap();
    let g = v.create(ROOT, b"g", false, NOW).unwrap();
    assert!(g.cnid > f.cnid);
    fsck(&mut v);
}

#[test]
fn power_cut_at_every_write_of_a_transaction() {
    // A create that splits nothing, then one that writes data: cut the
    // power after each possible number of block writes; the volume must
    // mount (replaying the journal) and pass fsck, with the change either
    // fully there or not at all.
    let base = fresh(4096);
    {
        let mut v = Volume::mount(&base, false).unwrap();
        v.create(ROOT, b"keep", false, NOW).unwrap();
        v.unmount().unwrap();
    }
    let image = base.to_bytes();
    let mut cut = 0;
    loop {
        let disk = MemDisk::from_bytes(image.clone());
        let mut v = Volume::mount(&disk, false).unwrap();
        disk.cut_power_after(cut);
        let started = disk.blocks_written();
        let created = v.create(ROOT, b"new file", false, NOW);
        let wrote = created.and_then(|e| v.write(e.cnid, false, 0, &pattern(9000, 3), NOW));
        let used = disk.blocks_written() - started;
        disk.restore_power();
        drop(v);
        let mut after = Volume::mount(&disk, false).unwrap();
        fsck(&mut after);
        assert!(after.lookup(ROOT, b"keep").is_ok());
        if let Ok(e) = after.lookup(ROOT, b"new file") {
            let size = e.size();
            assert!(size == 0 || size == 9000, "partial length {size}");
            if size == 9000 {
                let mut back = vec![0u8; 9000];
                after.read(e.cnid, false, 0, &mut back).unwrap();
                assert!(back == pattern(9000, 3));
            }
        }
        // Read-only mounts see a committed transaction too.
        let copy = MemDisk::from_bytes(disk.to_bytes());
        assert!(Volume::mount(&copy, true).is_ok());
        if wrote.is_ok() && used < cut {
            break; // everything was written before the cut
        }
        cut += 1;
        assert!(cut < 200);
    }
}

#[test]
fn random_operations_against_a_model() {
    // Files created, written, truncated and removed at random, compared
    // with a model; fsck every 100 steps.
    let disk = fresh(16384);
    let mut v = Volume::mount(&disk, false).unwrap();
    let mut model: BTreeMap<String, (u64, Vec<u8>)> = BTreeMap::new();
    let mut seed = 12345u64;
    let mut rand = move |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    for step in 0..600 {
        let name = format!("f{}", rand(40));
        match (rand(4), model.get(&name).cloned()) {
            (0, None) => {
                let e = v.create(ROOT, name.as_bytes(), false, NOW).unwrap();
                model.insert(name, (e.cnid, Vec::new()));
            }
            (1, Some((cnid, mut data))) => {
                let at = rand(data.len() as u64 + 5000) as usize;
                let chunk = pattern(rand(20_000) as usize + 1, step as u8);
                v.write(cnid, false, at as u64, &chunk, NOW).unwrap();
                if data.len() < at + chunk.len() {
                    data.resize(at + chunk.len(), 0);
                }
                data[at..at + chunk.len()].copy_from_slice(&chunk);
                model.insert(name, (cnid, data));
            }
            (2, Some((cnid, mut data))) => {
                let len = rand(data.len() as u64 + 1) as usize;
                v.set_length(cnid, false, len as u64, NOW).unwrap();
                data.truncate(len);
                model.insert(name, (cnid, data));
            }
            (3, Some(_)) => {
                v.remove(ROOT, name.as_bytes(), NOW).unwrap();
                model.remove(&name);
            }
            _ => {}
        }
        if step % 100 == 99 {
            fsck(&mut v);
            for (cnid, data) in model.values() {
                let mut back = vec![0u8; data.len()];
                assert_eq!(v.read(*cnid, false, 0, &mut back).unwrap(), data.len());
                assert!(&back == data);
            }
        }
    }
    assert_eq!(v.list(ROOT).unwrap().len(), model.len());
}

#[test]
fn listing_in_pieces_after_a_name() {
    let disk = fresh(8192);
    let mut v = Volume::mount(&disk, false).unwrap();
    let dir = v.create(ROOT, b"many", true, NOW).unwrap();
    let names: Vec<Vec<u8>> = (0..300)
        .map(|i| format!("item {i:03}").into_bytes())
        .collect();
    for n in &names {
        v.create(dir.cnid, n, false, NOW).unwrap();
    }
    let whole: Vec<Vec<u8>> = v
        .list(dir.cnid)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    // One at a time, each after the previous name: the same order.
    let mut pieces = Vec::new();
    let mut after: Option<Vec<u8>> = None;
    loop {
        let next = v.list_after(dir.cnid, after.as_deref(), 1).unwrap();
        let Some(e) = next.into_iter().next() else {
            break;
        };
        after = Some(e.name.clone());
        pieces.push(e.name);
    }
    assert_eq!(pieces, whole);
    assert_eq!(pieces.len(), 300);
    // In bigger pieces, and after a name that is gone.
    let first = v.list_after(dir.cnid, None, 10).unwrap();
    assert_eq!(first.len(), 10);
    v.remove(dir.cnid, &first[9].name, NOW).unwrap();
    let rest = v.list_after(dir.cnid, Some(&first[9].name), 1000).unwrap();
    assert_eq!(rest.len(), 290);
    assert_eq!(rest[0].name, whole[10]);
    // Nothing after the last, nor from the folder's own thread.
    assert!(
        v.list_after(dir.cnid, Some(&whole[299]), 5)
            .unwrap()
            .is_empty()
    );
    assert!(v.list_after(dir.cnid, Some(b"zzz"), 5).unwrap().is_empty());
}
