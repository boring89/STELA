// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sound output (`docs/sound.md`): the HDA controller's stream fed by a
//! kernel thread (`audio`) from the rings programs share with it.
//!
//! `AudioOpen` makes a ring (`abi::audio`) and a session; the first one
//! starts the stream. Every 5 ms the thread fills the cyclic DMA buffer up
//! to 16 KiB ahead of the controller: P13 K18, every session's samples
//! (at its volume, `AudioSetVolume`) are added up in 32 bits, the master
//! volume and mute (`AudioMaster`, `SysInfo(AUDIO)`) applied, and the sum
//! clipped to 16 bits; a session that falls behind adds silence. The
//! stream never stops while a session is open (no pops); the last session
//! going away stops it.

use crate::audiofeed::{Feeder, finish, from_ring, mix_into};
use crate::object::{Entry, KernelObject};
use crate::task::queue::WaitQueue;
use crate::task::{self, Process};
use abi::audio as ring;
use abi::{Error, Handle, Rights};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use drivers::hda::Hda;
use frame::mm::shared::SharedMemory;
use frame::sync::SpinLock;

struct Playback {
    id: u64,
    memory: SharedMemory,
    volume: Arc<AtomicU8>,
    underruns: u32,
}

struct Audio {
    hda: Hda,
    /// The open sessions (P13 K18: mixed).
    playing: Vec<Playback>,
    /// The stream's positions while any session is open.
    feeder: Option<Feeder>,
    /// Master volume 0-100 and mute.
    master: u8,
    muted: bool,
}

static AUDIO: SpinLock<Option<Audio>> = SpinLock::new(None);
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
/// Threads waiting in a `WaitSet` that holds an output: woken by the
/// feeding thread when a ring is at least half empty.
static WATCHERS: SpinLock<WaitQueue> = SpinLock::new(WaitQueue::new());

/// What an Audio handle refers to: one open output (a session).
pub struct AudioSession {
    id: u64,
    volume: Arc<AtomicU8>,
}

impl Drop for AudioSession {
    fn drop(&mut self) {
        stop(self.id);
    }
}

/// The sound card, for the hardware report (P11): the HDA controller's
/// description, if there is one.
pub fn description() -> Option<alloc::string::String> {
    AUDIO.lock().as_ref().map(|a| a.hda.description.clone())
}

