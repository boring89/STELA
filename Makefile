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
USER_PROGRAMS := hello spin fault ipc_server ipc_client keyecho fstest proctest init shell windowserver terminal echo_win mouseion viewer classic ifconfig ping fetch tone hwinfo bench kalamos monitor periplus stress
USER_DIR    := $(BUILD)/user
# The command-line tools (P11, docs/apps.md): one package, user/bin, with
# a program each in src/bin/; they go to Initrd:/bin/, on the shell's path.
BIN_TOOLS   := echo cat grep wc head tail sort uniq find du df date sleep touch stat hexdump tar true false
INITRD      := $(BUILD)/initrd.tar
# Programs packed as bundles (docs/mouseion.md): program:Name puts the
# program at Name.app/exec with Name.app/info.txt; the rest sit in the
# initrd's root.
BUNDLES     := terminal:Terminal echo_win:Echo viewer:Viewer classic:Classic kalamos:Kalamos monitor:Monitor periplus:Periplus
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
LINUX_TESTS := hello files memory time tty nosys net poll pipe threads signals procs misc
LINUX_DIR   := $(USER_DIR)/linux
LINUX_CFLAGS := -static -O2 -no-pie -fno-pie -Wall -Wextra -Werror \
               -Wl,-z,max-page-size=4096 -Wl,--build-id=none
# A Rust std program (P12, linux/rust/hello-std): x86_64-unknown-linux-musl
# (`rustup target add x86_64-unknown-linux-musl`, in rust-toolchain.toml),
# static and not position independent (STELA loads ET_EXEC only).
LINUX_RUSTFLAGS := -C relocation-model=static -C target-feature=+crt-static \
               -C link-arg=-Wl,-z,max-page-size=4096 -C link-arg=-Wl,--build-id=none

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
HOST_CRATES := abi block bootinfo drivers ehfs ehfs-tools elfload frame libstela net toolbox
# Crates with a bare-metal binary whose library part has host unit tests.
HOST_LIB_CRATES := kernel windowserver terminal mouseion viewer classic bin periplus
# Crates built for the bare-metal kernel target.
KERNEL_CRATES := abi block bootinfo drivers ehfs frame kernel net
# User-space crates (same target).
USER_CRATES := libstela toolbox bin $(USER_PROGRAMS)

.PHONY: all boot kernel user initrd musl linux-tests assets draft-assets esp esp-test run bench test test-host test-fuzz test-qemu test-qemu-smp test-qemu-min test-textmode test-window \
	disk-image linux-rust test-ports test-disk-image classic test-classic test-net test-net-cards test-sound host-images test-images test-storage test-shell test-shutdown test-linux test-usb test-panic test-mouseion test-editor test-pipe test-browser test-stress ehfs-tools test-ehfs-compat clippy fmt clean

all: esp

boot:
	cargo build --release -p stela-boot --target $(UEFI_TARGET)

kernel:
	cargo build --release -p kernel --target $(KERNEL_TARGET)

user:
	cargo build --release $(addprefix -p ,$(USER_PROGRAMS)) -p bin --target $(KERNEL_TARGET)
	mkdir -p $(USER_DIR)/bin
	cp $(addprefix $(BUILD)/target/$(KERNEL_TARGET)/release/,$(USER_PROGRAMS)) $(USER_DIR)/
	cp $(addprefix $(BUILD)/target/$(KERNEL_TARGET)/release/,$(BIN_TOOLS)) $(USER_DIR)/bin/

# The initial RAM disk: the user programs in a ustar archive (the Linux
# test programs in its linux/ folder, the bundles as Name.app/ folders),
# with fixed owners and times so that the same programs give the same
# bytes.
initrd: user linux-tests
	rm -rf $(INITRD_ROOT)
	mkdir -p $(INITRD_ROOT)
	cp $(addprefix $(USER_DIR)/,$(filter-out $(BUNDLED),$(USER_PROGRAMS))) $(INITRD_ROOT)/
	cp -r $(LINUX_DIR) $(INITRD_ROOT)/linux
	cp -r $(USER_DIR)/bin $(INITRD_ROOT)/bin
	mkdir -p $(INITRD_ROOT)/fonts
	cp third_party/fusion-pixel/fusion-pixel-12-zh_tw.bin $(INITRD_ROOT)/fonts/
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

linux-tests: $(addprefix $(LINUX_DIR)/,$(LINUX_TESTS)) linux-rust

linux-rust:
	RUSTFLAGS="$(LINUX_RUSTFLAGS)" cargo build --release --target x86_64-unknown-linux-musl \
		--manifest-path linux/rust/hello-std/Cargo.toml --target-dir $(BUILD)/target/linux-rust
	mkdir -p $(LINUX_DIR)
	cp $(BUILD)/target/linux-rust/x86_64-unknown-linux-musl/release/hello-std $(LINUX_DIR)/

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
	printf '# boot options for the self tests (docs/p520.md)\nselftest\n' > $(ESP_TEST)/STELA/boot.cfg

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
		--expect-rect $(PANIC_FACE),\#c8763c --expect-text 32,1216,1600,352,\#5c1a14,8

$(BUILD)/OVMF_VARS.fd:
	mkdir -p $(BUILD)
	cp $(OVMF_VARS) $@

run: esp $(BUILD)/OVMF_VARS.fd
	$(QEMU) $(QEMU_FLAGS) -serial stdio

test: test-host test-fuzz test-qemu test-qemu-smp test-qemu-min test-textmode test-disk-image test-net test-sound test-window test-storage test-shell test-pipe test-shutdown test-linux test-usb test-panic test-mouseion test-editor test-browser

# Images the host tests read (FAT made by mtools, GPT made by sgdisk).
host-images:
	python3 tools/make_test_images.py host $(BUILD)/images

