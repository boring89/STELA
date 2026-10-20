# STELA build driver: a thin wrapper around cargo and QEMU.

BUILD       := build
comma       := ,
ESP         := $(BUILD)/esp
ESP_TEST    := $(BUILD)/esp-test
UEFI_TARGET := x86_64-unknown-uefi
KERNEL_TARGET := x86_64-unknown-none
BOOT_EFI    := $(BUILD)/target/$(UEFI_TARGET)/release/stela-boot.efi
KERNEL_ELF  := $(BUILD)/target/$(KERNEL_TARGET)/release/kernel
# User programs packed into the initrd (user/<name>, built for the same
# bare-metal target as the kernel, linked with user/user.ld).
USER_PROGRAMS := hello spin fault ipc_server ipc_client echo fstest proctest init shell windowserver terminal echo_win mouseion viewer
USER_DIR    := $(BUILD)/user
INITRD      := $(BUILD)/initrd.tar
# Programs packed as bundles (docs/mouseion.md): program:Name puts the
# program at Name.app/exec with Name.app/info.txt; the rest sit in the
# initrd's root.
BUNDLES     := terminal:Terminal echo_win:Echo viewer:Viewer
BUNDLED     := $(foreach b,$(BUNDLES),$(firstword $(subst :, ,$(b))))
INITRD_ROOT := $(BUILD)/initrd-root

# musl (MIT license), only to build the Linux test programs; never part of
# the repository. Not downloaded here: the tarball is copied from where it
# already is (68k_HAL's downloads, or MUSL_TARBALL=...), checked against
# the SHA-256 musl publishes for the release, and built in build/musl.
MUSL_VERSION := 1.2.5
MUSL_SHA256 := a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
MUSL_TARBALL ?= $(HOME)/文件/68k_HAL/third_party/downloads/musl/musl-$(MUSL_VERSION).tar.gz
MUSL_DOWNLOAD := $(BUILD)/downloads/musl-$(MUSL_VERSION).tar.gz
MUSL_DIR    := $(BUILD)/musl
MUSL_GCC    := $(MUSL_DIR)/install/bin/musl-gcc
# Linux test programs (linux/tests/<name>.c), static, packed into the
# initrd's linux/ folder.
LINUX_TESTS := hello files memory time tty nosys
LINUX_DIR   := $(USER_DIR)/linux
LINUX_CFLAGS := -static -O2 -no-pie -fno-pie -Wall -Wextra -Werror \
               -Wl,-z,max-page-size=4096 -Wl,--build-id=none

OVMF_CODE   := /usr/share/OVMF/OVMF_CODE_4M.fd
OVMF_VARS   := /usr/share/OVMF/OVMF_VARS_4M.fd
QEMU        := qemu-system-x86_64
QEMU_BASE   := -machine q35 -m 512M -no-reboot -rtc base=utc \
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
HOST_CRATES := abi block bootinfo drivers ehfs ehfs-tools elfload frame libstela toolbox
# Crates with a bare-metal binary whose library part has host unit tests.
HOST_LIB_CRATES := kernel windowserver terminal mouseion viewer
# Crates built for the bare-metal kernel target.
KERNEL_CRATES := abi block bootinfo drivers ehfs frame kernel
# User-space crates (same target).
USER_CRATES := libstela toolbox $(USER_PROGRAMS)

.PHONY: all boot kernel user initrd musl linux-tests assets draft-assets esp esp-test run test test-host test-qemu test-qemu-min test-window \
	host-images test-images test-storage test-shell test-shutdown test-linux test-usb test-panic test-mouseion ehfs-tools test-ehfs-compat clippy fmt clean

all: esp

boot:
	cargo build --release -p stela-boot --target $(UEFI_TARGET)

kernel:
	cargo build --release -p kernel --target $(KERNEL_TARGET)

user:
	cargo build --release $(addprefix -p ,$(USER_PROGRAMS)) --target $(KERNEL_TARGET)
	mkdir -p $(USER_DIR)
	cp $(addprefix $(BUILD)/target/$(KERNEL_TARGET)/release/,$(USER_PROGRAMS)) $(USER_DIR)/

