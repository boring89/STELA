# STELA build driver: a thin wrapper around cargo and QEMU.

BUILD       := build
ESP         := $(BUILD)/esp
ESP_TEST    := $(BUILD)/esp-test
UEFI_TARGET := x86_64-unknown-uefi
KERNEL_TARGET := x86_64-unknown-none
BOOT_EFI    := $(BUILD)/target/$(UEFI_TARGET)/release/stela-boot.efi
KERNEL_ELF  := $(BUILD)/target/$(KERNEL_TARGET)/release/kernel

OVMF_CODE   := /usr/share/OVMF/OVMF_CODE_4M.fd
OVMF_VARS   := /usr/share/OVMF/OVMF_VARS_4M.fd
QEMU        := qemu-system-x86_64
QEMU_BASE   := -machine q35 -m 512M -no-reboot \
               -drive if=pflash,format=raw,readonly=on,file=$(OVMF_CODE) \
               -drive if=pflash,format=raw,file=$(BUILD)/OVMF_VARS.fd
QEMU_FLAGS  := $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP)
# The self-test kernel leaves QEMU through isa-debug-exit: writing v to port
# 0xF4 exits with status (v << 1) | 1, so 0x10 (success) gives 33.
QEMU_TEST_FLAGS := $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP_TEST) \
               -device isa-debug-exit,iobase=0xf4,iosize=0x04 -display none -serial stdio

# Crates that run on the host and have unit tests.
HOST_CRATES := bootinfo frame
# Crates built for the bare-metal kernel target.
KERNEL_CRATES := bootinfo frame kernel

.PHONY: all boot kernel kernel-test esp esp-test run test test-host test-qemu clippy fmt clean

all: esp

boot:
	cargo build --release -p stela-boot --target $(UEFI_TARGET)

kernel:
	cargo build --release -p kernel --target $(KERNEL_TARGET)

# The EFI System Partition as a directory; QEMU serves it as a virtual FAT disk.
esp: boot kernel
	mkdir -p $(ESP)/EFI/BOOT $(ESP)/STELA
	cp $(BOOT_EFI) $(ESP)/EFI/BOOT/BOOTX64.EFI
	cp $(KERNEL_ELF) $(ESP)/STELA/kernel.elf

# Same loader, kernel built with the selftest feature, in its own ESP so that
# `make run` never runs the self tests.
esp-test: boot
	cargo build --release -p kernel --target $(KERNEL_TARGET) --features selftest
	mkdir -p $(ESP_TEST)/EFI/BOOT $(ESP_TEST)/STELA
	cp $(BOOT_EFI) $(ESP_TEST)/EFI/BOOT/BOOTX64.EFI
	cp $(KERNEL_ELF) $(ESP_TEST)/STELA/kernel.elf

$(BUILD)/OVMF_VARS.fd:
	mkdir -p $(BUILD)
	cp $(OVMF_VARS) $@

run: esp $(BUILD)/OVMF_VARS.fd
	$(QEMU) $(QEMU_FLAGS) -serial stdio

test: test-host test-qemu

test-host:
	cargo test $(addprefix -p ,$(HOST_CRATES))

test-qemu: esp-test $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --expect "STELA boot loader" -- $(QEMU) $(QEMU_TEST_FLAGS)

clippy:
	cargo clippy --release -p stela-boot --target $(UEFI_TARGET) -- -D warnings
	cargo clippy --release $(addprefix -p ,$(KERNEL_CRATES)) --target $(KERNEL_TARGET) -- -D warnings
	cargo clippy --release $(addprefix -p ,$(KERNEL_CRATES)) --target $(KERNEL_TARGET) --features kernel/selftest -- -D warnings
	cargo clippy --tests $(addprefix -p ,$(HOST_CRATES)) -- -D warnings

fmt:
	cargo fmt --all

clean:
	rm -rf $(BUILD)