test-host: initrd host-images
	python3 tools/img_to_rs.py assets $(BUILD)/assets-check > /dev/null
	diff -r $(BUILD)/assets-check assets/generated
	cargo test $(addprefix -p ,$(HOST_CRATES))
	cargo test --lib $(addprefix -p ,$(HOST_LIB_CRATES))

# Fuzzing on the host (P11 K45, docs/testing.md §3): the parsers of
# disks (eHFS, FAT, GPT, MBR), the initrd, network frames (and two stacks
# talking over a lossy wire), HTTP, DNS, HTML, boot.cfg/classic.cfg and
# paths get changed valid inputs; a panic or a hang fails, the input is
# saved to build/fuzz-<target>.bin. At most 60 s.
test-fuzz: host-images
	cargo build --profile fuzz -p fuzz
	$(BUILD)/target/fuzz/fuzz $(BUILD)

# The keyboard self test prints "input: waiting"; the script then types
# "stela" and Enter through QMP send-key.
SEND_KEYS := --send-keys "input: waiting" "s t e l a ret" --mouse "mouse: waiting" 40 30 left

test-qemu: esp-test $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --exit-code 33 $(SEND_KEYS) --log $(BUILD)/test-qemu.log \
		--expect "STELA boot loader" --expect "time: RTC " \
		--expect "STELA kernel" --expect "input: OK" --expect "mouse: OK" \
		--expect "user: hello from built-in user code" --expect "hello from user mode" \
		--expect "hello from a second thread" --expect "hello: thread returned 42" \
		--expect "fault: reading address 0" \
		--expect "spin: " --expect "ipc_client: OK" --expect "net: raw frames work" --expect "proctest: OK" \
		--expect "linux: hello from raw system calls" --expect "selftest: OK" \
		-- $(QEMU) $(QEMU_TEST_FLAGS)

# SMP (docs/smp.md): the whole self test on four processors; the "smp"
# test checks each one's timer, wake-ups across processors, a spinning
# process per processor and killing them from the boot processor.
test-qemu-smp: esp-test $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --exit-code 33 $(SEND_KEYS) --log $(BUILD)/test-qemu-smp.log \
		--expect "smp: 4 processor(s) online" --expect "input: OK" --expect "proctest: OK" \
		--expect "smp: 4 processors: ticks, wake both ways" --expect "selftest: OK" \
		-- $(QEMU) $(QEMU_TEST_FLAGS) -smp 4

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

# A disk image for a real machine (docs/p520.md): GPT, an ESP with the
# loader, kernel, initrd and a boot.cfg (all options commented out), and
# an eHFS Data volume of DATA_MIB MiB. Only the file build/stela.img is
# written; copying it to a USB stick is the user's own step. When the
# user's Mac ROM and System disk are there (MAC_ROM, MAC_DISK; local
# only, P11), they are copied into Data:/Classic/ for Classic.app; set
# MAC_ROM= to leave them out.
DATA_MIB    ?= 4096
DISK_IMAGE  := $(BUILD)/stela.img
DISK_ESP    := $(BUILD)/disk-esp
# The Data partition's first sector (65 MiB; tools/make_disk_image.py),
# for ehfs-cp --offset.
DATA_SECTOR := 133120
disk-image: esp ehfs-tools
	rm -rf $(DISK_ESP)
	cp -r $(ESP) $(DISK_ESP)
	cp tools/boot.cfg $(DISK_ESP)/STELA/boot.cfg
	python3 tools/make_disk_image.py $(DISK_IMAGE) $(DISK_ESP) $(EHFS_TOOLS) $(DATA_MIB)
	@if [ -f "$(MAC_ROM)" ] && [ -f "$(MAC_DISK)" ]; then \
		$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(DISK_IMAGE) mkdir /Classic && \
		$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(DISK_IMAGE) in "$(MAC_ROM)" /Classic/ROM && \
		$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(DISK_IMAGE) in "$(MAC_DISK)" /Classic/System.img && \
		echo "disk-image: copied your Mac ROM and System disk into Data:/Classic/ (this image is yours only: do not share it)"; \
	else \
		echo "disk-image: no Mac ROM or System disk at MAC_ROM=$(MAC_ROM) MAC_DISK=$(MAC_DISK); Data:/Classic/ left out (Classic.app will say what is missing)"; \
	fi

# The disk image as the only disk (AHCI, the firmware boots from it): the
# desktop comes up, a folder made in the shell survives a shutdown (the
# host's fsck and ehfs-ls check the Data volume) and is there on the
# second boot. A small Data volume keeps the test quick.
TEST_DISK_IMAGE = $(IMAGES)/stela-test.img
DISK_IMAGE_FLAGS = -drive if=none,id=disk0,format=raw,file=$(TEST_DISK_IMAGE) \
	-device ide-hd,drive=disk0,bus=ide.0,bootindex=0
test-disk-image: esp ehfs-tools $(BUILD)/OVMF_VARS.fd
	rm -rf $(DISK_ESP)
	cp -r $(ESP) $(DISK_ESP)
	cp tools/boot.cfg $(DISK_ESP)/STELA/boot.cfg
	mkdir -p $(IMAGES)
	python3 tools/make_disk_image.py $(TEST_DISK_IMAGE) $(DISK_ESP) $(EHFS_TOOLS) 128
	python3 tools/qemu_test.py --exit-code 0 --timeout 90 \
		--type "STELA shell" "mkdir from-the-disk-image\n" \
		--type "> mkdir from-the-disk-image" "shutdown\n" \
		--expect "windowserver: " --expect "mouseion: ready" --expect "STELA shell" \
		--expect "power: turning off" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio $(DISK_IMAGE_FLAGS)
	$(EHFS_TOOLS)/fsck.ehfs --offset 133120 $(TEST_DISK_IMAGE) | grep -q "unmounted cleanly"
	$(EHFS_TOOLS)/ehfs-ls --offset 133120 $(TEST_DISK_IMAGE) / | grep -q from-the-disk-image
	python3 tools/qemu_test.py --timeout 90 --type "STELA shell" "ls Data:/\n" \
		--expect "mounted Boot (FAT, ro" --expect "STELA shell" \
		--expect "from-the-disk-image" --expect "Welcome.txt" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio $(DISK_IMAGE_FLAGS)