# The initial RAM disk: the user programs in a ustar archive (the Linux
# test programs in its linux/ folder, the bundles as Name.app/ folders),
# with fixed owners and times so that the same programs give the same
# bytes.
initrd: user linux-tests
	rm -rf $(INITRD_ROOT)
	mkdir -p $(INITRD_ROOT)
	cp $(addprefix $(USER_DIR)/,$(filter-out $(BUNDLED),$(USER_PROGRAMS))) $(INITRD_ROOT)/
	cp -r $(LINUX_DIR) $(INITRD_ROOT)/linux
	for b in $(BUNDLES); do p=$${b%%:*}; n=$${b#*:}; mkdir -p $(INITRD_ROOT)/$$n.app; \
		cp $(USER_DIR)/$$p $(INITRD_ROOT)/$$n.app/exec; \
		printf '%s\n%s\n' "$$n" "$$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)" > $(INITRD_ROOT)/$$n.app/info.txt; done
	tar --format=ustar --owner=0 --group=0 --numeric-owner --mtime=@0 --sort=name \
		-cf $(INITRD) -C $(INITRD_ROOT) .

# The icons (assets/README.md): assets/*.ppm and *.pbm to Rust data in
# assets/generated/. draft-assets redraws the DRAFT images (it overwrites
# assets/: not after the real artwork is in).
assets:
	python3 tools/img_to_rs.py assets assets/generated

draft-assets:
	python3 tools/make_draft_assets.py assets
	python3 tools/img_to_rs.py assets assets/generated

musl: $(MUSL_GCC)

$(MUSL_DOWNLOAD):
	mkdir -p $(BUILD)/downloads
	cp $(MUSL_TARBALL) $@

$(MUSL_GCC): $(MUSL_DOWNLOAD)
	echo "$(MUSL_SHA256)  $(MUSL_DOWNLOAD)" | sha256sum -c -
	rm -rf $(MUSL_DIR)
	mkdir -p $(MUSL_DIR)/src
	tar -xzf $(MUSL_DOWNLOAD) -C $(MUSL_DIR)/src --strip-components=1
	cd $(MUSL_DIR)/src && ./configure --prefix=$(abspath $(MUSL_DIR))/install \
		--target=x86_64 --disable-shared CC=gcc AR=ar RANLIB=ranlib > ../configure.log
	$(MAKE) -C $(MUSL_DIR)/src -j$$(nproc) > $(MUSL_DIR)/make.log
	$(MAKE) -C $(MUSL_DIR)/src install > $(MUSL_DIR)/install.log

linux-tests: $(addprefix $(LINUX_DIR)/,$(LINUX_TESTS))

$(LINUX_DIR)/%: linux/tests/%.c $(MUSL_GCC)
	mkdir -p $(LINUX_DIR)
	$(MUSL_GCC) $(LINUX_CFLAGS) -o $@ $<

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

# The panic screen: the self-test kernel with a file `panic-test` in its
# initrd panics on purpose and halts; the screen must show the cracked
# stela (terracotta in the stela's left face; 2560x1600: the 256x256
# stela at 1152,544) on the panic colour.
ESP_PANIC   := $(BUILD)/esp-panic
PANIC_FACE  := 1236,600,16,12
test-panic: esp-test $(BUILD)/OVMF_VARS.fd
	rm -rf $(BUILD)/panic-initrd
	mkdir -p $(ESP_PANIC)/EFI/BOOT $(ESP_PANIC)/STELA
	cp -r $(INITRD_ROOT) $(BUILD)/panic-initrd
	touch $(BUILD)/panic-initrd/panic-test
	tar --format=ustar --owner=0 --group=0 --numeric-owner --mtime=@0 --sort=name \
		-cf $(ESP_PANIC)/STELA/initrd.tar -C $(BUILD)/panic-initrd .
	cp $(ESP_TEST)/EFI/BOOT/BOOTX64.EFI $(ESP_PANIC)/EFI/BOOT/
	cp $(ESP_TEST)/STELA/kernel.elf $(ESP_PANIC)/STELA/
	python3 tools/qemu_test.py --timeout 60 --screendump $(BUILD)/panic.ppm \
		--expect "STELA kernel panic" --expect "the panic screen test" \
		--expect "the panic screen is up" \
		-- $(QEMU) $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP_PANIC) -display none -serial stdio
	python3 tools/fb_check.py $(BUILD)/panic.ppm --expect-rect 0,0,300,300,\#5c1a14 \
		--expect-rect $(PANIC_FACE),\#c8763c