/// Probes the HDA controller and starts the audio thread. Call once at
/// boot, after `block::probe`.
pub fn start() {
    let Some(hda) = drivers::hda::probe() else {
        return;
    };
    hda.silence();
    *AUDIO.lock() = Some(Audio {
        hda,
        playing: Vec::new(),
        feeder: None,
        master: 100,
        muted: false,
    });
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

/// Takes up to `n` bytes from a session's ring into `buf` (the rest stays
/// zero); counts an underrun when the ring runs dry in the middle.
fn take(p: &mut Playback, buf: &mut [u8]) {
    let n = buf.len();
    let write = u32_at(&p.memory, ring::WRITE_POS);
    let read = u32_at(&p.memory, ring::READ_POS);
    let size = ring::RING_BYTES;
    let from = from_ring(write, read, n);
    // The ring's bytes may wrap too: at most up to its end.
    let ring_at = read % size;
    let first = from.min((size - ring_at) as usize);
    if first > 0 {
        let _ = p
            .memory
            .read(ring::DATA as u64 + u64::from(ring_at), &mut buf[..first]);
    }
    if from > first {
        let _ = p.memory.read(ring::DATA as u64, &mut buf[first..from]);
    }
    set_u32(&p.memory, ring::READ_POS, read.wrapping_add(from as u32));
    buf[from..].fill(0);
    // An idle program's silence is not counted.
    if from > 0 && from < n {
        p.underruns = p.underruns.wrapping_add(1);
        set_u32(&p.memory, ring::UNDERRUNS, p.underruns);
    }
}

/// One step of feeding: every session's bytes mixed, up to the lead.
fn feed(a: &mut Audio) {
    let Some(feeder) = a.feeder.as_mut() else {
        return;
    };
    let mut want = feeder.advance(a.hda.position());
    let mut chunk = vec![0u8; 4096];
    let mut source = vec![0u8; 4096];
    let mut acc = vec![0i32; 2048];
    while want > 0 {
        let (at, n) = feeder.piece(want.min(chunk.len() as u64));
        let samples = n / 2;
        if a.playing.len() == 1 && a.master == 100 && !a.muted {
            // One session at full volume: its bytes as they are.
            let p = &mut a.playing[0];
            take(p, &mut chunk[..n]);
            let v = p.volume.load(Ordering::Relaxed);
            if v < 100 {
                acc[..samples].fill(0);
                mix_into(&mut acc[..samples], &chunk[..n], v);
                finish(&acc[..samples], 100, false, &mut chunk[..n]);
            }
        } else {
            acc[..samples].fill(0);
            for p in a.playing.iter_mut() {
                take(p, &mut source[..n]);
                mix_into(
                    &mut acc[..samples],
                    &source[..n],
                    p.volume.load(Ordering::Relaxed),
                );
            }
            finish(&acc[..samples], a.master, a.muted, &mut chunk[..n]);
        }
        a.hda.write(at, &chunk[..n]);
        feeder.wrote(n);
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

/// The `abi::wait` events of the outputs: `WRITABLE` while some ring is
/// at least half empty.
pub fn events() -> u32 {
    let guard = AUDIO.lock();
    let Some(a) = guard.as_ref() else {
        return 0;
    };
    let room = a.playing.iter().any(|p| {
        let used =
            u32_at(&p.memory, ring::WRITE_POS).wrapping_sub(u32_at(&p.memory, ring::READ_POS));
        used <= ring::RING_BYTES / 2
    });
    if room { abi::wait::WRITABLE } else { 0 }
}

/// Thread `me` (in a `WaitSet` wait) is woken when the ring has room.
pub fn watch(me: task::ThreadId) {
    WATCHERS.lock().add(me);
}

/// Undoes [`watch`].
pub fn unwatch(me: task::ThreadId) {
    WATCHERS.lock().remove(me);
}

/// Session `id` ends; the last one stops the stream.
fn stop(id: u64) {
    if let Some(a) = AUDIO.lock().as_mut() {
        a.playing.retain(|p| p.id != id);
        if a.playing.is_empty() && a.feeder.take().is_some() {
            a.hda.stop();
            a.hda.silence();
        }
    }
}

/// The master volume and mute (`SysInfo(AUDIO)`): (volume, muted,
/// sessions); `None` without a sound card.
pub fn master() -> Option<(u8, bool, usize)> {
    AUDIO
        .lock()
        .as_ref()
        .map(|a| (a.master, a.muted, a.playing.len()))
}

/// `AudioMaster(volume, mute, power)` (P13 K18): needs the `POWER`
/// capability.
pub fn audio_master(process: &Process, volume: u64, mute: u64, power: u64) -> Result<u64, Error> {
    crate::object::check_capability(process, power, abi::capability::POWER)?;
    let mut guard = AUDIO.lock();
    let a = guard.as_mut().ok_or(Error::NotSupported)?;
    a.master = volume.min(100) as u8;
    a.muted = mute != 0;
    Ok(0)
}

/// `AudioSetVolume(audio, volume)` (P13 K18): this session's volume,
/// 0-100.
pub fn audio_set_volume(process: &Process, handle: u64, volume: u64) -> Result<u64, Error> {
    let handles = process.handles().lock();
    match &handles.get(Handle::from_raw(handle))?.object {
        KernelObject::Audio(s) => {
            s.volume.store(volume.min(100) as u8, Ordering::Relaxed);
            Ok(0)
        }
        _ => Err(Error::WrongType),
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
    let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    let volume = Arc::new(AtomicU8::new(100));
    {
        let mut guard = AUDIO.lock();
        let a = guard.as_mut().ok_or(Error::NotSupported)?;
        // P13 K18: more sessions are mixed in; the first starts the stream.
        if a.feeder.is_none() {
            a.hda.silence();
            a.hda.start();
            a.feeder = Some(Feeder::new(drivers::hda::RING_BYTES, a.hda.position()));
        }
        a.playing.push(Playback {
            id,
            memory: memory.clone(),
            volume: Arc::clone(&volume),
            underruns: 0,
        });
    }
    let session = Arc::new(AudioSession { id, volume });
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
