// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sound output (`docs/sound.md`): the HDA controller's stream fed by a
//! kernel thread (`audio`) from the ring a program shares with it.
//!
//! `AudioOpen` (one program at a time) makes the ring (`abi::audio`),
//! starts the stream and the feeding; every 5 ms the thread moves the
//! program's samples into the cyclic DMA buffer, keeping it 16 KiB ahead
//! of the controller, with silence when the program falls behind (the
//! stream never stops while open: no pops). `AudioClose`, or the last
//! handle going away, stops it.

use crate::audiofeed::{Feeder, from_ring};
use crate::object::{Entry, KernelObject};
use crate::task::queue::WaitQueue;
use crate::task::{self, Process};
use abi::audio as ring;
use abi::{Error, Handle, Rights};
use alloc::sync::Arc;
use alloc::vec;
use drivers::hda::Hda;
use frame::mm::shared::SharedMemory;
use frame::sync::SpinLock;

struct Playback {
    memory: SharedMemory,
    feeder: Feeder,
    underruns: u32,
}

struct Audio {
    hda: Hda,
    playing: Option<Playback>,
}

static AUDIO: SpinLock<Option<Audio>> = SpinLock::new(None);
/// Threads waiting in a `WaitSet` that holds the output: woken by the
/// feeding thread when the ring is at least half empty.
static WATCHERS: SpinLock<WaitQueue> = SpinLock::new(WaitQueue::new());

/// What an Audio handle refers to: the open output.
pub struct AudioSession;

impl Drop for AudioSession {
    fn drop(&mut self) {
        stop();
    }
}

/// Probes the HDA controller and starts the audio thread. Call once at
/// boot, after `block::probe`.
pub fn start() {
    let Some(hda) = drivers::hda::probe() else {
        return;
    };
    hda.silence();
    *AUDIO.lock() = Some(Audio { hda, playing: None });
    if let Err(e) = task::spawn("audio", run) {
        frame::kprintln!("audio: cannot start the thread: {e:?}");
    }
}

fn u32_at(m: &SharedMemory, at: usize) -> u32 {
    let mut b = [0u8; 4];
    let _ = m.read(at as u64, &mut b);
    u32::from_le_bytes(b)
}

fn set_u32(m: &SharedMemory, at: usize, v: u32) {
    let _ = m.write(at as u64, &v.to_le_bytes());
}

/// One step of feeding: the program's bytes, then silence, up to the
/// lead.
fn feed(a: &mut Audio) {
    let Some(p) = a.playing.as_mut() else {
        return;
    };
    let mut want = p.feeder.advance(a.hda.position());
    let mut chunk = vec![0u8; 4096];
    while want > 0 {
        let (at, n) = p.feeder.piece(want.min(chunk.len() as u64));
        let write = u32_at(&p.memory, ring::WRITE_POS);
        let read = u32_at(&p.memory, ring::READ_POS);
        let size = ring::RING_BYTES;
        let from = from_ring(write, read, n);
        // The ring's bytes may wrap too: at most up to its end.
        let ring_at = read % size;
        let from = from.min((size - ring_at) as usize);
        let buf = &mut chunk[..n];
        if from > 0 {
            let _ = p
                .memory
                .read(ring::DATA as u64 + u64::from(ring_at), &mut buf[..from]);
            set_u32(&p.memory, ring::READ_POS, read.wrapping_add(from as u32));
        }
        if from < n {
            buf[from..].fill(0);
            // The ring ran dry in the middle of this piece: the program
            // fell behind (an idle program's silence is not counted).
            if from > 0 {
                p.underruns = p.underruns.wrapping_add(1);
                set_u32(&p.memory, ring::UNDERRUNS, p.underruns);
            }
        }
        a.hda.write(at, buf);
        p.feeder.wrote(n);
        want -= n as u64;
    }
}