$(BUILD)/OVMF_VARS.fd:
	mkdir -p $(BUILD)
	cp $(OVMF_VARS) $@

run: esp $(BUILD)/OVMF_VARS.fd
	$(QEMU) $(QEMU_FLAGS) -serial stdio

test: test-host test-qemu test-qemu-min test-window test-storage test-shell test-shutdown test-linux test-usb test-panic test-mouseion

# Images the host tests read (FAT made by mtools, GPT made by sgdisk).
host-images:
	python3 tools/make_test_images.py host $(BUILD)/images

test-host: initrd host-images
	python3 tools/img_to_rs.py assets $(BUILD)/assets-check > /dev/null
	diff -r $(BUILD)/assets-check assets/generated
	cargo test $(addprefix -p ,$(HOST_CRATES))
	cargo test --lib $(addprefix -p ,$(HOST_LIB_CRATES))

# The keyboard self test prints "input: waiting"; the script then types
# "stela" and Enter through QMP send-key.
SEND_KEYS := --send-keys "input: waiting" "s t e l a ret" --mouse "mouse: waiting" 40 30 left

test-qemu: esp-test $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --exit-code 33 $(SEND_KEYS) --log $(BUILD)/test-qemu.log \
		--expect "STELA boot loader" --expect "time: RTC " \
		--expect "STELA kernel" --expect "input: OK" --expect "mouse: OK" \
		--expect "user: hello from built-in user code" --expect "hello from user mode" \
		--expect "hello from a second thread" --expect "fault: reading address 0" \
		--expect "spin: " --expect "ipc_client: OK" --expect "proctest: OK" \
		--expect "linux: hello from raw system calls" --expect "selftest: OK" \
		-- $(QEMU) $(QEMU_TEST_FLAGS)

# The minimal machine also gets the AHCI test disk (a copy of its own):
# AHCI, GPT, FAT, eHFS and fstest on a Core 2 without NX in 256 MiB.
test-qemu-min: esp-test test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-min.img
	python3 tools/qemu_test.py --exit-code 33 $(SEND_KEYS) --expect "NX off" \
		--expect "vfs: mounted Boot (FAT, ro" --expect "input: OK" --expect "fstest: OK" \
		--expect "selftest: OK" -- $(QEMU) $(QEMU_MIN_FLAGS) \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-min.img \
		-device ide-hd,drive=sata1,bus=ide.1
	$(EHFS_TOOLS)/fsck.ehfs --offset 83968 $(IMAGES)/ahci-min.img

# The desktop: boot the normal kernel (init starts the window server and
# the terminal, the terminal the shell), type a command, start echo_win
# from the shell and type a line into it, then save the screen and check
# the desktop, the menu bar, the terminal's (inactive) title bar and its
# text, echo_win's (active) title bar and its contents. The geometry is
# that of the 2560x1600 OVMF mode: 16x32 font, menu bar 40, title bar 38;
# the terminal's content at 40,108 (1288x808), echo_win's at 1844,140
# (656x448).
WINDOW_CHECKS := --expect-rect 1000,1100,300,300,\#5b7b86 --expect-rect 1300,8,400,20,\#f2e8d5 \
	--expect-rect 300,72,100,10,\#d8cdb8 --expect-text 44,112,1280,800,\#f2e8d5,4 \
	--expect-rect 1864,104,80,8,\#c8763c --expect-text 1848,144,648,440,\#f2e8d5,3
