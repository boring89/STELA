// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host tests of the C core: the 68k test programs of `tests/m68k`
//! (assembled with `m68k-linux-gnu-as`, as in 68k_HAL) run on Musashi.

use crate::ffi::{Cpu, Reg, Vm};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// The C side has one machine: tests take turns.
static MACHINE: Mutex<()> = Mutex::new(());

const PROGRAM_BASE: u32 = 0x1000;
const RAM_SIZE: u32 = 0x10000;
const STACK_TOP: u32 = 0x8000;
const MAX_CYCLES: i32 = 100_000;

/// `tests/m68k/<name>.s` assembled and flattened to its `.text`.
fn program(name: &str) -> Vec<u8> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env!("OUT_DIR")).join("m68k");
    std::fs::create_dir_all(&out).expect("create m68k dir");
    let obj = out.join(format!("{name}.o"));
    let bin = out.join(format!("{name}.bin"));
    let status = Command::new("m68k-linux-gnu-as")
        .arg("-m68030")
        .arg("-o")
        .arg(&obj)
        .arg(root.join("tests/m68k").join(format!("{name}.s")))
        .status()
        .expect("m68k-linux-gnu-as (package binutils-m68k-linux-gnu)");
    assert!(status.success());
    let status = Command::new("m68k-linux-gnu-objcopy")
        .args(["-O", "binary", "-j", ".text"])
        .arg(&obj)
        .arg(&bin)
        .status()
        .expect("m68k-linux-gnu-objcopy");
    assert!(status.success());
    std::fs::read(&bin).expect("read the program")
}

/// Loads and runs a program until STOP.
fn run_program(name: &str, cpu: Cpu) -> Vm {
    let code = program(name);
    let mut vm = Vm::new(RAM_SIZE, cpu).expect("a machine");
    assert!(vm.load(PROGRAM_BASE, &code));
    assert!(vm.reset(PROGRAM_BASE, STACK_TOP));
    vm.run(MAX_CYCLES);
    assert!(vm.halted(), "{name} did not reach STOP");
    vm
}

#[test]
fn memory_is_big_endian() {
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    let mut vm = Vm::new(RAM_SIZE, Cpu::M68030).expect("a machine");
    vm.write(0x100, 4, 0x1234_5678);
    assert_eq!(vm.ram()[0x100], 0x12);
    assert_eq!(vm.ram()[0x103], 0x78);
    assert_eq!(vm.read(0x100, 2), 0x1234);
    assert_eq!(vm.read(0x100, 4), 0x1234_5678);
    // Out of range: ignored, reads 0; loads that do not fit fail.
    vm.write(RAM_SIZE, 1, 0xFF);
    assert_eq!(vm.read(RAM_SIZE, 1), 0);
    assert!(!vm.load(RAM_SIZE - 1, b"ab"));
}

#[test]
fn one_machine_at_a_time() {
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        Vm::new(4, Cpu::M68030).is_none(),
        "tiny RAM must be refused"
    );
    let vm = Vm::new(RAM_SIZE, Cpu::M68030).expect("a machine");
    assert!(Vm::new(RAM_SIZE, Cpu::M68030).is_none());
    drop(vm);
    assert!(Vm::new(RAM_SIZE, Cpu::M68030).is_some());
}

#[test]
fn move_instructions() {
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    for cpu in [Cpu::M68030, Cpu::M68000] {
        let vm = run_program("move", cpu);
        assert_eq!(vm.reg(Reg::D(0)), 0x1234_5678);
        assert_eq!(vm.reg(Reg::D(1)), 0xFFFF_FF80);
        assert_eq!(vm.reg(Reg::D(2)), 0x1234_FFEE);
        assert_eq!(vm.reg(Reg::D(3)), 0xAB);
        assert_eq!(vm.reg(Reg::D(4)), 0x1234_5678);
        assert_eq!(vm.ram()[0x2000], 0x12);
        assert_eq!(vm.ram()[0x2003], 0x78);
    }
}

#[test]
fn arithmetic_flags() {
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    let vm = run_program("arith", Cpu::M68030);
    // CCR bits: X=0x10 N=0x08 Z=0x04 V=0x02 C=0x01.
    assert_eq!(vm.reg(Reg::D(4)) & 0x1F, 0x08 | 0x02);
    assert_eq!(vm.reg(Reg::D(5)) & 0x1F, 0x10 | 0x04 | 0x01);
    assert_eq!(vm.reg(Reg::D(6)) & 0x1F, 0x10 | 0x08 | 0x01);
    // CMP leaves X as the SUBQ before it set it.
    assert_eq!(vm.reg(Reg::D(7)) & 0x1F, 0x10 | 0x04);
}

#[test]
fn branches_and_subroutines() {
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    let vm = run_program("loop", Cpu::M68030);
    assert_eq!(
        [0, 1, 2, 3].map(|n| vm.reg(Reg::D(n))),
        [55, 3, 1, 0],
        "loop"
    );
    drop(vm);
    let vm = run_program("subroutine", Cpu::M68030);
    assert_eq!(vm.reg(Reg::D(0)), 42);
    assert_eq!(vm.reg(Reg::D(1)), 0x1111_1111);
    assert_eq!(vm.reg(Reg::A(7)), STACK_TOP);
}

