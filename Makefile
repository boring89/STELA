# STELA build driver: a thin wrapper around cargo and QEMU.

BUILD      := build
ESP        := $(BUILD)/esp
UEFI_TARGET := x86_64-unknown-uefi
BOOT_EFI   := $(BUILD)/target/$(UEFI_TARGET)/release/stela-boot.efi

OVMF_CODE  := /usr/share/OVMF/OVMF_CODE_4M.fd
OVMF_VARS  := /usr/share/OVMF/OVMF_VARS_4M.fd
QEMU       := qemu-system-x86_64
QEMU_FLAGS := -machine q35 -m 512M -no-reboot \
              -drive if=pflash,format=raw,readonly=on,file=$(OVMF_CODE) \
              -drive if=pflash,format=raw,file=$(BUILD)/OVMF_VARS.fd \
              -drive format=raw,file=fat:rw:$(ESP)

# Crates that run on the host and have unit tests (none yet).
HOST_CRATES :=

.PHONY: all boot esp run test test-host test-qemu clippy fmt clean

all: esp

boot:
	cargo build --release -p stela-boot --target $(UEFI_TARGET)

# The EFI System Partition as a directory; QEMU serves it as a virtual FAT disk.
esp: boot
	mkdir -p $(ESP)/EFI/BOOT
	cp $(BOOT_EFI) $(ESP)/EFI/BOOT/BOOTX64.EFI

$(BUILD)/OVMF_VARS.fd:
	mkdir -p $(BUILD)
	cp $(OVMF_VARS) $@

run: esp $(BUILD)/OVMF_VARS.fd
	$(QEMU) $(QEMU_FLAGS) -serial stdio

test: test-host test-qemu

test-host:
	@if [ -n "$(HOST_CRATES)" ]; then cargo test $(addprefix -p ,$(HOST_CRATES)); else echo "test-host: no host crates yet"; fi

test-qemu: esp $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --expect "STELA boot loader" -- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio

clippy:
	cargo clippy --release -p stela-boot --target $(UEFI_TARGET) -- -D warnings

fmt:
	cargo fmt --all

clean:
	rm -rf $(BUILD)