# The Classic environment (docs/classic.md), local only (not in make test
# or CI: it needs the user's own Mac IIci ROM and System disk, which are
# only read; copies go into a disk image under build/). The disk image of
# make disk-image with the two files in Data:/Classic/; the shell starts
# Classic.app; the Mac must reach the Finder (serial "classic: finder"),
# then the screen is saved and the Mac's desktop pattern and menu bar
# are checked inside the window. A second boot with an HDA card runs
# Classic.app --test-beep: the Mac's SysBeep must reach the WAV file QEMU
# records. Then Data:/Classic/classic.cfg asks for 32 MiB and the NuBus
# card at 800x600 (1 bit): the window must be that big. A fourth boot, from
# a copy of the image made before the first,
# runs Classic.app --test-fs: the Finder, driven from
# the keyboard, opens the Data volume (the File System Manager plug-in) and
# makes a new folder there, which the shell, the host's ehfs-ls and fsck
# then see.
MAC_ROM     ?= $(HOME)/mac/mac_rom_archive_-_as_of_8-19-2011/368CADFE - Mac IIci.ROM
MAC_DISK    ?= $(HOME)/mac/System7_5_3.img
CLASSIC_IMAGE = $(IMAGES)/classic-data.img
CLASSIC_FS_IMAGE = $(IMAGES)/classic-fs.img
# The power test (P11 K26) starts from a System disk that was shut down
# properly: after an improper shut down, System 7.5 greets the user with
# an alert that would catch the test's clicks.
CLASSIC_POWER_IMAGE = $(IMAGES)/classic-power.img
# A blank HFS volume for disk2 (P11 K25), made by hfsutils' hformat
# (HOME points into build/: hfsutils keeps its current volume there).
CLASSIC_APPS = $(IMAGES)/classic-apps.img
CLASSIC_WAV := $(BUILD)/classic.wav
# The Classic window lands at 70,138 (content, 640x480): the Mac's menu
# bar (white, 20 rows) on top, the desktop pattern below it.
CLASSIC_CHECKS := --expect-rect 610,146,400,20,\#ffffff \
	--expect-pattern 170,262,800,600,\#000000,\#ffffff,2
# The first boot has no classic.cfg: on QEMU's 2560x1600 screen the
# window is doubled (P11 K24, content at 70,138; the pattern in 2x2
# squares). With the NuBus card at 800x600 and scale=1 (P11): the menu bar
# and the pattern beyond the built-in 640x480.
CLASSIC_CARD_CHECKS := --expect-rect 600,142,250,10,\#ffffff \
	--expect-pattern 400,640,200,80,\#000000,\#ffffff
classic: user

