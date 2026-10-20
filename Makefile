# STELA build driver: a thin wrapper around cargo and QEMU.

BUILD       := build
ESP         := $(BUILD)/esp
ESP_TEST    := $(BUILD)/esp-test
UEFI_TARGET := x86_64-unknown-uefi
KERNEL_TARGET := x86_64-unknown-none
BOOT_EFI    := $(BUILD)/target/$(UEFI_TARGET)/release/stela-boot.efi
KERNEL_ELF  := $(BUILD)/target/$(KERNEL_TARGET)/release/kernel
# User programs packed into the initrd (user/<name>, built for the same
# bare-metal target as the kernel, linked with user/user.ld).
USER_PROGRAMS := hello spin fault
USER_DIR    := $(BUILD)/user
INITRD      := $(BUILD)/initrd.tar

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
# The hardware baseline (docs/architecture.md section 3): a Core 2 class CPU
# without the NX bit, 256 MiB, no KVM. -m comes last and overrides 512M.
QEMU_MIN_FLAGS := $(QEMU_TEST_FLAGS) -cpu Conroe,-nx -m 256M

# Crates that run on the host and have unit tests.
HOST_CRATES := abi bootinfo elfload frame
# Crates with a bare-metal binary whose library part has host unit tests.
HOST_LIB_CRATES := kernel
# Crates built for the bare-metal kernel target.
KERNEL_CRATES := abi bootinfo frame kernel
# User-space crates (same target).
USER_CRATES := libstela $(USER_PROGRAMS)

.PHONY: all boot kernel user initrd esp esp-test run test test-host test-qemu test-qemu-min test-fb clippy fmt clean

all: esp

boot:
	cargo build --release -p stela-boot --target $(UEFI_TARGET)

kernel:
	cargo build --release -p kernel --target $(KERNEL_TARGET)

user:
	cargo build --release $(addprefix -p ,$(USER_PROGRAMS)) --target $(KERNEL_TARGET)
	mkdir -p $(USER_DIR)
	cp $(addprefix $(BUILD)/target/$(KERNEL_TARGET)/release/,$(USER_PROGRAMS)) $(USER_DIR)/

# The initial RAM disk: the user programs in a ustar archive, with fixed
# owners and times so that the same programs give the same bytes.
initrd: user
	tar --format=ustar --owner=0 --group=0 --numeric-owner --mtime=@0 \
		-cf $(INITRD) -C $(USER_DIR) $(USER_PROGRAMS)

# The EFI System Partition as a directory; QEMU serves it as a virtual FAT disk.
esp: boot kernel initrd
	mkdir -p $(ESP)/EFI/BOOT $(ESP)/STELA
	cp $(BOOT_EFI) $(ESP)/EFI/BOOT/BOOTX64.EFI
	cp $(KERNEL_ELF) $(ESP)/STELA/kernel.elf
	cp $(INITRD) $(ESP)/STELA/initrd.tar

# Same loader, kernel built with the selftest feature, in its own ESP so that
# `make run` never runs the self tests.
esp-test: boot initrd
	cargo build --release -p kernel --target $(KERNEL_TARGET) --features selftest
	mkdir -p $(ESP_TEST)/EFI/BOOT $(ESP_TEST)/STELA
	cp $(BOOT_EFI) $(ESP_TEST)/EFI/BOOT/BOOTX64.EFI
	cp $(KERNEL_ELF) $(ESP_TEST)/STELA/kernel.elf
	cp $(INITRD) $(ESP_TEST)/STELA/initrd.tar

$(BUILD)/OVMF_VARS.fd:
	mkdir -p $(BUILD)
	cp $(OVMF_VARS) $@

run: esp $(BUILD)/OVMF_VARS.fd
	$(QEMU) $(QEMU_FLAGS) -serial stdio

test: test-host test-qemu test-qemu-min test-fb

test-host: initrd
	cargo test $(addprefix -p ,$(HOST_CRATES))
	cargo test --lib $(addprefix -p ,$(HOST_LIB_CRATES))

# The keyboard self test prints "input: waiting"; the script then types
# "stela" and Enter through QMP send-key.
SEND_KEYS := --send-keys "input: waiting" "s t e l a ret"

test-qemu: esp-test $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --exit-code 33 $(SEND_KEYS) --expect "STELA boot loader" \
		--expect "STELA kernel" --expect "input: OK" \
		--expect "user: hello from built-in user code" --expect "hello from user mode" \
		--expect "hello from a second thread" --expect "fault: reading address 0" \
		--expect "spin: " --expect "selftest: OK" \
		-- $(QEMU) $(QEMU_TEST_FLAGS)

test-qemu-min: esp-test $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --exit-code 33 $(SEND_KEYS) --expect "NX off" \
		--expect "input: OK" --expect "selftest: OK" -- $(QEMU) $(QEMU_MIN_FLAGS)

# Boot the normal kernel (which runs hello from the initrd first), type a
# line into its echo loop, save the screen once the uptime is printed, and
# check that the framebuffer console drew text.
test-fb: esp $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --screendump $(BUILD)/screen.ppm \
		--send-keys "kernel: ready" "h i ret" --expect "STELA kernel" \
		--expect "hello from user mode" --expect "exited with code 0" --expect "kernel: ready" \
		--expect "uptime:" -- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio
	python3 tools/fb_check.py $(BUILD)/screen.ppm

clippy:
	cargo clippy --release -p stela-boot --target $(UEFI_TARGET) -- -D warnings
	cargo clippy --release $(addprefix -p ,$(KERNEL_CRATES)) --target $(KERNEL_TARGET) -- -D warnings
	cargo clippy --release $(addprefix -p ,$(KERNEL_CRATES)) --target $(KERNEL_TARGET) --features kernel/selftest -- -D warnings
	cargo clippy --release $(addprefix -p ,$(USER_CRATES)) --target $(KERNEL_TARGET) -- -D warnings
	cargo clippy --tests $(addprefix -p ,$(HOST_CRATES)) -- -D warnings
	cargo clippy --lib --tests $(addprefix -p ,$(HOST_LIB_CRATES)) -- -D warnings

fmt:
	cargo fmt --all

clean:
	rm -rf $(BUILD)