test-window: esp $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --timeout 90 --screendump $(BUILD)/screen.ppm --log $(BUILD)/test-window.log \
		--type "STELA shell" "uptime\n" --type "uptime:" "run Echo.app\n" \
		--type "> run Echo.app" "hello\n" \
		--expect "windowserver: " --expect "STELA shell" --expect "uptime:" \
		--expect "echo_win: hello" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio
	python3 tools/fb_check.py $(BUILD)/screen.ppm $(WINDOW_CHECKS)

# The shell on the normal kernel with the AHCI test disk (a copy of its
# own): commands typed through QMP, their answers checked on the serial
# console. Each command is typed when the previous one's line appears.
test-shell: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-shell.img
	python3 tools/qemu_test.py --timeout 120 \
		--type "STELA shell" "vols\n" \
		--type "Initrd" "ls Data:/\n" \
		--type "item(s)" "mkdir t\n" \
		--type "> mkdir t" "cd t\n" \
		--type "> cd t" "pwd\n" \
		--type "Data:/t" "cat nothing\n" \
		--type "cat: " "run hello\n" \
		--type "hello exited with code" "ps\n" \
		--type "ended)" "mem\n" \
		--type "memory: " "uptime\n" \
		--expect "STELA shell" --expect "Data       eHFS" --expect "Initrd     initrd" \
		--expect "welcome.txt" --expect "item(s)" --expect "> mkdir t" --expect "> cd t" \
		--expect "Data:/t" --expect "cat: Data:/t/nothing: not found" \
		--expect "hello from user mode" --expect "hello exited with code 0" \
		--expect "shell" --expect "ended)" --expect "memory: " --expect "uptime: " \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-shell.img \
		-device ide-hd,drive=sata1,bus=ide.1

# Power: `shutdown` in the shell flushes the volumes and turns the machine
# off through ACPI, so QEMU exits by itself with status 0 and the host's
# fsck sees a cleanly unmounted Data volume; `reboot` resets the machine,
# which -no-reboot turns into an exit with status 0 too.
test-shutdown: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-power.img
	python3 tools/qemu_test.py --exit-code 0 --type "STELA shell" "mkdir before-shutdown\n" \
		--type "> mkdir before-shutdown" "shutdown\n" \
		--expect "STELA shell" --expect "kernel: volumes unmounted" --expect "power: turning off" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-power.img \
		-device ide-hd,drive=sata1,bus=ide.1
	$(EHFS_TOOLS)/fsck.ehfs --offset 83968 $(IMAGES)/ahci-power.img | tee $(BUILD)/fsck-power.txt
	grep -q "unmounted cleanly" $(BUILD)/fsck-power.txt
	$(EHFS_TOOLS)/ehfs-ls --offset 83968 $(IMAGES)/ahci-power.img / | grep -q before-shutdown
	python3 tools/qemu_test.py --exit-code 0 --type "STELA shell" "reboot\n" \
		--expect "STELA shell" --expect "kernel: volumes unmounted" --expect "power: restarting" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-power.img \
		-device ide-hd,drive=sata1,bus=ide.1
	$(EHFS_TOOLS)/fsck.ehfs --offset 83968 $(IMAGES)/ahci-power.img | grep -q "unmounted cleanly"

# The Linux personality: the shell runs the static musl test programs
# from Initrd:/linux on the normal kernel (with the AHCI test disk, so the
# working directory is Data:/), and a line is typed for linux/tty.
test-linux: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-linux.img
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-linux.log \
		--type "STELA shell" "run linux/hello a b\n" \
		--type "hello exited with code" "run linux/files\n" \
		--type "files exited with code" "run linux/memory\n" \
		--type "memory exited with code" "run linux/time\n" \
		--type "time exited with code" "run linux/nosys\n" \
		--type "nosys exited with code" "run linux/tty\n" \
		--type "tty: type a line" "stela rocks\n" \
		--expect "hello from linux: 3 args" --expect "hello: uname STELA" \
		--expect "hello: thread-local value 7" --expect "hello exited with code 3" \
		--expect "files: OK" --expect "files exited with code 0" \
		--expect "memory: OK" --expect "memory exited with code 0" \
		--expect "time: OK" --expect "time exited with code 0" \
		--expect "nosys: OK" --expect "nosys exited with code 0" \
		--expect "tty: you typed: stela rocks" --expect "tty exited with code 0" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-linux.img \
		-device ide-hd,drive=sata1,bus=ide.1