#[test]
fn exceptions() {
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    let vm = run_program("divzero", Cpu::M68030);
    assert_eq!(vm.reg(Reg::D(7)), 0xDEAD);
    assert_eq!(vm.reg(Reg::D(0)), 7);
    assert_eq!(vm.reg(Reg::D(1)), 7);
    drop(vm);
    let vm = run_program("fline", Cpu::M68030);
    assert_eq!(vm.reg(Reg::D(7)), 0xF11E);
    assert_eq!(vm.reg(Reg::D(0)), 7);
    assert_eq!(vm.reg(Reg::D(4)), vm.reg(Reg::D(6)));
    assert_eq!(vm.reg(Reg::D(5)), 0x2C);
}

#[test]
fn a_line_traps_are_logged() {
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    let vm = run_program("aline", Cpu::M68030);
    // Execution continues after each trap.
    assert_eq!([0, 1, 2].map(|n| vm.reg(Reg::D(n))), [1, 2, 3]);
    assert_eq!(vm.trap_total(), 2);
    assert_eq!(vm.trap_log(0), Some((0xA9F4, PROGRAM_BASE + 2)));
    assert_eq!(vm.trap_log(1).map(|t| t.0), Some(0xA000));
    assert_eq!(vm.trap_log(2), None);
}

static TRAPPED: AtomicU32 = AtomicU32::new(0);

extern "C" fn record_trap(opcode: u16, _pc: u32) {
    TRAPPED.store(u32::from(opcode), Ordering::SeqCst);
}

#[test]
fn a_line_handler_runs() {
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    // A-line trap $A9F4, then STOP #$2700.
    let code = [0xA9, 0xF4, 0x4E, 0x72, 0x27, 0x00];
    let mut vm = Vm::new(RAM_SIZE, Cpu::M68030).expect("a machine");
    assert!(vm.set_trap_handler(0x9F4, Some(record_trap)));
    assert!(!vm.set_trap_handler(0x1000, Some(record_trap)));
    assert!(vm.load(PROGRAM_BASE, &code));
    assert!(vm.reset(PROGRAM_BASE, STACK_TOP));
    vm.run(MAX_CYCLES);
    assert_eq!(TRAPPED.load(Ordering::SeqCst), 0xA9F4);
    assert!(vm.halted());
}

#[test]
fn trap_log_wraps() {
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    let mut vm = Vm::new(RAM_SIZE, Cpu::M68030).expect("a machine");
    for i in 0..32 + 3 {
        vm.dispatch_trap(0xA000 + i, 0x100 + u32::from(i));
    }
    assert_eq!(vm.trap_total(), 35);
    assert_eq!(vm.trap_log_len(), 32);
    assert_eq!(vm.trap_log(0).map(|t| t.0), Some(0xA003));
    assert_eq!(vm.trap_log(31).map(|t| t.0), Some(0xA000 + 34));
}

#[test]
fn run_stops_at_the_cycle_limit() {
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    // BRA.B -2 loops forever.
    let mut vm = Vm::new(RAM_SIZE, Cpu::M68030).expect("a machine");
    assert!(vm.load(PROGRAM_BASE, &[0x60, 0xFE]));
    assert!(vm.reset(PROGRAM_BASE, STACK_TOP));
    assert!(vm.run(1000) >= 1000);
    assert!(!vm.halted());
}

struct NoDisk;

impl crate::ffi::MacDisk for NoDisk {
    fn blocks(&self) -> u32 {
        0
    }
    fn read_only(&self) -> bool {
        true
    }
    fn read(&mut self, _offset: u32, _buf: &mut [u8]) -> bool {
        false
    }
    fn write(&mut self, _offset: u32, _data: &[u8]) -> bool {
        false
    }
}

#[test]
fn mac_refuses_other_roms() {
    use crate::ffi::{Mac, StartError};
    let _turn = MACHINE.lock().unwrap_or_else(|e| e.into_inner());
    // Not the IIci ROM (its checksum is 368CADFE): nothing of Apple's is
    // in the tests.
    let rom = vec![0u8; 512 * 1024].into_boxed_slice();
    let r = Mac::start(rom, 8 << 20, Box::new(NoDisk), &[0; 256], 0);
    assert_eq!(r.err(), Some(StartError::BadRom));
    // One machine at a time: a bare 68k machine blocks the Mac.
    let vm = Vm::new(RAM_SIZE, Cpu::M68030).expect("a machine");
    let rom = vec![0u8; 512 * 1024].into_boxed_slice();
    let r = Mac::start(rom, 8 << 20, Box::new(NoDisk), &[0; 256], 0);
    assert_eq!(r.err(), Some(StartError::Busy));
    drop(vm);
    assert!(Vm::new(RAM_SIZE, Cpu::M68030).is_some());
}