fn run() {
    loop {
        if let Some(a) = AUDIO.lock().as_mut() {
            feed(a);
        }
        if events() != 0 {
            let woken = WATCHERS.lock().drain();
            for id in woken {
                task::wake(id);
            }
        }
        task::sleep_ms(5);
    }
}

/// The `abi::wait` events of the output: `WRITABLE` while the ring is
/// at least half empty.
pub fn events() -> u32 {
    let guard = AUDIO.lock();
    let Some(p) = guard.as_ref().and_then(|a| a.playing.as_ref()) else {
        return 0;
    };
    let used = u32_at(&p.memory, ring::WRITE_POS).wrapping_sub(u32_at(&p.memory, ring::READ_POS));
    if used <= ring::RING_BYTES / 2 {
        abi::wait::WRITABLE
    } else {
        0
    }
}

/// Thread `me` (in a `WaitSet` wait) is woken when the ring has room.
pub fn watch(me: task::ThreadId) {
    WATCHERS.lock().add(me);
}

/// Undoes [`watch`].
pub fn unwatch(me: task::ThreadId) {
    WATCHERS.lock().remove(me);
}

fn stop() {
    if let Some(a) = AUDIO.lock().as_mut()
        && a.playing.take().is_some()
    {
        a.hda.stop();
        a.hda.silence();
    }
}

/// `AudioOpen(rate, channels, &mut [u64; 2])`.
pub fn audio_open(process: &Process, rate: u64, channels: u64, out: u64) -> Result<u64, Error> {
    if rate != u64::from(ring::RATE) || channels != u64::from(ring::CHANNELS) {
        return Err(Error::NotSupported);
    }
    if !process.space().check(out, 16, true) {
        return Err(Error::BadAddress);
    }
    let memory =
        SharedMemory::new((ring::MEMORY_SIZE / 4096) as usize).ok_or(Error::OutOfMemory)?;
    set_u32(&memory, ring::SIZE, ring::RING_BYTES);
    set_u32(&memory, ring::RATE_AT, ring::RATE);
    set_u32(&memory, ring::CHANNELS_AT, ring::CHANNELS);
    {
        let mut guard = AUDIO.lock();
        let a = guard.as_mut().ok_or(Error::NotSupported)?;
        if a.playing.is_some() {
            return Err(Error::Busy);
        }
        a.hda.silence();
        a.hda.start();
        let feeder = Feeder::new(drivers::hda::RING_BYTES, a.hda.position());
        a.playing = Some(Playback {
            memory: memory.clone(),
            feeder,
            underruns: 0,
        });
    }
    let session = Arc::new(AudioSession);
    let mut handles = process.handles().lock();
    let audio = handles.insert(Entry {
        object: KernelObject::Audio(session),
        rights: Rights::READ | Rights::WRITE | Rights::DUPLICATE,
    })?;
    let mem = match handles.insert(Entry {
        object: KernelObject::Memory(memory),
        rights: Rights::READ | Rights::WRITE | Rights::MAP | Rights::DUPLICATE,
    }) {
        Ok(h) => h,
        Err(e) => {
            let _ = handles.remove(audio);
            return Err(e);
        }
    };
    drop(handles);
    let mut b = [0u8; 16];
    b[0..8].copy_from_slice(&audio.raw().to_le_bytes());
    b[8..16].copy_from_slice(&mem.raw().to_le_bytes());
    process
        .space()
        .write(out, &b)
        .map_err(|_| Error::BadAddress)?;
    Ok(0)
}

/// `AudioClose(audio)`.
pub fn audio_close(process: &Process, handle: u64) -> Result<u64, Error> {
    let entry = {
        let mut handles = process.handles().lock();
        if !matches!(
            handles.get(Handle::from_raw(handle))?.object,
            KernelObject::Audio(_)
        ) {
            return Err(Error::WrongType);
        }
        handles.remove(Handle::from_raw(handle))?
    };
    // Duplicates keep the session; the stream stops with the last one.
    drop(entry);
    Ok(0)
}