# USB: the normal kernel without PS/2 (i8042=off) and with an xHCI
# controller: a keyboard, a mouse and a hub with a keyboard behind it.
# Once the hub's keyboard is enumerated, a command typed (QMP send-key
# goes to a USB keyboard) must reach the shell in the terminal; mouse
# motion sent to the mouse must arrive as reports; then hot plugging: the
# mouse and the hub's keyboard are removed and added back (on another hub
# port).
USB_FLAGS := -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0,id=kbd \
	-device usb-mouse,bus=xhci.0,id=mouse -device usb-hub,bus=xhci.0,id=hub \
	-device usb-kbd,bus=xhci.0,port=3.1,id=kbd2
test-usb: esp $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --timeout 90 --log $(BUILD)/test-usb.log \
		--type "usb: 7: hub" "uptime\n" \
		--qmp "uptime:" input-send-event '{"events":[{"type":"rel","data":{"axis":"x","value":30}}]}' \
		--qmp "usb: 6: first report" device_del '{"id":"mouse"}' \
		--qmp "usb: 6: removed" device_add '{"driver":"usb-mouse","bus":"xhci.0","id":"mouse2"}' \
		--qmp "usb: 8: mouse" device_del '{"id":"kbd2"}' \
		--qmp "usb: 7.1: removed" device_add '{"driver":"usb-kbd","bus":"xhci.0","port":"3.2","id":"kbd3"}' \
		--expect "usb: xhci0" --expect "usb: 7: hub" --expect "uptime:" \
		--expect "usb: 6: first report" --expect "usb: 6: removed" --expect "usb: 8: mouse" \
		--expect "usb: 7.1: removed" --expect "usb: 7.2: keyboard" \
		-- $(QEMU) $(subst -machine q35,-machine q35$(comma)i8042=off,$(QEMU_FLAGS)) \
		-display none -serial stdio $(USB_FLAGS)

# Mouseion (2560x1600, 16x32 font, 64-pixel icons in 160x112 cells; the
# desktop's volume icons on the right: Boot, Data, Initrd, Tmp): double
# click Data, then welcome.txt in Data's window (the viewer opens it),
# then Initrd, then Terminal.app in Initrd's window (a second terminal
# starts). The screen shows the volume icons and the new terminal active.
MOUSEION_CHECKS := --expect-rect 2445,82,40,12,\#c8763c --expect-rect 2000,1300,300,200,\#5b7b86 \
	--expect-rect 300,192,100,10,\#c8763c
test-mouseion: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci-pristine.img $(IMAGES)/ahci-mouseion.img
	python3 tools/qemu_test.py --timeout 120 --screendump $(BUILD)/mouseion.ppm --log $(BUILD)/test-mouseion.log \
		--click "mouseion: started Terminal" 2464 204 2 \
		--click "mouseion: opened Data:/" 256 232 2 \
		--click "viewer: Data:/welcome.txt" 2464 316 2 \
		--click "mouseion: opened Initrd:/" 456 272 2 \
		--expect "mouseion: ready, 4 volume(s)" --expect "mouseion: opened Data:/" \
		--expect "viewer: Data:/welcome.txt" --expect "mouseion: opened Initrd:/" \
		--expect "mouseion: started Terminal" --expect "STELA shell" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-mouseion.img \
		-device ide-hd,drive=sata1,bus=ide.1
	python3 tools/fb_check.py $(BUILD)/mouseion.ppm $(MOUSEION_CHECKS)

# The eHFS host tools (tools/ehfs), installed under their usual names.
EHFS_TOOLS  := $(BUILD)/host/ehfs
HOST_TARGET_DIR := $(BUILD)/target/release