test-classic: esp ehfs-tools $(BUILD)/OVMF_VARS.fd
	test -f "$(MAC_ROM)" && test -f "$(MAC_DISK)"
	command -v hformat >/dev/null || { echo "make test-classic needs hfsutils (hformat)"; exit 1; }
	rm -rf $(DISK_ESP)
	cp -r $(ESP) $(DISK_ESP)
	cp tools/boot.cfg $(DISK_ESP)/STELA/boot.cfg
	mkdir -p $(IMAGES)
	python3 tools/make_disk_image.py $(CLASSIC_IMAGE) $(DISK_ESP) $(EHFS_TOOLS) 128
	$(EHFS_TOOLS)/ehfs-cp --offset 133120 $(CLASSIC_IMAGE) mkdir /Classic
	$(EHFS_TOOLS)/ehfs-cp --offset 133120 $(CLASSIC_IMAGE) in "$(MAC_ROM)" /Classic/ROM
	$(EHFS_TOOLS)/ehfs-cp --offset 133120 $(CLASSIC_IMAGE) in "$(MAC_DISK)" /Classic/System.img
	cp $(CLASSIC_IMAGE) $(CLASSIC_FS_IMAGE)
	cp $(CLASSIC_IMAGE) $(CLASSIC_POWER_IMAGE)
	python3 tools/qemu_test.py --timeout 600 --screendump $(BUILD)/classic.ppm --log $(BUILD)/test-classic.log \
		--type "STELA shell" "run Classic.app\n" \
		--expect "STELA shell" --expect "classic: started" --expect "classic: screen 640x480 (scale 2)" \
		--expect "classic: finder desktop shown" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio \
		-drive if=none,id=disk0,format=raw,file=$(CLASSIC_IMAGE) \
		-device ide-hd,drive=disk0,bus=ide.0,bootindex=0
	python3 tools/fb_check.py $(BUILD)/classic.ppm $(CLASSIC_CHECKS)
	rm -f $(CLASSIC_WAV)
	python3 tools/qemu_test.py --exit-code 0 --timeout 600 --log $(BUILD)/test-classic-sound.log \
		--type "STELA shell" "run Classic.app --test-beep\n" --type "Classic exited" "shutdown\n" \
		--expect "classic: sound on" --expect "classic: beep (sent)" --expect "classic: test done" \
		--expect "power: turning off" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio \
		-drive if=none,id=disk0,format=raw,file=$(CLASSIC_IMAGE) \
		-device ide-hd,drive=disk0,bus=ide.0,bootindex=0 \
		-audiodev wav,id=a0,path=$(CLASSIC_WAV) -device intel-hda -device hda-duplex,audiodev=a0
	python3 tools/check_wav.py $(CLASSIC_WAV) 0.05 0
	printf '# make test-classic\nram=32\nresolution=800x600\ndepth=1\nscale=1\ndisk2=Applications.img,ro\n' > $(BUILD)/classic.cfg
	$(EHFS_TOOLS)/ehfs-cp --offset 133120 $(CLASSIC_IMAGE) in $(BUILD)/classic.cfg /Classic/classic.cfg
	rm -f $(CLASSIC_APPS)
	truncate -s 4M $(CLASSIC_APPS)
	HOME=$(abspath $(IMAGES)) hformat -l Applications $(CLASSIC_APPS)
	$(EHFS_TOOLS)/ehfs-cp --offset 133120 $(CLASSIC_IMAGE) in $(CLASSIC_APPS) /Classic/Applications.img
	python3 tools/qemu_test.py --timeout 600 --screendump $(BUILD)/classic-card.ppm --log $(BUILD)/test-classic-card.log \
		--type "STELA shell" "run Classic.app\n" \
		--expect "classic: disk2 Applications.img (4096 KiB, read only)" \
		--expect "classic: started (ROM 368CADFE, 32 MiB" \
		--expect "classic: screen 800x600 (scale 1)" \
		--expect "classic: volume \"Applications\" (drive 9)" --expect "classic: finder desktop shown" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio \
		-drive if=none,id=disk0,format=raw,file=$(CLASSIC_IMAGE) \
		-device ide-hd,drive=disk0,bus=ide.0,bootindex=0
	python3 tools/fb_check.py $(BUILD)/classic-card.ppm $(CLASSIC_CARD_CHECKS)
	python3 tools/qemu_test.py --exit-code 0 --timeout 600 --log $(BUILD)/test-classic-fs.log \
		--type "STELA shell" "run Classic.app --test-fs\n" \
		--type "Classic exited" "ls Data:/\n" --type "untitled folder" "shutdown\n" \
		--expect "classic: the Data volume is on the Mac's desktop" \
		--expect "classic: fs: created untitled folder" --expect "classic: fs test done" \
		--expect "untitled folder" --expect "power: turning off" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio \
		-drive if=none,id=disk0,format=raw,file=$(CLASSIC_FS_IMAGE) \
		-device ide-hd,drive=disk0,bus=ide.0,bootindex=0
	$(EHFS_TOOLS)/ehfs-ls --offset 133120 $(CLASSIC_FS_IMAGE) / | grep -q "untitled folder"
	$(EHFS_TOOLS)/fsck.ehfs --offset 133120 $(CLASSIC_FS_IMAGE) | grep -q "unmounted cleanly"
	python3 tools/qemu_test.py --exit-code 0 --timeout 600 --log $(BUILD)/test-classic-power.log \
		--type "STELA shell" "run Classic.app --test-power\n" --type "Classic exited" "shutdown\n" \
		--expect "classic: finder desktop shown" --expect "classic: restarting" \
		--expect "classic: finder desktop shown" --expect "classic: the Mac was shut down" \
		--expect "power: turning off" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio \
		-drive if=none,id=disk0,format=raw,file=$(CLASSIC_POWER_IMAGE) \
		-device ide-hd,drive=disk0,bus=ide.0,bootindex=0
	rm -f $(BUILD)/classic-system.img
	$(EHFS_TOOLS)/ehfs-cp --offset 133120 $(CLASSIC_POWER_IMAGE) out /Classic/System.img $(BUILD)/classic-system.img
	python3 tools/hfs_check.py $(BUILD)/classic-system.img
	rm -f $(BUILD)/classic-system.img
	$(EHFS_TOOLS)/fsck.ehfs --offset 133120 $(CLASSIC_POWER_IMAGE) | grep -q "unmounted cleanly"

# Networking (docs/net.md): QEMU user networking (slirp: gateway and host
# 10.0.2.2, DNS 10.0.2.3, us 10.0.2.15) with each of the three cards; an
# HTTP server on the host serves build/www. The shell's ifconfig (DHCP),
# ping to the gateway, fetch of a file over TCP and the Linux program
# linux/net (BSD sockets) must work; DNS (ifconfig resolve) only when the
# host itself resolves names (tools/with_http.py sets HOST_DNS).
NET_PORT    := 8642
NET_CARDS   := e1000 e1000e virtio-net-pci
NET_DNS_TYPE   = $(if $(HOST_DNS),--type "ping exited" "ifconfig resolve example.com\n",)
NET_DNS_EXPECT = $(if $(HOST_DNS),--expect "example.com has address",)
NET_FETCH_AFTER = $(if $(HOST_DNS),ifconfig exited,ping exited)
NET_LINUX_NAME  = $(if $(HOST_DNS), example.com,)
NET_LINUX_EXPECT = $(if $(HOST_DNS),--expect "net: resolved example.com to",)
test-net: esp $(BUILD)/OVMF_VARS.fd
	mkdir -p $(BUILD)/www
	printf 'hello from the host over TCP\n' > $(BUILD)/www/hello.txt
	python3 tools/with_http.py $(BUILD)/www $(NET_PORT) -- $(MAKE) --no-print-directory test-net-cards

# Measurements (P11, docs/bench.md): the normal kernel with the AHCI test
# disk and user networking, no KVM; the shell runs bench (memory on the
# idle desktop, system calls, thread switches, eHFS 16 MiB, TCP 4 MiB
# from the host). bench_report.py turns the timed log into a table in
# build/bench.md; the numbers worth keeping go to docs/bench.md by hand.
bench: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-bench.img
	mkdir -p $(BUILD)/www
	head -c 4194304 /dev/zero > $(BUILD)/www/bench.bin
	python3 tools/with_http.py $(BUILD)/www $(NET_PORT) -- \
		python3 tools/qemu_test.py --timeout 300 --times --log $(BUILD)/bench.log \
		--type "STELA shell" "run bench http://10.0.2.2:$(NET_PORT)/bench.bin\n" \
		--expect "mouseion: ready" --expect "bench: done" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-bench.img \
		-device ide-hd,drive=sata1,bus=ide.1
	python3 tools/bench_report.py $(BUILD)/bench.log $(ESP)/STELA/kernel.elf > $(BUILD)/bench.md
	cat $(BUILD)/bench.md