ehfs-tools:
	cargo build --release -p ehfs-tools
	mkdir -p $(EHFS_TOOLS)
	cp $(HOST_TARGET_DIR)/mkfs-ehfs $(EHFS_TOOLS)/mkfs.ehfs
	cp $(HOST_TARGET_DIR)/fsck-ehfs $(EHFS_TOOLS)/fsck.ehfs
	cp $(HOST_TARGET_DIR)/ehfs-cp $(HOST_TARGET_DIR)/ehfs-ls $(EHFS_TOOLS)/

# Local only (not in make test or CI): cross-checks with 68k_HAL's eHFS
# tools, when they are built: its fsck on a STELA image, and STELA's fsck
# and ehfs-ls on a 68k_HAL image. Only images under build/ are used.
HAL_EHFS    := $(HOME)/文件/68k_HAL/build/host/ehfs
test-ehfs-compat: ehfs-tools
	python3 tools/ehfs_compat.py $(EHFS_TOOLS) $(HAL_EHFS) $(BUILD)/images/compat

# Disk images for the storage test, made fresh for every run (the test
# writes them): an AHCI disk (512-byte sectors) and an NVMe namespace
# (4096-byte blocks), 64 MiB each, starting with the test-disk magic.
IMAGES      := $(BUILD)/images
STORAGE_FLAGS = -drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci.img \
	-device ide-hd,drive=sata1,bus=ide.1 \
	-drive if=none,id=nvm1,format=raw,file=$(IMAGES)/nvme.img \
	-device nvme,serial=stela-nvme,drive=nvm1,logical_block_size=4096,physical_block_size=4096

test-images: ehfs-tools
	python3 tools/make_test_images.py storage $(IMAGES) $(EHFS_TOOLS)
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-pristine.img

# Boots the self-test kernel twice with the test disks: the second boot
# must find what the first one wrote (raw blocks and eHFS files); then the
# host's fsck.ehfs checks both eHFS volumes.
STORAGE_EXPECT := --expect "vfs: mounted Boot (FAT, ro, 39 MiB), Data (eHFS, rw, 86 MiB), Archive (eHFS, rw, 62 MiB)"
test-storage: esp-test test-images $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --exit-code 33 $(SEND_KEYS) $(STORAGE_EXPECT) \
		--expect "storage: ahci0p1 boot 1" --expect "storage: nvme0n1 boot 1" \
		--expect "fstest: boot 1" --expect "fstest: OK" \
		--expect "selftest: OK" -- $(QEMU) $(QEMU_TEST_FLAGS) $(STORAGE_FLAGS)
	python3 tools/qemu_test.py --exit-code 33 $(SEND_KEYS) $(STORAGE_EXPECT) \
		--expect "storage: ahci0p1 boot 2" --expect "storage: nvme0n1 boot 2" \
		--expect "fstest: boot 2" --expect "fstest: OK" \
		--expect "selftest: OK" -- $(QEMU) $(QEMU_TEST_FLAGS) $(STORAGE_FLAGS)
	$(EHFS_TOOLS)/fsck.ehfs --offset 83968 $(IMAGES)/ahci.img
	$(EHFS_TOOLS)/fsck.ehfs --offset 2048 $(IMAGES)/nvme.img
	$(EHFS_TOOLS)/ehfs-ls --offset 83968 $(IMAGES)/ahci.img /

clippy:
	cargo clippy --release -p stela-boot --target $(UEFI_TARGET) -- -D warnings
	cargo clippy --release $(addprefix -p ,$(KERNEL_CRATES)) --target $(KERNEL_TARGET) -- -D warnings
	cargo clippy --release $(addprefix -p ,$(KERNEL_CRATES)) --target $(KERNEL_TARGET) --features kernel/selftest -- -D warnings
	cargo clippy --release $(addprefix -p ,$(USER_CRATES)) --target $(KERNEL_TARGET) -- -D warnings
	cargo clippy --tests $(addprefix -p ,$(HOST_CRATES)) -- -D warnings
	cargo clippy --bins -p ehfs-tools -- -D warnings
	cargo clippy --lib --tests $(addprefix -p ,$(HOST_LIB_CRATES)) -- -D warnings

fmt:
	cargo fmt --all

clean:
	rm -rf $(BUILD)