# Stress (P11 K46, docs/testing.md §4; local only, not in make test or
# CI): the normal kernel, four processors, the AHCI test disk and user
# networking; the shell runs stress (100 processes, 1000 files, 64 MiB
# through a pipe, 50 downloads at once, 20 windows) three times and checks
# that frames, kernel heap, handles and processes came back after the
# third round to where the second left them.
test-stress: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-stress.img
	mkdir -p $(BUILD)/www
	head -c 262144 /dev/urandom > $(BUILD)/www/stress.bin
	python3 tools/with_http.py $(BUILD)/www $(NET_PORT) -- \
		python3 tools/qemu_test.py --timeout 900 --log $(BUILD)/test-stress.log \
		--type "STELA shell" "stress http://10.0.2.2:$(NET_PORT)/stress.bin\n" \
		--expect "mouseion: ready" --expect "stress: round 1 windows OK" --expect "stress: round 2 windows OK" \
		--expect "stress: round 3 windows OK" --expect "stress: OK" \
		-- $(QEMU) $(QEMU_FLAGS) -smp 4 -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-stress.img \
		-device ide-hd,drive=sata1,bus=ide.1

test-net-cards:
	for card in $(NET_CARDS); do \
		python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-net-$$card.log \
			--type "STELA shell" "ifconfig\n" \
			--type "inet 10.0.2.15" "ping 10.0.2.2 2\n" \
			$(NET_DNS_TYPE) \
			--type "$(NET_FETCH_AFTER)" "fetch http://10.0.2.2:$(NET_PORT)/hello.txt\n" \
			--type "fetch exited" "run linux/net $(NET_PORT) host$(NET_LINUX_NAME)\n" \
			--expect "net: DHCP: 10.0.2.15" --expect "inet 10.0.2.15 mask 255.255.255.0 gateway 10.0.2.2" \
			--expect "  received " \
			--expect "bytes from 10.0.2.2: seq=1" $(NET_DNS_EXPECT) --expect "hello from the host over TCP" \
			--expect "net: connected from 10.0.2.15" $(NET_LINUX_EXPECT) --expect "net: OK" \
			-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio -nic none \
			-netdev user,id=n0 -device $$card,netdev=n0 || exit 1; \
	done

# Sound (docs/sound.md): QEMU's Intel HDA with a codec that records to a
# WAV file; the shell plays `tone` (440 Hz, 1 s), then shuts down so that
# QEMU closes the file; check_wav.py wants a second of loud 440 Hz audio.
SOUND_WAV   := $(BUILD)/out.wav
test-sound: esp $(BUILD)/OVMF_VARS.fd
	rm -f $(SOUND_WAV)
	python3 tools/qemu_test.py --exit-code 0 --timeout 90 --log $(BUILD)/test-sound.log \
		--type "STELA shell" "tone\n" --type "tone exited" "shutdown\n" \
		--expect "hda: " --expect "STELA shell" --expect "tone: 440 Hz for 1000 ms" \
		--expect "tone: done" --expect "power: turning off" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-audiodev wav,id=a0,path=$(SOUND_WAV) -device intel-hda -device hda-duplex,audiodev=a0
	python3 tools/check_wav.py $(SOUND_WAV) 1.0 440

# Boot options (docs/p520.md): the normal kernel with a boot.cfg that
# asks for text mode and no USB; init starts the shell without the window
# server and the xHCI controller is left alone. verbose and waitkey (P11,
# docs/p520.md §3.1): every step reports, and each of the kernel's eight
# steps waits for the Enter that QMP sends (none may time out).
ESP_TEXT    := $(BUILD)/esp-text
test-textmode: esp $(BUILD)/OVMF_VARS.fd
	rm -rf $(ESP_TEXT)
	cp -r $(ESP) $(ESP_TEXT)
	printf 'textmode\n  nousb   # no xHCI\nverbose\nwaitkey\n' > $(ESP_TEXT)/STELA/boot.cfg
	python3 tools/qemu_test.py --timeout 90 --log $(BUILD)/test-textmode.log \
		$(foreach i,1 2 3 4 5 6 7 8,--send-keys "boot: press any key" "ret") \
		--type "STELA shell" "uptime\n" \
		--expect "boot: options textmode nousb verbose waitkey" --expect "boot: memory done" \
		--expect "boot: timer done" --expect "boot: PCI done" --expect "boot: press any key" \
		--expect "usb: off (boot option nousb)" --expect "boot: volumes done" \
		--expect "init: text mode (boot option)" --expect "STELA shell" --expect "uptime: " \
		-- $(QEMU) $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP_TEXT) -display none -serial stdio \
		-device qemu-xhci
	test $$(grep -c "boot: no key" $(BUILD)/test-textmode.log) -eq 0

# The desktop: boot the normal kernel (init starts the window server and
# the terminal, the terminal the shell), type a command, start echo_win
# from the shell and type a line into it, copy it with Ctrl-C and paste
# it back with Ctrl-V (P11: the clipboard), then save the screen and check
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
		--type "echo_win: ready" "hello\n" \
		--send-keys "echo_win: hello" "ctrl-c" \
		--send-keys "echo_win: copied hello" "ctrl-v ret" \
		--expect "windowserver: " --expect "STELA shell" --expect "uptime:" \
		--expect "echo_win: hello" --expect "echo_win: copied hello" --expect "echo_win: hello" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio
	python3 tools/fb_check.py $(BUILD)/screen.ppm $(WINDOW_CHECKS)

# The shell on the normal kernel with the AHCI test disk (a copy of its
# own) on two processors (docs/smp.md): commands typed through QMP, their
# answers checked on the serial console. Each command is typed when the
# previous one's line appears.
test-shell: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-shell.img
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-shell.log \
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
		--type "uptime: " "{up}{up}\n" \
		--type "memory: " "vls{left}{left}o{end}\n" \
		--type "Initrd     initrd" "ls /\n" \
		--type "中文.txt" "hwinfo\n" \
		--type "hwinfo exited with code 0" "cat /welcome.txt | grep -i stela | wc -l\n" \
		--type "wc exited with code 0" "grep -n -i stela /welcome.txt\n" \
		--type "grep exited with code 0" "cp -r Initrd:/linux /lx\n" \
		--type "bytes copied" "find /lx -name n*\n" \
		--type "find exited with code 0" "date\n" \
		--type " +08:00" "echo /lx/*s\n" \
		--type "echo exited with code 0" "cat /welc{tab}\n" \
		--type "Welcome to STELA's" "upt{tab}\n" \
		--type "ticks" "ls /lx/t{tab}ty\n" \
		--type "> ls /lx/tty" "Monitor.app &\n" \
		--expect "smp: 2 processor(s) online" --expect "STELA shell" --expect "Data       eHFS" --expect "Initrd     initrd" \
		--expect "welcome.txt" --expect "item(s)" --expect "> mkdir t" --expect "> cd t" \
		--expect "Data:/t" --expect "cat: Data:/t/nothing: not found" \
		--expect "hello from user mode" --expect "hello exited with code 0" \
		--expect "shell" --expect "ended)" --expect "memory: " --expect "uptime: " \
		--expect "memory: " --expect "Initrd     initrd" \
		--expect "中文.txt" \
		--expect "cpu: " --expect "processors: 2 online" --expect "[acpi]" --expect "FACP" \
		--expect "8086:29c0 class 06.00.00 host bridge" --expect "[disks] 2" --expect "2 \"Data\" " \
		--expect "[network]" \
		--expect "hwinfo exited with code 0" --expect "wc exited with code 0" \
		--expect "1:Welcome to STELA's Data volume." --expect "file(s)," \
		--expect "Data:/lx/nosys" --expect "find exited with code 0" --expect " +08:00" \
		--expect "/lx/files /lx/nosys" --expect "cat /welcome.txt " --expect "Welcome to STELA's" \
		--expect "uptime " --expect "uptime: " --expect "time  tty" --expect "tty" \
		--expect "monitor: showing" \
		-- $(QEMU) $(QEMU_FLAGS) -smp 2 -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-shell.img \
		-device ide-hd,drive=sata1,bus=ide.1

# Pipes and jobs (P11, docs/userland.md §6): hello into wc, into a file
# and appended, wc from the file, a Linux program into wc, a job killed
# in the background and one brought to the foreground. Each command is
# typed once the one before has exited: keys typed during a foreground
# job go to that job's terminal.
test-pipe: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-pipe.img
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-pipe.log \
		--type "STELA shell" "hello | wc\n" \
		--type "wc exited with code" "hello > out.txt\n" \
		--type "hello exited with code" "cat out.txt\n" \
		--type "hello from a second thread" "hello >> out.txt\n" \
		--type "hello exited with code" "wc < out.txt\n" \
		--type "wc exited with code" "run linux/hello x | wc\n" \
		--type "wc exited with code" "proctest wait &\n" \
		--type "[1] proctest wait" "jobs\n" \
		--type "[1] running" "kill 1\n" \
		--type "[1] killed" "spin &\n" \
		--type "[2] spin" "fg 2\n" \
		--expect "STELA shell" --expect "wc: 3 lines" --expect "wc exited with code 0" \
		--expect "hello from user mode" --expect "wc: 6 lines" --expect "wc: 5 lines" \
		--expect "[1] running  proctest wait" --expect "[1] killed (137)" \
		--expect "spin: " --expect "[2] exited with code 0" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-pipe.img \
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
	python3 tools/qemu_test.py --timeout 180 --log $(BUILD)/test-linux.log \
		--type "STELA shell" "run linux/hello a b\n" \
		--type "hello exited with code" "mkdir linuxdir\n" \
		--type "> mkdir linuxdir" "cd linuxdir\n" \
		--type "> cd linuxdir" "run linux/files\n" \
		--type "files exited with code" "run linux/memory\n" \
		--type "memory exited with code" "run linux/time\n" \
		--type "time exited with code" "run linux/nosys\n" \
		--type "nosys exited with code" "run linux/tty\n" \
		--type "tty: type a line" "stela rocks\n" \
		--type "tty exited with code" "run linux/poll\n" \
		--type "poll: type a line" "poll works\n" \
		--type "poll exited with code" "run linux/pipe\n" \
		--type "pipe exited with code" "run linux/threads\n" \
		--type "threads exited with code" "run linux/procs\n" \
		--type "procs exited with code" "run linux/misc\n" \
		--type "misc exited with code" "run linux/hello-std rust\n" \
		--type "hello-std exited with code" "run linux/signals\n" \
		--send-keys "signals: press Ctrl-C" "ctrl-c" \
		--expect "hello from linux: 3 args" --expect "hello: uname STELA" \
		--expect "hello: thread-local value 7" --expect "hello exited with code 3" \
		--expect "files: working directory /Data/linuxdir" \
		--expect "files: OK" --expect "files exited with code 0" \
		--expect "memory: OK" --expect "memory exited with code 0" \
		--expect "time: TZ UTC-8, 28800 seconds east of UTC" --expect "time: OK" --expect "time exited with code 0" \
		--expect "nosys: OK" --expect "nosys exited with code 0" \
		--expect "tty: 25 rows, 80 columns" \
		--expect "tty: you typed: stela rocks" --expect "tty exited with code 0" \
		--expect "poll: you typed: poll works" --expect "poll: OK" \
		--expect "poll exited with code 0" \
		--expect "pipe: OK" --expect "pipe exited with code 0" \
		--expect "threads: 10 threads added up to 110000" --expect "threads: OK" \
		--expect "threads exited with code 4" \
		--expect "procs: child said: hello from the child" --expect "argv[1] = one" \
		--expect "argv[2] = /tmp/script" --expect "procs: OK" --expect "procs exited with code 0" \
		--expect "misc: pty /dev/pts/0 works" --expect "misc: exe /Initrd/linux/misc, fd 1 is /dev/tty" \
		--expect "misc: OK" --expect "misc exited with code 0" \
		--expect "hello-std: 8 threads added up to 36000" --expect "hello-std: channel [0, 1, 4, 9, 16]" \
		--expect "hello-std: OK" --expect "hello-std exited with code 0" \
		--expect "signals: handlers, mask and timers work" \
		--expect "signals: children, pipes, faults and restarts work" \
		--expect "signals: got SIGINT" --expect "signals: OK" --expect "signals exited with code 0" \
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
# P11 K41: a USB stick too (Bulk-Only mass storage) on the hub's port 2
# (so the root ports stay as they were; the keyboard added back goes to
# hub port 3): an MBR with one FAT32 partition at 1 MiB, labelled STICK,
# holding HELLO.TXT (made with mtools in build/images); it is mounted as
# STICK: and read from the shell. K42: it is unplugged (unmounted;
# Mouseion hears of it), a copy is plugged in (a second image: QEMU locks
# an image while a drive uses it), the shell writes a file on it (FAT
# writing) and reads it back, and mtools finds the file on the host.
USB_STICK = $(IMAGES)/usb-stick.img
USB_STICK2 = $(IMAGES)/usb-stick2.img
USB_FLAGS = -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0,id=kbd \
	-device usb-mouse,bus=xhci.0,id=mouse -device usb-hub,bus=xhci.0,id=hub \
	-device usb-kbd,bus=xhci.0,port=3.1,id=kbd2 \
	-drive if=none,id=stick,format=raw,file=$(USB_STICK) \
	-device usb-storage,bus=xhci.0,port=3.2,drive=stick,id=stick \
	-blockdev driver=raw,node-name=stick2,file.driver=file,file.filename=$(abspath $(USB_STICK2))
test-usb: esp $(BUILD)/OVMF_VARS.fd
	mkdir -p $(IMAGES)
	rm -f $(USB_STICK)
	truncate -s 64M $(USB_STICK)
	python3 -c "import sys,struct; f=open(sys.argv[1],'r+b'); f.seek(446); f.write(bytes([0,0,0,0,0x0C,0,0,0])+struct.pack('<II',2048,129024)); f.seek(510); f.write(b'\x55\xaa')" $(USB_STICK)
	mformat -i $(USB_STICK)@@1M -T 129024 -F -v STICK ::
	printf 'hello from the USB stick\n' > $(BUILD)/stick-hello.txt
	mcopy -i $(USB_STICK)@@1M $(BUILD)/stick-hello.txt ::/HELLO.TXT
	cp $(USB_STICK) $(USB_STICK2)
	python3 tools/qemu_test.py --timeout 90 --log $(BUILD)/test-usb.log \
		--type "usb: 7: hub" "uptime\n" \
		--qmp "uptime:" input-send-event '{"events":[{"type":"rel","data":{"axis":"x","value":30}}]}' \
		--qmp "usb: 6: first report" device_del '{"id":"mouse"}' \
		--qmp "usb: 6: removed" device_add '{"driver":"usb-mouse","bus":"xhci.0","id":"mouse2"}' \
		--qmp "usb: 8: mouse" device_del '{"id":"kbd2"}' \
		--qmp "usb: 7.1: removed" device_add '{"driver":"usb-kbd","bus":"xhci.0","port":"3.3","id":"kbd3"}' \
		--expect "usb: xhci0" --expect "usb: 7: hub" --expect "uptime:" \
		--expect "usb: 6: first report" --expect "usb: 6: removed" --expect "usb: 8: mouse" \
		--type "usb: 7.3: keyboard" "cat STICK:/hello.txt\n" \
		--qmp "hello from the USB stick" device_del '{"id":"stick"}' \
		--qmp "mouseion: volume STICK went" device_add '{"driver":"usb-storage","bus":"xhci.0","port":"3.2","drive":"stick2","id":"stick2"}' \
		--type "mouseion: volume STICK came" "echo written by STELA > STICK:/new.txt\n" \
		--type "echo exited with code 0" "cat STICK:/new.txt\n" \
		--expect "usb: 7.1: removed" --expect "usb: 7.3: keyboard" \
		--expect "hello from the USB stick" --expect "vfs: unmounted STICK (usb1 went away)" \
		--expect "mouseion: volume STICK went" --expect "vfs: mounted STICK (FAT, rw, 62 MiB) from usb2" \
		--expect "mouseion: volume STICK came" --expect "> cat STICK:/new.txt" --expect "written by STELA" \
		-- $(QEMU) $(subst -machine q35,-machine q35$(comma)i8042=off,$(QEMU_FLAGS)) \
		-display none -serial stdio $(USB_FLAGS)
	mtype -i $(USB_STICK2)@@1M ::/new.txt | grep -q "written by STELA"

# Mouseion (2560x1600, 16x32 font, 64-pixel icons in 160x112 cells; the
# desktop's volume icons on the right: Boot, Data, Initrd, Tmp): double
# click Data, then welcome.txt in Data's window (Kalamos opens it, P11),
# then Initrd, then Terminal.app (the sixth icon, after Classic.app,
# Echo.app, Kalamos.app, Monitor.app and Periplus.app; P11) in Initrd's
# window (a second terminal starts). The screen shows the volume icons
# and the new terminal active.
MOUSEION_CHECKS := --expect-rect 2445,82,40,12,\#c8763c --expect-rect 2000,1300,300,200,\#5b7b86 \
	--expect-rect 300,192,100,10,\#c8763c
# P11: Delete on the selected Classic.app asks, and Enter tries (Initrd is
# read only, so it stays); a second boot opens the windows open last time
# by themselves.
test-mouseion: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci-pristine.img $(IMAGES)/ahci-mouseion.img
	python3 tools/qemu_test.py --timeout 120 --screendump $(BUILD)/mouseion.ppm --log $(BUILD)/test-mouseion.log \
		--click "mouseion: started Terminal" 2464 204 2 \
		--click "mouseion: opened Data:/" 256 232 2 \
		--click "kalamos: opened Data:/welcome.txt" 2464 316 2 \
		--click "mouseion: opened Initrd:/" 296 274 1 \
		--type "mouseion: selected Classic.app" "{delete}" \
		--type "mouseion: asking" "\n" \
		--click "mouseion: delete Classic.app" 1096 272 2 \
		--expect "mouseion: ready, 4 volume(s)" --expect "mouseion: opened Data:/" \
		--expect "kalamos: opened Data:/welcome.txt" --expect "mouseion: opened Initrd:/" \
		--expect "mouseion: asking: Delete \"Classic.app\"?" \
		--expect "mouseion: delete Classic.app: read-only volume" \
		--expect "mouseion: started Terminal" --expect "STELA shell" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-mouseion.img \
		-device ide-hd,drive=sata1,bus=ide.1
	python3 tools/fb_check.py $(BUILD)/mouseion.ppm $(MOUSEION_CHECKS)
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-mouseion-2.log \
		--expect "mouseion: opened Data:/" --expect "mouseion: opened Initrd:/" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-mouseion.img \
		-device ide-hd,drive=sata1,bus=ide.1

# The editor (P11, docs/apps.md §2): Kalamos.app opens a file that does
# not exist yet, the text is typed over QMP and saved with Alt-S (the
# window server's menu shortcut, K43); Ctrl-Q
# closes it, the shell shuts down, and the host compares the file.
test-editor: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-editor.img
	python3 tools/qemu_test.py --exit-code 0 --timeout 120 --log $(BUILD)/test-editor.log \
		--type "STELA shell" "run Kalamos.app /notes.txt\n" \
		--type "kalamos: new Data:/notes.txt" "Hello from Kalamos.\nThe second line\n" \
		--send-keys "kalamos: modified" "alt-s" \
		--send-keys "kalamos: saved Data:/notes.txt" "ctrl-q" \
		--type "Kalamos exited with code 0" "shutdown\n" \
		--expect "kalamos: new Data:/notes.txt" --expect "kalamos: modified" \
		--expect "kalamos: saved Data:/notes.txt (36 bytes)" --expect "kalamos: closed" \
		--expect "power: turning off" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-editor.img \
		-device ide-hd,drive=sata1,bus=ide.1
	rm -f $(BUILD)/notes.txt
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-editor.img out /notes.txt $(BUILD)/notes.txt
	printf 'Hello from Kalamos.\nThe second line\n' | cmp - $(BUILD)/notes.txt
	$(EHFS_TOOLS)/fsck.ehfs --offset 83968 $(IMAGES)/ahci-editor.img | grep -q "unmounted cleanly"

# The browser (P11, docs/apps.md §4): Periplus starts on its home page
# with the address field ready; the host serves build/www (tools/
# with_http.py). /sub redirects to /sub/ (the host's server does that for
# folders); Tab and Enter follow its link to page2.html; Alt-Left goes
# back.
test-browser: esp $(BUILD)/OVMF_VARS.fd
	mkdir -p $(BUILD)/www/sub
	printf '<!DOCTYPE html>\n<html><head><title>Sub page</title></head>\n<body><h1>Sub page</h1><p>Go to <a href="../page2.html">the second page</a>.</p></body></html>\n' > $(BUILD)/www/sub/index.html
	printf '<html><head><title>Second page</title></head><body><p>Back to <a href="sub/">the sub page</a>.</p></body></html>\n' > $(BUILD)/www/page2.html
	python3 tools/with_http.py $(BUILD)/www $(NET_PORT) -- \
		python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-browser.log \
		--type "STELA shell" "run Periplus.app\n" \
		--type "periplus: loaded stela:home" "http://10.0.2.2:$(NET_PORT)/sub\n" \
		--send-keys "Sub page" "tab ret" \
		--send-keys "Second page" "alt-left" \
		--expect "periplus: loaded stela:home" \
		--expect 'periplus: loaded http://10.0.2.2:$(NET_PORT)/sub/ (title "Sub page")' \
		--expect 'periplus: loaded http://10.0.2.2:$(NET_PORT)/page2.html (title "Second page")' \
		--expect 'periplus: loaded http://10.0.2.2:$(NET_PORT)/sub/ (title "Sub page")' \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio

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

# The ports (P12, docs/ports.md): build, package and test every port of
# the ports repository (PORTS_DIR, default ~/stela-ports) in QEMU. Local
# only (not in `make test` or CI): it needs the ports repository and
# downloads or copies their sources.
PORTS_DIR ?= $(HOME)/stela-ports
test-ports: musl esp test-images ehfs-tools
	STELA_ROOT=$(CURDIR) python3 $(PORTS_DIR)/tools/portbuild.py all all
	STELA_ROOT=$(CURDIR) python3 $(PORTS_DIR)/tools/portbuild.py index

clippy:
	cargo clippy --release -p stela-boot --target $(UEFI_TARGET) -- -D warnings
	cargo clippy --release $(addprefix -p ,$(KERNEL_CRATES)) --target $(KERNEL_TARGET) -- -D warnings
	cargo clippy --release $(addprefix -p ,$(KERNEL_CRATES)) --target $(KERNEL_TARGET) --features kernel/selftest -- -D warnings
	cargo clippy --release $(addprefix -p ,$(USER_CRATES)) --target $(KERNEL_TARGET) -- -D warnings
	cargo clippy --tests $(addprefix -p ,$(HOST_CRATES)) -- -D warnings
	cargo clippy --tests --lib -p classic -- -D warnings
	cargo clippy --bins -p ehfs-tools -- -D warnings
	cargo clippy --bins -p fuzz -- -D warnings
	cargo clippy --lib --tests $(addprefix -p ,$(HOST_LIB_CRATES)) -- -D warnings

fmt:
	cargo fmt --all

clean:
	rm -rf $(BUILD)
