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
USER_PROGRAMS := hello spin fault ipc_server ipc_client keyecho fstest proctest init shell windowserver terminal echo_win mouseion viewer classic ifconfig ping fetch tone hwinfo bench kalamos monitor periplus stress setup ime e1000u powerbox sandboxtest
USER_DIR    := $(BUILD)/user
# The command-line tools (P11, docs/apps.md): one package, user/bin, with
# a program each in src/bin/; they go to Initrd:/bin/, on the shell's path.
BIN_TOOLS   := echo cat grep wc head tail sort uniq find du df date sleep touch stat hexdump tar true false
# Programs of their own packages that also go to Initrd:/bin/ (P12: pkg,
# docs/pkg.md).
BIN_PROGRAMS := pkg stela-update
INITRD      := $(BUILD)/initrd.tar
# Programs packed as bundles (docs/mouseion.md): program:Name puts the
# program at Name.app/exec with Name.app/info.txt; the rest sit in the
# initrd's root.
BUNDLES     := terminal:Terminal echo_win:Echo viewer:Viewer classic:Classic kalamos:Kalamos monitor:Monitor periplus:Periplus setup:Setup
BUNDLED     := $(foreach b,$(BUNDLES),$(firstword $(subst :, ,$(b))))
# Built but not in the initrd (P13 K17: make test-sandbox puts it on the
# test disk as Data:/Sandbox.app).
NOT_IN_INITRD := sandboxtest
INITRD_ROOT := $(BUILD)/initrd-root
# P14 K4 (docs/system.md): the System volume (read-only eHFS) has the
# applications, the command-line tools, fonts, the IME's dictionary, the
# guide and the Linux libraries; the initrd keeps what starts the desktop
# and the self test's programs. System:/Programs/ has these bundles and
# programs, System:/bin/ the tools.
INITRD_BUNDLES := terminal:Terminal setup:Setup
SYSTEM_BUNDLES := $(filter-out $(INITRD_BUNDLES),$(BUNDLES))
SYSTEM_SINGLES := ifconfig ping fetch tone bench
SYSTEM_IMG  := $(BUILD)/system.img
SYSTEM_ROOT := $(BUILD)/system-root
SYSTEM_IMG_MIB ?= 16
VERSION     ?= $(shell cat VERSION 2>/dev/null || grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
# Every QEMU run of the normal kernel gets the System image as its own
# disk (snapshot: never written, shared by tests run side by side).
SYSTEM_DRIVE := -drive if=none,id=sysdisk,format=raw,file=$(SYSTEM_IMG),snapshot=on \
	-device ide-hd,drive=sysdisk,bus=ide.2

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
# libchewing (LGPL-2.1-or-later, P13 K1, docs/ime.md): the input method's
# engine, never part of the repository. The release tarball is copied
# from ~/stela-ports/sources/ (CHEWING_TARBALL=...) or else downloaded
# from GitHub, checked against its SHA-256 and unpacked in build/chewing/src;
# the host tool init_database turns its phone.cin and tsi.src into the
# dictionary in build/chewing/data (Initrd:/chewing/). user/ime/build.rs
# compiles the library itself.
CHEWING_VERSION := 0.5.1
CHEWING_SHA256 := 9708c63415fa6034435c0f38100e7d30d0e1bac927f67bec6dfeb3fef016172b
CHEWING_URL := https://github.com/chewing/libchewing/releases/download/v$(CHEWING_VERSION)/libchewing-$(CHEWING_VERSION).tar.bz2
CHEWING_TARBALL ?= $(HOME)/stela-ports/sources/libchewing-$(CHEWING_VERSION).tar.bz2
CHEWING_DOWNLOAD := $(BUILD)/downloads/libchewing-$(CHEWING_VERSION).tar.bz2
CHEWING_DIR := $(BUILD)/chewing
CHEWING_DATA := $(CHEWING_DIR)/data/index_tree.dat
# Linux test programs (linux/tests/<name>.c), static, packed into the
# initrd's linux/ folder.
LINUX_TESTS := hello files memory time tty nosys net poll pipe threads signals procs misc events sys
LINUX_DIR   := $(USER_DIR)/linux
# P13 K5: the dynamically linked test program and its libraries.
LINUX_DYN   := $(LINUX_DIR)/dyn $(LINUX_DIR)/lib/ld-musl-x86_64.so.1 $(LINUX_DIR)/lib/libdyntest.so
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
# P13 K23 (docs/testing.md §5): the firmware's variables as a snapshot
# (snapshot=on: writes go to a temporary file), so that several QEMUs can
# use the same file at once (make test -jN).
QEMU_BASE   := -machine q35 -m 512M -no-reboot -rtc base=utc \
               -drive if=pflash,format=raw,readonly=on,file=$(OVMF_CODE) \
               -drive if=pflash,format=raw,file=$(BUILD)/OVMF_VARS.fd,snapshot=on
QEMU_FLAGS  := $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP) $(SYSTEM_DRIVE)
# The self-test kernel leaves QEMU through isa-debug-exit: writing v to port
# 0xF4 exits with status (v << 1) | 1, so 0x10 (success) gives 33.
QEMU_TEST_FLAGS := $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP_TEST) \
               -device isa-debug-exit,iobase=0xf4,iosize=0x04 -display none -serial stdio
# The hardware baseline (docs/architecture.md section 3): a Core 2 class CPU
# without the NX bit, 256 MiB, no KVM. -m comes last and overrides 512M.
QEMU_MIN_FLAGS := $(QEMU_TEST_FLAGS) -cpu Conroe,-nx -m 128M

# Crates that run on the host and have unit tests.
HOST_CRATES := abi block bootinfo drivers ehfs ehfs-tools elfload frame libstela net toolbox pkgcore sysimage
# Crates with a bare-metal binary whose library part has host unit tests.
HOST_LIB_CRATES := kernel windowserver terminal mouseion viewer classic bin periplus ime
# Crates built for the bare-metal kernel target.
KERNEL_CRATES := abi block bootinfo drivers ehfs frame kernel net
# User-space crates (same target).
USER_CRATES := libstela toolbox pkgcore bin $(BIN_PROGRAMS) $(USER_PACKAGES)
# The packages to give cargo: `spin` is also a crates.io crate (a TLS
# dependency, P12), so the workspace's is named with its version.
USER_PACKAGES := $(patsubst spin,spin@0.1.0,$(USER_PROGRAMS))

.PHONY: test-qemu-all test-qemu-shard
.PHONY: all boot kernel user initrd system-image go go-tests musl chewing linux-tests assets draft-assets esp esp-test run bench test test-host test-fuzz test-qemu test-qemu-smp test-qemu-min test-textmode test-window \
	disk-image linux-rust test-ports test-tls test-pkg test-setup test-disk-image classic test-classic test-net test-net-cards test-net-rndis test-net-user test-sound host-images test-images test-storage test-shell test-shutdown test-linux test-usb test-panic test-mouseion test-sandbox test-editor test-ime test-alpine test-pipe test-browser test-update test-go test-stress ehfs-tools test-ehfs-compat clippy fmt clean

all: esp system-image

boot:
	+cargo build --release -p stela-boot --target $(UEFI_TARGET)

kernel:
	+cargo build --release -p kernel --target $(KERNEL_TARGET)

user: $(CHEWING_DATA)
	+cargo build --release $(addprefix -p ,$(USER_PACKAGES)) -p bin $(addprefix -p ,$(BIN_PROGRAMS)) --target $(KERNEL_TARGET)
	mkdir -p $(USER_DIR)/bin
	cp $(addprefix $(BUILD)/target/$(KERNEL_TARGET)/release/,$(USER_PROGRAMS)) $(USER_DIR)/
	cp $(addprefix $(BUILD)/target/$(KERNEL_TARGET)/release/,$(BIN_TOOLS) $(BIN_PROGRAMS)) $(USER_DIR)/bin/

# The initial RAM disk: the user programs in a ustar archive (the Linux
# test programs in its linux/ folder, the bundles as Name.app/ folders),
# with fixed owners and times so that the same programs give the same
# bytes.
# Bundles program:Name ($(1)) into the folder $(2), with their icons.
define put_bundles
	for b in $(1); do p=$${b%%:*}; n=$${b#*:}; mkdir -p $(2)/$$n.app; \
		cp $(USER_DIR)/$$p $(2)/$$n.app/exec; \
		printf '%s\n%s\n' "$$n" "$(VERSION)" > $(2)/$$n.app/info.txt; done
	for b in $(BUNDLE_ICONS); do n=$${b%%:*}; f=$${b#*:}; if [ -d $(2)/$$n.app ]; then \
		cp $$f-32.ppm $(2)/$$n.app/icon-32.ppm; cp $$f-64.ppm $(2)/$$n.app/icon-64.ppm; fi; done
endef

# The System volume's image (P14 K4, docs/system.md): a read-only eHFS
# volume called System, $(SYSTEM_IMG_MIB) MiB, the last 4 KiB left for the
# trailer (K5). `make system-image VERSION=0.2.1` stamps another version.
system-image: user linux-tests ehfs-tools $(CHEWING_DATA)
	rm -rf $(SYSTEM_ROOT)
	mkdir -p $(SYSTEM_ROOT)/Programs $(SYSTEM_ROOT)/bin $(SYSTEM_ROOT)/fonts \
		$(SYSTEM_ROOT)/chewing $(SYSTEM_ROOT)/doc $(SYSTEM_ROOT)/linux
	cp $(addprefix $(USER_DIR)/,$(SYSTEM_SINGLES)) $(SYSTEM_ROOT)/Programs/
	$(call put_bundles,$(SYSTEM_BUNDLES),$(SYSTEM_ROOT)/Programs)
	cp $(addprefix $(USER_DIR)/bin/,$(BIN_TOOLS)) $(SYSTEM_ROOT)/bin/
	cp third_party/fusion-pixel/fusion-pixel-12-zh_tw.bin $(SYSTEM_ROOT)/fonts/
	cp $(CHEWING_DIR)/data/* $(SYSTEM_ROOT)/chewing/
	python3 tools/md_to_html.py docs/guide.md $(SYSTEM_ROOT)/doc/guide.html
	python3 tools/md_to_html.py docs/guide.en.md $(SYSTEM_ROOT)/doc/guide.en.html
	cp -r $(LINUX_DIR)/lib $(SYSTEM_ROOT)/linux/lib
	printf '%s\n' "$(VERSION)" > $(SYSTEM_ROOT)/VERSION
	python3 tools/make_system_image.py $(SYSTEM_ROOT) $(SYSTEM_IMG) $(SYSTEM_IMG_MIB) $(EHFS_TOOLS)
	python3 tools/pkgsign.py keygen $(BUILD)/sys-signing.key $(BUILD)/sys-signing.pub
	cmp $(BUILD)/sys-signing.pub tools/pkg.pub
	python3 tools/sysimage.py sign $(SYSTEM_IMG) $(BUILD)/sys-signing.key "$(VERSION)"

# P14 K2 (docs/mouseion.md §4.2): bundles with their own icon (NAME:the
# images' prefix; -32.ppm for 1x, -64.ppm for 2x). Echo.app shows the
# stela (a draft, to tell it from the usual app icon).
BUNDLE_ICONS := Echo:assets/stela
initrd: user linux-tests
	rm -rf $(INITRD_ROOT)
	mkdir -p $(INITRD_ROOT)
	cp $(addprefix $(USER_DIR)/,$(filter-out $(BUNDLED) $(NOT_IN_INITRD) $(SYSTEM_SINGLES),$(USER_PROGRAMS))) $(INITRD_ROOT)/
	cp -r $(LINUX_DIR) $(INITRD_ROOT)/linux
	rm -rf $(INITRD_ROOT)/linux/lib $(INITRD_ROOT)/linux/go
	mkdir -p $(INITRD_ROOT)/bin
	cp $(addprefix $(USER_DIR)/bin/,$(BIN_PROGRAMS)) $(INITRD_ROOT)/bin/
	mkdir -p $(INITRD_ROOT)/certs
	python3 tools/cacert_to_der.py third_party/mozilla-ca/cacert.pem $(INITRD_ROOT)/certs/cacert.der
	cp tools/pkg.pub $(INITRD_ROOT)/certs/pkg.pub
	$(call put_bundles,$(INITRD_BUNDLES),$(INITRD_ROOT))
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

chewing: $(CHEWING_DATA)

$(CHEWING_DOWNLOAD):
	mkdir -p $(dir $@)
	if [ -f $(CHEWING_TARBALL) ]; then cp $(CHEWING_TARBALL) $@; \
	else curl -fsSL -o $@ $(CHEWING_URL); fi

$(CHEWING_DATA): $(CHEWING_DOWNLOAD) user/ime/c/config.h
	echo "$(CHEWING_SHA256)  $(CHEWING_DOWNLOAD)" | sha256sum -c -
	rm -rf $(CHEWING_DIR)
	mkdir -p $(CHEWING_DIR)/src $(CHEWING_DIR)/data
	tar -xjf $(CHEWING_DOWNLOAD) -C $(CHEWING_DIR)/src --strip-components=1
	cd $(CHEWING_DIR)/src && gcc -O2 -w -DHAVE_CONFIG_H -I$(abspath user/ime/c) -Iinclude -Iinclude/internal \
		-Isrc -Isrc/porting_layer/include -o ../init_database \
		src/tools/init_database.c src/common/chewing-utf8-util.c src/common/key2pho.c
	cd $(CHEWING_DIR)/data && env LC_ALL=C ../init_database ../src/data/phone.cin ../src/data/tsi.src > ../init_database.log
	cp $(addprefix $(CHEWING_DIR)/src/data/,pinyin.tab swkb.dat symbols.dat) $(CHEWING_DIR)/data/

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
		--target=x86_64 CC=gcc AR=ar RANLIB=ranlib > ../configure.log
	$(MAKE) -C $(MUSL_DIR)/src -j$$(nproc) > $(MUSL_DIR)/make.log
	# The dynamic linker's name stays /lib/ld-musl-x86_64.so.1 (what
	# musl-gcc writes into dynamic programs: right on STELA, docs/linux.md
	# §16); only its install symlink goes into build/, not the host's /lib.
	$(MAKE) -C $(MUSL_DIR)/src install \
		LDSO_PATHNAME=$(abspath $(MUSL_DIR))/install/lib/ld-musl-x86_64.so.1 > $(MUSL_DIR)/install.log

# Go (P14 K9, docs/linux.md §17): the official toolchain (BSD-3-Clause)
# from go.dev, kept in GO_TARBALL (downloaded once, its SHA-256 checked)
# and unpacked in build/go; the test programs of linux/tests/go are
# built static (CGO_ENABLED=0) into build/user/linux/go/. Not in the
# initrd or System: make test-go puts them on its test disk.
GO_VERSION := 1.27.3
GO_SHA256  := 65efbc80fdc1fa2cd2631cad5b481777a68e8725f34e89f50fc6ac714e379b40
GO_TARBALL ?= $(HOME)/stela-ports/sources/go/go$(GO_VERSION).linux-amd64.tar.gz
GO_DIR     := $(BUILD)/go
GO_TESTS   := hello goroutine httpget readdir
GO_ENV      = CGO_ENABLED=0 GOOS=linux GOARCH=amd64 GOTOOLCHAIN=local GOFLAGS=-trimpath \
	GOCACHE=$(abspath $(BUILD))/go-cache GOPATH=$(abspath $(BUILD))/go-path
go: $(GO_DIR)/bin/go
$(GO_DIR)/bin/go:
	@if [ ! -f "$(GO_TARBALL)" ]; then mkdir -p "$$(dirname "$(GO_TARBALL)")" && \
		curl -fsSL -o "$(GO_TARBALL)" https://go.dev/dl/go$(GO_VERSION).linux-amd64.tar.gz; fi
	echo "$(GO_SHA256)  $(GO_TARBALL)" | sha256sum -c -
	rm -rf $(GO_DIR)
	mkdir -p $(BUILD)
	tar -C $(BUILD) -xzf "$(GO_TARBALL)"
go-tests: $(addprefix $(LINUX_DIR)/go/,$(GO_TESTS))
$(LINUX_DIR)/go/%: linux/tests/go/%/main.go linux/tests/go/go.mod $(GO_DIR)/bin/go
	mkdir -p $(LINUX_DIR)/go
	cd linux/tests/go && $(GO_ENV) $(abspath $(GO_DIR))/bin/go build -o $(abspath $@) ./$*

linux-tests: $(addprefix $(LINUX_DIR)/,$(LINUX_TESTS)) $(LINUX_DYN) linux-rust

# Dynamic linking (P13 K5, docs/linux.md §16): musl's libc.so is also its
# dynamic linker, Initrd:/linux/lib/ld-musl-x86_64.so.1 (= /lib for Linux
# programs); dyn is a PIE linked against it and libdyntest.so.
$(LINUX_DIR)/lib/ld-musl-x86_64.so.1: $(MUSL_GCC)
	mkdir -p $(LINUX_DIR)/lib
	strip --strip-unneeded -o $@ $(MUSL_DIR)/install/lib/libc.so

$(LINUX_DIR)/lib/libdyntest.so: linux/tests/libdyntest.c $(MUSL_GCC)
	mkdir -p $(LINUX_DIR)/lib
	$(MUSL_GCC) -shared -fPIC -O2 -Wall -Wextra -Werror -Wl,-z,max-page-size=4096 \
		-Wl,--build-id=none -o $@ $<

$(LINUX_DIR)/dyn: linux/tests/dyn.c $(LINUX_DIR)/lib/libdyntest.so $(LINUX_DIR)/lib/ld-musl-x86_64.so.1
	$(MUSL_GCC) -O2 -fPIE -pie -Wall -Wextra -Werror -Wl,-z,max-page-size=4096 -Wl,--build-id=none \
		-Wl,--dynamic-linker=/lib/ld-musl-x86_64.so.1 -o $@ $< -L$(LINUX_DIR)/lib -ldyntest

linux-rust:
	+RUSTFLAGS="$(LINUX_RUSTFLAGS)" cargo build --release --target x86_64-unknown-linux-musl \
		--manifest-path linux/rust/hello-std/Cargo.toml --target-dir $(BUILD)/target/linux-rust
	mkdir -p $(LINUX_DIR)
	cp $(BUILD)/target/linux-rust/x86_64-unknown-linux-musl/release/hello-std $(LINUX_DIR)/

$(LINUX_DIR)/%: linux/tests/%.c $(MUSL_GCC)
	mkdir -p $(LINUX_DIR)
	$(MUSL_GCC) $(LINUX_CFLAGS) -o $@ $<

# The EFI System Partition as a directory; QEMU serves it as a virtual FAT disk.
esp: boot kernel initrd system-image
	mkdir -p $(ESP)/EFI/BOOT $(ESP)/STELA
	cp $(BOOT_EFI) $(ESP)/EFI/BOOT/BOOTX64.EFI
	cp $(KERNEL_ELF) $(ESP)/STELA/kernel.elf
	cp $(INITRD) $(ESP)/STELA/initrd.tar

# Same loader, kernel built with the selftest feature, in its own ESP so that
# `make run` never runs the self tests. After esp (P14 K4): both builds
# leave the kernel at the same path, so esp must have copied its own
# first.
esp-test: boot initrd esp
	+cargo build --release -p kernel --target $(KERNEL_TARGET) --features selftest
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

# The tests that boot STELA in QEMU. P13 K23 (docs/testing.md §5): they
# share no port, disk image, ESP copy or file the others write, so
# `make test -jN` runs them side by side.
QEMU_TESTS := test-qemu test-qemu-smp test-qemu-min test-textmode test-disk-image test-net test-net-user test-tls test-pkg test-setup test-sound test-window test-storage test-shell test-pipe test-shutdown test-linux test-usb test-panic test-mouseion test-sandbox test-editor test-ime test-browser test-update

# The host's tests first, then the QEMU ones (in this order, or side by
# side with -jN).
test: test-host test-fuzz
	$(MAKE) --no-print-directory test-qemu-all

# P14 K9: test-go only where Go is (its tarball downloaded once).
QEMU_TESTS += $(if $(wildcard $(GO_TARBALL)),test-go,)

test-qemu-all: $(QEMU_TESTS)

# CI's QEMU shards (P13 K23): the QEMU tests whose name's checksum is N
# modulo SHARDS (`make test-qemu-shard N=0 SHARDS=3`).
SHARDS ?= 3
N ?= 0
test-qemu-shard:
	$(MAKE) --no-print-directory $$(for t in $(QEMU_TESTS); do \
		[ $$(( $$(printf %s $$t | cksum | cut -d' ' -f1) % $(SHARDS) )) -eq $(N) ] && echo $$t; done)

# Images the host tests read (FAT made by mtools, GPT made by sgdisk).
host-images:
	python3 tools/make_test_images.py host $(BUILD)/images

test-host: initrd host-images $(CHEWING_DATA)
	python3 tools/img_to_rs.py assets $(BUILD)/assets-check > /dev/null
	diff -r $(BUILD)/assets-check assets/generated
	+cargo test $(addprefix -p ,$(HOST_CRATES))
	+cargo test --lib $(addprefix -p ,$(HOST_LIB_CRATES))
	python3 tools/md_to_html.py --test

# Fuzzing on the host (P11 K45, docs/testing.md §3): the parsers of
# disks (eHFS, FAT, GPT, MBR), the initrd, network frames (and two stacks
# talking over a lossy wire), HTTP, DNS, HTML, boot.cfg/classic.cfg and
# paths get changed valid inputs; a panic or a hang fails, the input is
# saved to build/fuzz-<target>.bin. At most 60 s.
test-fuzz: host-images
	+cargo build --profile fuzz -p fuzz
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
# P14 K5: the two System slots, SYSTEM_MIB each, between the ESP and Data
# (tools/make_disk_image.py); slot A holds build/system.img.
SYSTEM_MIB  ?= 64
# The Data partition's first sector (65 MiB + the slots), for ehfs-cp
# --offset.
DATA_SECTOR := $(shell echo $$(( (65 + 2 * $(SYSTEM_MIB)) * 2048 )))
disk-image: esp ehfs-tools
	rm -rf $(DISK_ESP)
	cp -r $(ESP) $(DISK_ESP)
	cp tools/boot.cfg $(DISK_ESP)/STELA/boot.cfg
	python3 tools/make_disk_image.py --system $(SYSTEM_IMG) --system-mib $(SYSTEM_MIB) \
		$(DISK_IMAGE) $(DISK_ESP) $(EHFS_TOOLS) $(DATA_MIB)
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
	python3 tools/make_disk_image.py --setup-done --system $(SYSTEM_IMG) --system-mib $(SYSTEM_MIB) \
		$(TEST_DISK_IMAGE) $(DISK_ESP) $(EHFS_TOOLS) 128
	python3 tools/qemu_test.py --exit-code 0 --timeout 90 \
		--type "STELA shell" "mkdir from-the-disk-image\n" \
		--type "> mkdir from-the-disk-image" "shutdown\n" \
		--expect "system: slot a (version" --expect "System (eHFS, ro" \
		--expect "windowserver: " --expect "mouseion: ready" --expect "system: slot a confirmed" \
		--expect "STELA shell" --expect "power: turning off" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio $(DISK_IMAGE_FLAGS)
	$(EHFS_TOOLS)/fsck.ehfs --offset $(DATA_SECTOR) $(TEST_DISK_IMAGE) | grep -q "unmounted cleanly"
	mtype -i $(TEST_DISK_IMAGE)@@1M ::/STELA/slot | grep -qx "a 0"
	$(EHFS_TOOLS)/ehfs-ls --offset $(DATA_SECTOR) $(TEST_DISK_IMAGE) / | grep -q from-the-disk-image
	python3 tools/qemu_test.py --timeout 90 --type "STELA shell" "ls Data:/\n" \
		--expect "mounted Boot (FAT, ro" --expect "STELA shell" \
		--expect "from-the-disk-image" --expect "Welcome.txt" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio $(DISK_IMAGE_FLAGS)

# Go (P14 K9, docs/linux.md §17): the four programs of linux/tests/go on
# a copy of the test disk (Data:/go/): arguments and exit code, a folder
# and a file in /tmp, goroutines on threads with preemption (SIGURG on
# the alternate signal stack), net/http from the host. In make test when
# the Go tarball is there (GO_TARBALL; CI downloads and caches it).
GO_PORT := 8660
test-go: esp test-images go-tests $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-go.img
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-go.img mkdir /go
	for t in $(GO_TESTS); do $(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-go.img \
		in $(LINUX_DIR)/go/$$t /go/$$t || exit 1; done
	mkdir -p $(BUILD)/www-go
	printf 'hello from the host to Go\n' > $(BUILD)/www-go/go.txt
	python3 tools/with_http.py $(BUILD)/www-go $(GO_PORT) -- \
		python3 tools/qemu_test.py --timeout 180 --log $(BUILD)/test-go.log \
		--type "STELA shell" "run go/hello a b\n" \
		--type "hello exited" "run go/readdir /Data/go\n" \
		--type "readdir exited" "run go/goroutine\n" \
		--type "goroutine exited" "run go/httpget http://10.0.2.2:$(GO_PORT)/go.txt\n" \
		--expect "go: hello from linux/amd64 (go$(GO_VERSION)), 3 args" --expect "go: argv[2] = b" \
		--expect "hello exited with code 7" \
		--expect "go: /Data/go has 4 entries: goroutine hello httpget readdir" \
		--expect "go: read back \"written by Go\\n\", size 14" --expect "readdir exited with code 0" \
		--expect "go: 8 goroutines done, total 36" --expect "go: preempted after" \
		--expect "go: timer OK" --expect "goroutine exited with code 0" \
		--expect "go: HTTP 200, 26 bytes: hello from the host to Go" --expect "httpget exited with code 0" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-go.img \
		-device ide-hd,drive=sata1,bus=ide.1

# The System update (P14 K7, docs/system.md §7): a disk image with
# System A (this build's image) and an empty System B; Data holds the
# next version (VERSION + 1 in its last number), a copy whose signature is
# broken and a copy of this build's image. Mouseion's Special > Software
# Update... (keyboard menu) runs stela-update with POWER:
#  1. next → slot B; restart.  2. B starts (checked in full) and is
#  confirmed; the bad image is refused; rollback (the next start uses A);
#  writing the next version into A again is cut short by killing QEMU at
#  25%.  3. still B (the cut write pointed the slot file back at B); a
#  rollback to the half-written A is refused; this build's image into A;
#  restart.  4. A starts (checked in full).
TEST_UPDATE_IMAGE = $(IMAGES)/stela-update.img
UPDATE_NEXT := $(BUILD)/system-next.img
UPDATE_BAD  := $(BUILD)/system-bad.img
NEXT_VERSION = $(shell echo "$(VERSION)" | awk -F. -v OFS=. '{$$NF = $$NF + 1; print}')
UPDATE_MENU := alt-m right right right right down down down down ret
UPDATE_QEMU = -- $(QEMU) $(QEMU_BASE) -display none -serial stdio \
	-drive if=none,id=disk0,format=raw,file=$(TEST_UPDATE_IMAGE) -device ide-hd,drive=disk0,bus=ide.0,bootindex=0
test-update: esp ehfs-tools $(BUILD)/OVMF_VARS.fd
	rm -rf $(BUILD)/system-next-root
	cp -r $(SYSTEM_ROOT) $(BUILD)/system-next-root
	printf '%s\n' "$(NEXT_VERSION)" > $(BUILD)/system-next-root/VERSION
	python3 tools/make_system_image.py $(BUILD)/system-next-root $(UPDATE_NEXT) $(SYSTEM_IMG_MIB) $(EHFS_TOOLS)
	python3 tools/sysimage.py sign $(UPDATE_NEXT) $(BUILD)/sys-signing.key "$(NEXT_VERSION)"
	cp $(UPDATE_NEXT) $(UPDATE_BAD)
	python3 tools/sysimage.py badsig $(UPDATE_BAD)
	rm -rf $(DISK_ESP)-update
	cp -r $(ESP) $(DISK_ESP)-update
	cp tools/boot.cfg $(DISK_ESP)-update/STELA/boot.cfg
	mkdir -p $(IMAGES)
	python3 tools/make_disk_image.py --setup-done --system $(SYSTEM_IMG) --system-mib $(SYSTEM_MIB) \
		$(TEST_UPDATE_IMAGE) $(DISK_ESP)-update $(EHFS_TOOLS) 96
	$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(TEST_UPDATE_IMAGE) in $(UPDATE_NEXT) /system-next.img
	$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(TEST_UPDATE_IMAGE) in $(UPDATE_BAD) /system-bad.img
	$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(TEST_UPDATE_IMAGE) in $(SYSTEM_IMG) /system-now.img
	python3 tools/qemu_test.py --timeout 240 --log $(BUILD)/test-update-1.log \
		--click "mouseion: started Terminal" 2448 240 1 \
		--send-keys "mouseion: selected Data" "$(UPDATE_MENU)" \
		--type "mouseion: asking for the update" "Data:/system-next.img\n" \
		--send-keys "Restart now?" "ret" \
		--expect "system: slot a (version $(VERSION))" --expect "system: slot a confirmed" \
		--expect "mouseion: started stela-update apply --file Data:/system-next.img" \
		--expect "stela-update: Data:/system-next.img: version $(NEXT_VERSION), signature good" \
		--expect "stela-update: wrote 100%" \
		--expect "system: the next start uses slot b (version $(NEXT_VERSION))" \
		--expect "stela-update: restarting" \
		$(UPDATE_QEMU)
	mtype -i $(TEST_UPDATE_IMAGE)@@1M ::/STELA/slot | grep -qx "b 0 new"
	python3 tools/qemu_test.py --timeout 300 --log $(BUILD)/test-update-2.log \
		--click "mouseion: started Terminal" 2448 240 1 \
		--send-keys "mouseion: selected Data" "$(UPDATE_MENU)" \
		--type "mouseion: asking for the update" "Data:/system-bad.img\n" \
		--send-keys "Press Enter to close." "ret" \
		--click "stela-update: closed" 2448 368 1 \
		--send-keys "mouseion: selected Initrd" "$(UPDATE_MENU)" \
		--type "mouseion: asking for the update" "rollback\n" \
		--send-keys "Restart now?" "esc" \
		--send-keys "Press Enter to close." "ret" \
		--click "stela-update: closed" 2448 240 1 \
		--send-keys "mouseion: selected Data" "$(UPDATE_MENU)" \
		--type "mouseion: asking for the update" "Data:/system-next.img\n" \
		--expect "system: slot b (version $(NEXT_VERSION), checked in full)" \
		--expect "system: slot b confirmed" \
		--expect "stela-update: Data:/system-bad.img: the signature is not STELA's" \
		--expect "system: the next start uses slot a (version $(VERSION))" \
		--expect "stela-update: wrote 25%" \
		$(UPDATE_QEMU)
	mtype -i $(TEST_UPDATE_IMAGE)@@1M ::/STELA/slot | grep -qx "b 0"
	python3 tools/qemu_test.py --timeout 300 --log $(BUILD)/test-update-3.log \
		--click "mouseion: started Terminal" 2448 240 1 \
		--send-keys "mouseion: selected Data" "$(UPDATE_MENU)" \
		--type "mouseion: asking for the update" "rollback\n" \
		--send-keys "Press Enter to close." "ret" \
		--click "stela-update: closed" 2448 368 1 \
		--send-keys "mouseion: selected Initrd" "$(UPDATE_MENU)" \
		--type "mouseion: asking for the update" "Data:/system-now.img\n" \
		--send-keys "Restart now?" "ret" \
		--expect "system: slot b (version $(NEXT_VERSION))" \
		--expect "stela-update: the other slot has no good System image" \
		--expect "system: the next start uses slot a (version $(VERSION))" \
		--expect "stela-update: restarting" \
		$(UPDATE_QEMU)
	python3 tools/qemu_test.py --timeout 240 --log $(BUILD)/test-update-4.log \
		--expect "system: slot a (version $(VERSION), checked in full)" \
		--expect "System (eHFS, ro" --expect "system: slot a confirmed" \
		$(UPDATE_QEMU)
	mtype -i $(TEST_UPDATE_IMAGE)@@1M ::/STELA/slot | grep -qx "a 0"

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
# and the pattern beyond the built-in 640x480 (P13 K13: Classic's scale 1
# times the interface's 2: 2x2 squares again).
CLASSIC_CARD_CHECKS := --expect-rect 600,142,250,10,\#ffffff \
	--expect-pattern 100,1000,400,200,\#000000,\#ffffff,2
classic: user

test-classic: esp ehfs-tools $(BUILD)/OVMF_VARS.fd
	test -f "$(MAC_ROM)" && test -f "$(MAC_DISK)"
	command -v hformat >/dev/null || { echo "make test-classic needs hfsutils (hformat)"; exit 1; }
	rm -rf $(DISK_ESP)
	cp -r $(ESP) $(DISK_ESP)
	cp tools/boot.cfg $(DISK_ESP)/STELA/boot.cfg
	mkdir -p $(IMAGES)
	python3 tools/make_disk_image.py --setup-done --system $(SYSTEM_IMG) --system-mib $(SYSTEM_MIB) \
		$(CLASSIC_IMAGE) $(DISK_ESP) $(EHFS_TOOLS) 128
	$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(CLASSIC_IMAGE) mkdir /Classic
	$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(CLASSIC_IMAGE) in "$(MAC_ROM)" /Classic/ROM
	$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(CLASSIC_IMAGE) in "$(MAC_DISK)" /Classic/System.img
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
	# P13 K19: with tone playing in the background (two programs mixed).
	python3 tools/qemu_test.py --exit-code 0 --timeout 600 --log $(BUILD)/test-classic-sound.log \
		--type "STELA shell" "tone 440 600000 &\n" --type "tone: 440 Hz" "run Classic.app --test-beep\n" \
		--type "Classic exited" "shutdown\n" \
		--expect "tone: 440 Hz for 600000 ms" \
		--expect "classic: sound on" --expect "classic: beep (sent)" --expect "classic: test done" \
		--expect "power: turning off" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio \
		-drive if=none,id=disk0,format=raw,file=$(CLASSIC_IMAGE) \
		-device ide-hd,drive=disk0,bus=ide.0,bootindex=0 \
		-audiodev wav,id=a0,path=$(CLASSIC_WAV) -device intel-hda -device hda-duplex,audiodev=a0
	python3 tools/check_wav.py $(CLASSIC_WAV) 0.05 0
	printf '# make test-classic\nram=32\nresolution=800x600\ndepth=1\nscale=1\ndisk2=Applications.img,ro\n' > $(BUILD)/classic.cfg
	$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(CLASSIC_IMAGE) in $(BUILD)/classic.cfg /Classic/classic.cfg
	rm -f $(CLASSIC_APPS)
	truncate -s 4M $(CLASSIC_APPS)
	HOME=$(abspath $(IMAGES)) hformat -l Applications $(CLASSIC_APPS)
	$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(CLASSIC_IMAGE) in $(CLASSIC_APPS) /Classic/Applications.img
	python3 tools/qemu_test.py --timeout 600 --screendump $(BUILD)/classic-card.ppm --log $(BUILD)/test-classic-card.log \
		--type "STELA shell" "run Classic.app\n" \
		--expect "classic: disk2 Applications.img (4096 KiB, read only)" \
		--expect "classic: started (ROM 368CADFE, 32 MiB" \
		--expect "classic: screen 800x600 (scale 2)" \
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
	$(EHFS_TOOLS)/ehfs-ls --offset $(DATA_SECTOR) $(CLASSIC_FS_IMAGE) / | grep -q "untitled folder"
	$(EHFS_TOOLS)/fsck.ehfs --offset $(DATA_SECTOR) $(CLASSIC_FS_IMAGE) | grep -q "unmounted cleanly"
	python3 tools/qemu_test.py --exit-code 0 --timeout 600 --log $(BUILD)/test-classic-power.log \
		--type "STELA shell" "run Classic.app --test-power\n" --type "Classic exited" "shutdown\n" \
		--expect "classic: finder desktop shown" --expect "classic: restarting" \
		--expect "classic: finder desktop shown" --expect "classic: the Mac was shut down" \
		--expect "power: turning off" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio \
		-drive if=none,id=disk0,format=raw,file=$(CLASSIC_POWER_IMAGE) \
		-device ide-hd,drive=disk0,bus=ide.0,bootindex=0
	rm -f $(BUILD)/classic-system.img
	$(EHFS_TOOLS)/ehfs-cp --offset $(DATA_SECTOR) $(CLASSIC_POWER_IMAGE) out /Classic/System.img $(BUILD)/classic-system.img
	python3 tools/hfs_check.py $(BUILD)/classic-system.img
	rm -f $(BUILD)/classic-system.img
	$(EHFS_TOOLS)/fsck.ehfs --offset $(DATA_SECTOR) $(CLASSIC_POWER_IMAGE) | grep -q "unmounted cleanly"

# Networking (docs/net.md): QEMU user networking (slirp: gateway and host
# 10.0.2.2, DNS 10.0.2.3, us 10.0.2.15) with each of the three cards; an
# HTTP server on the host serves build/www. The shell's ifconfig (DHCP),
# ping to the gateway, fetch of a file over TCP and the Linux program
# linux/net (BSD sockets) must work; DNS (ifconfig resolve) only when the
# host itself resolves names (tools/with_http.py sets HOST_DNS).
NET_PORT    := 8642
NET_CARDS   := e1000 e1000e virtio-net-pci usb-net
NET_DNS_TYPE   = $(if $(HOST_DNS),--type "ping exited" "ifconfig resolve example.com\n",)
NET_DNS_EXPECT = $(if $(HOST_DNS),--expect "example.com has address",)
NET_FETCH_AFTER = $(if $(HOST_DNS),ifconfig exited,ping exited)
NET_LINUX_NAME  = $(if $(HOST_DNS), example.com,)
NET_LINUX_EXPECT = $(if $(HOST_DNS),--expect "net: resolved example.com to",)
test-net: esp $(BUILD)/OVMF_VARS.fd
	mkdir -p $(BUILD)/www
	printf 'hello from the host over TCP\n' > $(BUILD)/www/hello.txt
	python3 tools/with_http.py $(BUILD)/www $(NET_PORT) -- $(MAKE) --no-print-directory test-net-cards test-net-rndis

# TLS (P12, docs/tls.md §5): HTTPS servers on the host (Python's ssl,
# TLS 1.3 only) with certificates from a test root made here; fetch gets
# the file when it trusts that root (--ca), and refuses a certificate for
# another name, an expired one, a self-signed one and (with the system's
# Mozilla roots) the test root's.
TLS_PORT := 8643
# The other three servers of with_https.py (P13 K23: from TLS_PORT, so
# that all the ports can be moved: make test TLS_PORT=...).
TLS_WRONG   = $(shell echo $$(($(TLS_PORT) + 1)))
TLS_EXPIRED = $(shell echo $$(($(TLS_PORT) + 2)))
TLS_OTHER   = $(shell echo $$(($(TLS_PORT) + 3)))
test-tls: esp test-images $(BUILD)/OVMF_VARS.fd
	python3 tools/make_test_certs.py $(BUILD)/tls
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-tls.img
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-tls.img in $(BUILD)/tls/ca.der /test-ca.der
	mkdir -p $(BUILD)/www-tls
	printf 'hello over TLS\n' > $(BUILD)/www-tls/tls.txt
	python3 tools/with_https.py $(BUILD)/www-tls $(BUILD)/tls $(TLS_PORT) -- \
		python3 tools/qemu_test.py --timeout 180 --log $(BUILD)/test-tls.log \
		--type "STELA shell" "run fetch --ca test-ca.der https://10.0.2.2:$(TLS_PORT)/tls.txt\n" \
		--type "fetch exited with code" "run fetch --ca test-ca.der https://10.0.2.2:$(TLS_WRONG)/tls.txt\n" \
		--type "fetch exited with code" "run fetch --ca test-ca.der https://10.0.2.2:$(TLS_EXPIRED)/tls.txt\n" \
		--type "fetch exited with code" "run fetch --ca test-ca.der https://10.0.2.2:$(TLS_OTHER)/tls.txt\n" \
		--type "fetch exited with code" "run fetch https://10.0.2.2:$(TLS_PORT)/tls.txt\n" \
		--expect "fetch: https://10.0.2.2:$(TLS_PORT)/tls.txt: HTTP 200, 15 bytes" \
		--expect "hello over TLS" --expect "fetch exited with code 0" \
		--expect "fetch: https://10.0.2.2:$(TLS_WRONG)/tls.txt: access denied" \
		--expect "fetch: https://10.0.2.2:$(TLS_EXPIRED)/tls.txt: access denied" \
		--expect "fetch: https://10.0.2.2:$(TLS_OTHER)/tls.txt: access denied" \
		--expect "fetch: https://10.0.2.2:$(TLS_PORT)/tls.txt: access denied" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-tls.img \
		-device ide-hd,drive=sata1,bus=ide.1

# Packages (P12, docs/pkg.md §5): two test packages (greet: the Linux hello, and
# a data file depending on it) in the ports repository's format, an
# index signed with the test key (tools/pkgsign.py; its public half is
# Initrd:/certs/pkg.pub), served over HTTPS with the test root trusted
# through Data:/.stela/pkg.conf. pkg updates, searches, installs with the
# dependency, the shell runs the installed program, pkg removes one.
# P13 K23: its own port, certificates and folder (make -j runs it next to
# test-tls).
PKG_PORT := 8653
test-pkg: esp test-images $(BUILD)/OVMF_VARS.fd
	python3 tools/make_test_certs.py $(BUILD)/tls-pkg
	python3 tools/pkgsign.py keygen $(BUILD)/pkg-signing.key $(BUILD)/pkg.pub
	cmp $(BUILD)/pkg.pub tools/pkg.pub
	rm -rf $(BUILD)/www-pkg
	python3 tools/make_test_packages.py $(USER_DIR)/linux/hello $(BUILD)/www-pkg/pkg
	python3 tools/pkgsign.py index $(BUILD)/pkg-signing.key $(BUILD)/www-pkg/pkg https://10.0.2.2:$(PKG_PORT)/pkg/
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-pkg.img
	printf 'server = https://10.0.2.2:$(PKG_PORT)/pkg/\nca = Data:/test-ca.der\n' > $(BUILD)/pkg.conf
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-pkg.img in $(BUILD)/tls-pkg/ca.der /test-ca.der
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-pkg.img in $(BUILD)/pkg.conf /.stela/pkg.conf
	python3 tools/with_https.py $(BUILD)/www-pkg $(BUILD)/tls-pkg $(PKG_PORT) -- \
		python3 tools/qemu_test.py --timeout 240 --log $(BUILD)/test-pkg.log \
		--type "STELA shell" "run pkg update\n" \
		--type "pkg exited with code" "run pkg search greet\n" \
		--type "pkg exited with code" "run pkg install greetdoc\n" \
		--type "pkg exited with code" "run pkg list\n" \
		--type "pkg exited with code" "run greet installed\n" \
		--type "greet exited with code" "cat Data:/Programs/greetdoc/share/hello.txt\n" \
		--type "hello from a package" "run pkg remove greetdoc\n" \
		--type "pkg exited with code" "run pkg install nothere\n" \
		--expect "pkg: the index lists 2 packages" --expect "pkg exited with code 0" \
		--expect "greetdoc 1.0 - greetdoc (a test package)" \
		--expect "pkg: installed greet 1.0" --expect "pkg: installed greetdoc 1.0" \
		--expect "greetdoc 1.0" \
		--expect "hello from linux: 2 args" --expect "argv[1] = installed" \
		--expect "hello from a package" \
		--expect "pkg: removed greetdoc" \
		--expect "pkg: no package called nothere" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-pkg.img \
		-device ide-hd,drive=sata1,bus=ide.1
	$(EHFS_TOOLS)/fsck.ehfs --offset 83968 $(IMAGES)/ahci-pkg.img

# The first start (P12, docs/setup.md §5): boot option `setup`, a disk
# with one partition that was never formatted. Setup.app: 繁體中文, the
# time zone as it is, the partition formatted as Data (its name typed to
# confirm), the network page, no packages (no server configured on a new
# Data volume; pkg's installing is test-pkg), no Classic images, done:
# Mouseion starts. The host checks the new volume and what Setup wrote.
ESP_SETUP := $(BUILD)/esp-setup
test-setup: esp ehfs-tools $(BUILD)/OVMF_VARS.fd
	rm -rf $(ESP_SETUP)
	cp -r $(ESP) $(ESP_SETUP)
	printf 'setup\n' > $(ESP_SETUP)/STELA/boot.cfg
	mkdir -p $(IMAGES)
	python3 tools/make_test_images.py setup $(IMAGES)/setup.img
	python3 tools/qemu_test.py --timeout 240 --log $(BUILD)/test-setup.log \
		--send-keys "setup: page language" "down ret" \
		--send-keys "setup: page timezone" "ret" \
		--type "setup: can become Data: ahci0p1.1" "ahci0p1.1\n" \
		--send-keys "is the Data volume" "ret" \
		--send-keys "setup: page network" "ret" \
		--send-keys "setup: page packages" "ret" \
		--send-keys "setup: page classic" "ret" \
		--send-keys "setup: page display" "right down right down right ret" \
		--send-keys "setup: page done" "ret" \
		--expect "init: first start: Setup.app" --expect "setup: page language" \
		--expect "setup: language zh-TW" --expect "setup: time zone +08:00" \
		--expect "setup: page data" --expect "setup: formatting ahci0p1.1" \
		--expect "vfs: ahci0p1.1 formatted as Data" --expect "setup: ahci0p1.1 is the Data volume" \
		--expect "setup: 套件伺服器: 沒有設定" --expect "setup: page network" --expect "setup: page packages" \
		--expect "setup: page classic" --expect "setup: page display" \
		--expect "setup: display scale=1 blank=30 pattern=meander" \
		--expect "setup: page done" --expect "setup: done" \
		--expect "mouseion: ready" --expect "mouseion: no System volume" \
		-- $(QEMU) $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP_SETUP) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/setup.img -device ide-hd,drive=sata1,bus=ide.1
	$(EHFS_TOOLS)/fsck.ehfs --offset 2048 $(IMAGES)/setup.img
	$(EHFS_TOOLS)/ehfs-cp --offset 2048 $(IMAGES)/setup.img out /.stela/language $(BUILD)/setup-language
	grep -q zh-TW $(BUILD)/setup-language
	$(EHFS_TOOLS)/ehfs-cp --offset 2048 $(IMAGES)/setup.img out /.stela/setup-done $(BUILD)/setup-done
	# P14 K2: the display page's files; the window server uses the scale
	# from the next start (the boot option scale= would win).
	$(EHFS_TOOLS)/ehfs-cp --offset 2048 $(IMAGES)/setup.img out /.stela/display $(BUILD)/setup-display
	printf 'scale=1\nblank=30\n' | cmp - $(BUILD)/setup-display
	$(EHFS_TOOLS)/ehfs-cp --offset 2048 $(IMAGES)/setup.img out /.stela/desktop $(BUILD)/setup-desktop
	printf 'pattern=meander\n' | cmp - $(BUILD)/setup-desktop
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-setup-2.log \
		--expect "windowserver: scale 1" --expect "mouseion: ready" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/setup.img -device ide-hd,drive=sata1,bus=ide.1
	# P14 K7: Setup from a disk image with System slots installs STELA on a
	# second, empty disk (typing its name); that disk then starts by itself
	# (slot A of its own) into its own Setup.
	rm -rf $(DISK_ESP)-install
	cp -r $(ESP) $(DISK_ESP)-install
	cp tools/boot.cfg $(DISK_ESP)-install/STELA/boot.cfg
	python3 tools/make_disk_image.py --system $(SYSTEM_IMG) --system-mib $(SYSTEM_MIB) \
		$(IMAGES)/install-src.img $(DISK_ESP)-install $(EHFS_TOOLS) 64
	rm -f $(IMAGES)/install-target.img
	truncate -s 512M $(IMAGES)/install-target.img
	python3 tools/qemu_test.py --timeout 300 --log $(BUILD)/test-setup-install.log \
		--send-keys "setup: page language" "ret" \
		--send-keys "setup: page timezone" "ret" \
		--type "setup: page install" "ahci0p1\n" \
		--send-keys "setup: installed on ahci0p1" "ret" \
		--send-keys "setup: page data" "ret" \
		--expect "system: slot a (version" --expect "setup: can install on: ahci0p1 (512 MiB)" \
		--expect "setup: page install" --expect "vfs: installing STELA on ahci0p1" \
		--expect "vfs: installed STELA on ahci0p1: ESP 64 MiB, System A and B 64 MiB" \
		--expect "setup: installed on ahci0p1" --expect "setup: page data" --expect "setup: page network" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio \
		-drive if=none,id=disk0,format=raw,file=$(IMAGES)/install-src.img -device ide-hd,drive=disk0,bus=ide.0,bootindex=0 \
		-drive if=none,id=disk1,format=raw,file=$(IMAGES)/install-target.img -device ide-hd,drive=disk1,bus=ide.1
	sgdisk -v $(IMAGES)/install-target.img | grep -q "No problems found"
	mtype -i $(IMAGES)/install-target.img@@1M ::/STELA/slot | grep -qx "a 0"
	python3 tools/qemu_test.py --timeout 180 --log $(BUILD)/test-setup-installed.log \
		--expect "system: slot a (version" --expect "System (eHFS, ro" \
		--expect "init: first start: Setup.app" --expect "setup: page language" \
		-- $(QEMU) $(QEMU_BASE) -display none -serial stdio \
		-drive if=none,id=disk0,format=raw,file=$(IMAGES)/install-target.img -device ide-hd,drive=disk0,bus=ide.0,bootindex=0

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
			-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio -nic none -device qemu-xhci \
			-netdev user,id=n0 -device $$card,netdev=n0 || exit 1; \
	done

# P13 K10: QEMU's usb-net also offers RNDIS (its first configuration);
# the boot option usbrndis makes STELA take it instead of CDC-ECM. Then
# hot plugging: QMP removes the adapter (no network card), adds another
# (a new stack, DHCP again, fetch works).
ESP_RNDIS := $(BUILD)/esp-rndis
test-net-rndis:
	rm -rf $(ESP_RNDIS)
	cp -r $(ESP) $(ESP_RNDIS)
	printf 'usbrndis\n' > $(ESP_RNDIS)/STELA/boot.cfg
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-net-rndis.log \
		--type "STELA shell" "ifconfig\n" \
		--type "inet 10.0.2.15" "ping 10.0.2.2 2\n" \
		--type "ping exited" "fetch http://10.0.2.2:$(NET_PORT)/hello.txt\n" \
		--qmp "fetch exited" device_del '{"id": "un"}' \
		--qmp "no network card" device_add '{"driver": "usb-net", "id": "un", "netdev": "n0", "mac": "40:54:00:12:34:58"}' \
		--type "net: DHCP: 10.0.2.16" "fetch http://10.0.2.2:$(NET_PORT)/hello.txt\n" \
		--expect "network usb-rndis: MAC 40:54:00:12:34:57" --expect "net: DHCP: 10.0.2.15" \
		--expect "bytes from 10.0.2.2: seq=1" --expect "hello from the host over TCP" \
		--expect "net: usb-rndis went away; no network card" \
		--expect "network usb-rndis: MAC 40:54:00:12:34:58" --expect "net: DHCP: 10.0.2.16" \
		--expect "hello from the host over TCP" \
		-- $(QEMU) $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP_RNDIS) -display none -serial stdio $(SYSTEM_DRIVE) \
		-nic none -device qemu-xhci -netdev user,id=n0 -device usb-net,id=un,netdev=n0,mac=40:54:00:12:34:57

# P13 K12 (docs/devices.md): the boot option usernet leaves the PCI card
# to e1000u, a user-space driver init starts with the DEVICES capability:
# the e1000e with MSI (DEVICE_IRQ messages), then the e1000 polled; DHCP,
# ping and fetch through it.
ESP_USERNET := $(BUILD)/esp-usernet
USERNET_PORT := 8650
test-net-user: esp $(BUILD)/OVMF_VARS.fd
	rm -rf $(ESP_USERNET)
	cp -r $(ESP) $(ESP_USERNET)
	printf 'usernet\n' > $(ESP_USERNET)/STELA/boot.cfg
	mkdir -p $(BUILD)/www-user
	printf 'hello from the host over TCP\n' > $(BUILD)/www-user/hello.txt
	for card in e1000e:MSI e1000:polled; do dev=$${card%%:*}; how=$${card#*:}; \
		python3 tools/with_http.py $(BUILD)/www-user $(USERNET_PORT) -- \
		python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-net-user-$$dev.log \
			--type "STELA shell" "ifconfig\n" \
			--type "inet 10.0.2.15" "ping 10.0.2.2 2\n" \
			--type "ping exited" "fetch http://10.0.2.2:$(USERNET_PORT)/hello.txt\n" \
			--expect "net: PCI cards left to user space" \
			--expect "MAC 52:54:00:12:34:56, $$how" --expect "net: DHCP: 10.0.2.15" \
			--expect "bytes from 10.0.2.2: seq=1" --expect "hello from the host over TCP" \
			-- $(QEMU) $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP_USERNET) -display none -serial stdio $(SYSTEM_DRIVE) \
			-nic none -netdev user,id=n0 -device $$dev,netdev=n0 || exit 1; \
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
	# P13 K19: two programs at once, mixed (880 Hz in the background
	# while 440 Hz plays); then the menu bar's speaker: its panel, Mute.
	rm -f $(SOUND_WAV)
	python3 tools/qemu_test.py --exit-code 0 --timeout 90 --log $(BUILD)/test-sound-mix.log \
		--type "STELA shell" "tone 880 1500 &\n" --type "tone: 880 Hz" "tone 440 1000\n" \
		--click "tone exited" 2338 20 1 --click "windowserver: volume panel" 2128 129 1 \
		--type "windowserver: volume 100, muted" "shutdown\n" \
		--expect "tone: 880 Hz for 1500 ms" --expect "tone: 440 Hz for 1000 ms" \
		--expect "windowserver: volume panel" --expect "windowserver: volume 100, muted" \
		--expect "power: turning off" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-audiodev wav,id=a0,path=$(SOUND_WAV) -device intel-hda -device hda-duplex,audiodev=a0
	python3 tools/check_wav.py $(SOUND_WAV) 1.0 440,880

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
# that of the 2560x1600 OVMF mode: 16x32 font, menu bar 40, title bar 38.
# P13 K13: there the interface scale is 2 (logical 1280x800, 8x16 font
# drawn as 16x32): the terminal's content at 40,108, echo_win's title bar
# at y 202-238 from x 1768. A second run with scale=1 (boot.cfg) in 128 MiB
# keeps the old geometry (echo_win's content at 1844,140, 656x448).
WINDOW_CHECKS := --expect-rect 1000,1100,300,300,\#5b7b86 --expect-rect 1300,8,400,20,\#f2e8d5 \
	--expect-rect 300,72,100,10,\#d8cdb8 --expect-text 44,112,1280,800,\#f2e8d5,4 \
	--expect-rect 1880,206,80,8,\#c8763c --expect-text 1784,250,640,420,\#f2e8d5,3
WINDOW_CHECKS_1X := --expect-rect 1000,1100,300,300,\#5b7b86 --expect-rect 1300,8,400,20,\#f2e8d5 \
	--expect-rect 300,72,100,10,\#d8cdb8 --expect-text 44,112,1280,800,\#f2e8d5,4 \
	--expect-rect 1864,104,80,8,\#c8763c --expect-text 1848,144,648,440,\#f2e8d5,3
ESP_SCALE1 := $(BUILD)/esp-scale1
# test-mouseion's (P13 K23: not shared, make -j).
ESP_SCALE1M := $(BUILD)/esp-scale1-m
test-window: esp $(BUILD)/OVMF_VARS.fd
	python3 tools/qemu_test.py --timeout 90 --screendump $(BUILD)/screen.ppm --log $(BUILD)/test-window.log \
		--type "STELA shell" "uptime\n" --type "uptime:" "run Echo.app\n" \
		--type "echo_win: ready" "hello\n" \
		--send-keys "echo_win: hello" "ctrl-c" \
		--send-keys "echo_win: copied hello" "ctrl-v ret" \
		--send-keys "echo_win: hello" "alt-tab" \
		--send-keys "windowserver: switched to Terminal" "alt-tab" \
		--expect "windowserver: scale 2" --expect "STELA shell" --expect "uptime:" \
		--expect "echo_win: hello" --expect "echo_win: copied hello" --expect "echo_win: hello" \
		--expect "windowserver: switched to Terminal" --expect "windowserver: switched to Echo" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio
	python3 tools/fb_check.py $(BUILD)/screen.ppm $(WINDOW_CHECKS)
	# P12 (docs/architecture.md §3.1): the same desktop in 128 MiB; P13
	# K14: at scale 1.
	rm -rf $(ESP_SCALE1)
	cp -r $(ESP) $(ESP_SCALE1)
	printf 'scale=1\n' > $(ESP_SCALE1)/STELA/boot.cfg
	python3 tools/qemu_test.py --timeout 90 --screendump $(BUILD)/s1x.ppm --log $(BUILD)/test-window-128.log \
		--type "STELA shell" "uptime\n" --type "uptime:" "run Echo.app\n" \
		--type "echo_win: ready" "hello\n" \
		--expect "kernel: " --expect "windowserver: scale 1" --expect "mouseion: ready" --expect "STELA shell" --expect "uptime:" \
		--expect "echo_win: hello" \
		-- $(QEMU) $(subst -m 512M,-m 128M,$(QEMU_BASE)) -drive format=raw,file=fat:rw:$(ESP_SCALE1) -display none -serial stdio $(SYSTEM_DRIVE)
	python3 tools/fb_check.py $(BUILD)/s1x.ppm $(WINDOW_CHECKS_1X)

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
		--type "hello exited with code" "ime type su3cl3\n" \
		--type "ime exited with code" "ps\n" \
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
		--expect "ime: 你好" --expect "ime exited with code 0" \
		--expect "shell" --expect "ended)" --expect "memory: " --expect "uptime: " \
		--expect "memory: " --expect "Initrd     initrd" \
		--expect "中文.txt" \
		--expect "cpu: " --expect "processors: 2 online" --expect "[acpi]" --expect "FACP" \
		--expect "8086:29c0 class 06.00.00 host bridge" --expect "[disks] 3" --expect "2 \"Data\" " \
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
		--type "hello-std exited with code" "run linux/dyn\n" \
		--type "dyn exited with code" "run linux/events\n" \
		--type "events exited with code" "run linux/sys\n" \
		--type "sys exited with code" "run linux/signals\n" \
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
		--expect "dyn: hello from a dynamic program" --expect "dyn: pie yes" --expect "dyn: 2 + 3 = 5" \
		--expect "dyn: dlsym add(4, 5) = 9, calls 42" --expect "dyn: malloc ok 7" --expect "dyn exited with code 0" \
		--expect "events: copy-on-write fork works" --expect "events: MAP_SHARED works" \
		--expect "events: eventfd works" --expect "events: timerfd works" --expect "events: epoll works" \
		--expect "events: OK" --expect "events exited with code 0" \
		--expect "sys: release 0.1.0-stela" --expect "sys: name, priority and clock resolution work" \
		--expect "sys: statfs, statx and flock work" --expect "sys: sendfile, copy_file_range and memfd work" \
		--expect "lo is 127.0.0.1" --expect "sys: OK" --expect "sys exited with code 0" \
		--expect "signals: handlers, mask and timers work" \
		--expect "signals: children, pipes, faults and restarts work" \
		--expect "signals: got SIGINT" --expect "signals: OK" --expect "signals exited with code 0" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-linux.img \
		-device ide-hd,drive=sata1,bus=ide.1

# P13 K5, local only (not in make test or CI): Alpine Linux's own dynamic
# linker and its dynamically linked busybox (PIE), from the packages in
# ALPINE_DIR (downloaded once, docs/linux.md §16; they stay in build/).
# Alpine's ld-musl goes to Data:/Programs/lib (searched before
# Initrd:/linux/lib), busybox to Data:/alpine.
ALPINE_DIR ?= $(HOME)/stela-ports/sources/alpine
test-alpine: esp test-images ehfs-tools $(BUILD)/OVMF_VARS.fd
	rm -rf $(BUILD)/alpine && mkdir -p $(BUILD)/alpine
	for p in $(ALPINE_DIR)/musl-*.apk $(ALPINE_DIR)/busybox-*.apk; do tar -xzf $$p -C $(BUILD)/alpine 2>/dev/null; done
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-alpine.img
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-alpine.img mkdir /Programs
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-alpine.img mkdir /Programs/lib
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-alpine.img mkdir /alpine
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-alpine.img in $(BUILD)/alpine/lib/ld-musl-x86_64.so.1 /Programs/lib/ld-musl-x86_64.so.1
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-alpine.img in $(BUILD)/alpine/bin/busybox /alpine/busybox
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-alpine.log \
		--type "STELA shell" "run /alpine/busybox echo alpine busybox runs\n" \
		--type "busybox exited with code" "run linux/dyn\n" \
		--type "dyn exited with code" "run /alpine/busybox uname -s\n" \
		--type "busybox exited with code" "run /alpine/busybox ls /lib\n" \
		--expect "alpine busybox runs" --expect "busybox exited with code 0" \
		--expect "dyn: 2 + 3 = 5" --expect "dyn exited with code 0" \
		--expect "uname -s" --expect "STELA" --expect "ld-musl-x86_64.so.1" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-alpine.img \
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
		--type "usb: 7: hub" "" --type "seen:STELA shell" "uptime\n" \
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
# then Initrd, then Terminal.app (the seventh icon, first of the second row, after Classic.app,
# Echo.app, Kalamos.app, Monitor.app, Periplus.app and Setup.app; P11,
# P12) in Initrd's
# window (a second terminal starts). The screen shows the volume icons
# and the new terminal active.
MOUSEION_CHECKS := --expect-rect 2440,104,24,6,\#c8763c --expect-rect 2000,1300,300,200,\#5b7b86 \
	--expect-rect 300,192,100,10,\#c8763c
# P11: Delete on the selected Classic.app asks, and Enter tries (Initrd is
# read only, so it stays); a second boot opens the windows open last time
# by themselves.
test-mouseion: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci-pristine.img $(IMAGES)/ahci-mouseion.img
	python3 tools/qemu_test.py --timeout 120 --screendump $(BUILD)/mouseion.ppm --log $(BUILD)/test-mouseion.log \
		--click "mouseion: started Terminal" 2448 240 2 \
		--click "mouseion: opened Data:/" 432 392 2 \
		--click "kalamos: opened Data:/welcome.txt" 2448 368 2 \
		--click "mouseion: opened Initrd:/" 512 472 1 \
		--type "mouseion: selected Setup.app" "{delete}" \
		--type "mouseion: asking" "\n" \
		--click "mouseion: delete Setup.app" 672 472 2 \
		--expect "mouseion: ready, 5 volume(s)" --expect "mouseion: opened Data:/" \
		--expect "kalamos: opened Data:/welcome.txt" \
		--expect "mouseion: opened Initrd:/" \
		--expect "mouseion: asking: Delete \"Setup.app\"?" \
		--expect "mouseion: delete Setup.app: read-only volume" \
		--expect "mouseion: started Terminal" --expect "STELA shell" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-mouseion.img \
		-device ide-hd,drive=sata1,bus=ide.1
	python3 tools/fb_check.py $(BUILD)/mouseion.ppm $(MOUSEION_CHECKS)
	# P13 K14: the same clicks at scale 1 (boot.cfg scale=1), where the
	# layout is the old one (icons 64, 16x32 font, physical pixels).
	rm -rf $(ESP_SCALE1M)
	cp -r $(ESP) $(ESP_SCALE1M)
	printf 'scale=1\n' > $(ESP_SCALE1M)/STELA/boot.cfg
	cp $(IMAGES)/ahci-pristine.img $(IMAGES)/ahci-mouseion-1x.img
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-mouseion-1x.log \
		--click "mouseion: started Terminal" 2464 204 2 \
		--click "mouseion: opened Data:/" 256 232 2 \
		--click "kalamos: opened Data:/welcome.txt" 2464 316 2 \
		--click "mouseion: opened Initrd:/" 296 274 1 \
		--expect "windowserver: scale 1" --expect "mouseion: opened Data:/" \
		--expect "kalamos: opened Data:/welcome.txt" --expect "mouseion: opened Initrd:/" \
		--expect "mouseion: selected Setup.app" \
		-- $(QEMU) $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP_SCALE1M) -display none -serial stdio $(SYSTEM_DRIVE) \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-mouseion-1x.img \
		-device ide-hd,drive=sata1,bus=ide.1
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-mouseion-2.log \
		--expect "mouseion: opened Data:/" --expect "mouseion: opened Initrd:/" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-mouseion.img \
		-device ide-hd,drive=sata1,bus=ide.1
	# P14 K2: Get Info's keys on welcome.txt: L locks it, C sets a
	# comment, N cannot rename it while locked, L unlocks it.
	cp $(IMAGES)/ahci-pristine.img $(IMAGES)/ahci-mouseion-info.img
	python3 tools/qemu_test.py --timeout 150 --log $(BUILD)/test-mouseion-info.log \
		--click "mouseion: started Terminal" 2448 240 2 \
		--click "mouseion: opened Data:/" 432 392 1 \
		--send-keys "mouseion: selected welcome.txt" "alt-i" \
		--send-keys "mouseion: info for welcome.txt" "l" \
		--send-keys "mouseion: welcome.txt locked" "c" \
		--type "mouseion: asking for a comment on welcome.txt" "hello note\n" \
		--send-keys "mouseion: comment on welcome.txt" "n" \
		--type "mouseion: asking for a new name of welcome.txt" "2\n" \
		--send-keys "mouseion: rename welcome.txt" "l" \
		--expect "mouseion: opened Data:/" --expect "mouseion: selected welcome.txt" \
		--expect "mouseion: info for welcome.txt" --expect "mouseion: welcome.txt locked" \
		--expect "mouseion: comment on welcome.txt: 10 bytes" \
		--expect "mouseion: rename welcome.txt: access denied" \
		--expect "mouseion: welcome.txt unlocked" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-mouseion-info.img \
		-device ide-hd,drive=sata1,bus=ide.1
	# P14 K4: the System volume (read only, after Tmp on the desktop) and
	# its Programs folder, where Echo.app has its own icon (K2).
	cp $(IMAGES)/ahci-pristine.img $(IMAGES)/ahci-mouseion-sys.img
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-mouseion-sys.log \
		--click "mouseion: started Terminal" 2448 624 2 \
		--click "mouseion: opened System:/" 1232 400 2 \
		--expect "mouseion: opened System:/" --expect "mouseion: Echo.app has its own icon" \
		--expect "mouseion: opened System:/Programs" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-mouseion-sys.img \
		-device ide-hd,drive=sata1,bus=ide.1

# The sandbox (P13 K17, docs/sandbox.md): Mouseion starts Data:/Sandbox.app
# (user/sandboxtest) in its container. It checks what it may reach (its
# container, its bundle read only, Initrd, no other volume) and asks for a
# file: the window server's powerbox shows the Open dialog in Data:/
# (../, Programs/, Sandbox.app/, welcome.txt: Down four times, Enter) and
# hands it Data:/welcome.txt. P13 K20: the desktop's pattern from
# Data:/.stela/desktop, and File > Find... in the Data window (in front
# again once the powerbox's window closed), chosen from the keyboard
# (K22: Alt-M pulls down the Mouseion menu, Right the File menu, Down
# twice to Find..., Enter).
test-sandbox: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci-pristine.img $(IMAGES)/ahci-sandbox.img
	printf 'Sandbox\n0.1.0\n' > $(BUILD)/sandbox-info.txt
	printf 'pattern=meander\n' > $(BUILD)/sandbox-desktop.txt
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-sandbox.img in $(BUILD)/sandbox-desktop.txt /.stela/desktop
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-sandbox.img mkdir /Sandbox.app
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-sandbox.img in $(USER_DIR)/sandboxtest /Sandbox.app/exec
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-sandbox.img in $(BUILD)/sandbox-info.txt /Sandbox.app/info.txt
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-sandbox.log \
		--click "mouseion: started Terminal" 2448 240 2 \
		--click "mouseion: opened Data:/" 432 392 2 \
		--type "powerbox: open dialog shown" "{down}{down}{down}{down}\n" \
		--send-keys "sandbox: OK" "alt-m right down down ret" \
		--type "mouseion: find on Data" "welcome\n" \
		--expect "windowserver: desktop meander" \
		--expect "mouseion: started Sandbox (sandboxed)" \
		--expect "sandbox: working folder /Programs/Sandbox/container" \
		--expect "sandbox: Data not allowed" --expect "sandbox: volume list not allowed" \
		--expect "sandbox: open by id not allowed" --expect "sandbox: Initrd allowed" --expect "sandbox: System allowed" \
		--expect "sandbox: container writable" --expect "sandbox: bundle readable" \
		--expect "sandbox: bundle read only" --expect "sandbox: asking" \
		--expect "windowserver: powerbox (open)" --expect "powerbox: picked Data:/welcome.txt" \
		--expect "sandbox: read Data:/welcome.txt (32 bytes): Welcome to STELA's Data volume." \
		--expect "sandbox: OK" --expect "mouseion: find on Data" \
		--expect "mouseion: found 1 for \"welcome\" on Data" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-sandbox.img \
		-device ide-hd,drive=sata1,bus=ide.1
	# P13 K21: Data:/.stela/display blank=2s: the screen goes black after
	# two seconds without input (checked on a screenshot) and a key wakes
	# it.
	cp $(IMAGES)/ahci-pristine.img $(IMAGES)/ahci-blank.img
	printf 'blank=2s\n' > $(BUILD)/blank-display.txt
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-blank.img in $(BUILD)/blank-display.txt /.stela/display
	python3 tools/qemu_test.py --timeout 90 --log $(BUILD)/test-blank.log \
		--screenshot "windowserver: display asleep" $(BUILD)/blank.ppm \
		--send-keys "windowserver: display asleep" "shift" \
		--expect "mouseion: ready" --expect "windowserver: display asleep" \
		--expect "windowserver: display awake" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-blank.img \
		-device ide-hd,drive=sata1,bus=ide.1
	python3 tools/fb_check.py $(BUILD)/blank.ppm --expect-rect 0,0,2560,1600,\#000000

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

# The input method (P13 K4, docs/ime.md): in Kalamos, Ctrl-Space switches
# to Zhuyin; su3 and Down open the candidate list (a screenshot: the box
# the server reports, parchment with text, over the white page); 1 takes
# the first (你), cl3 and Enter commit 你好; Alt-S saves, Ctrl-Q quits,
# Ctrl-Space back to English and shutdown. The file is 你好 on the host.
# P14 K1: the IME's mark in the menu bar (2560x1600 at scale 2; the
# server prints it: "IME registered (mark at X,Y)").
IME_MARK ?= 2400 20
ESP_IME := $(BUILD)/esp-ime
test-ime: esp test-images $(BUILD)/OVMF_VARS.fd
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-ime.img
	python3 tools/qemu_test.py --exit-code 0 --timeout 150 --log $(BUILD)/test-ime.log \
		--type "STELA shell" "run Kalamos.app /ime.txt\n" \
		--send-keys "kalamos: new Data:/ime.txt" "ctrl-spc" \
		--send-keys "ime: 注" "s u 3 down" \
		--screenshot "ime: candidates" $(BUILD)/ime.ppm \
		--send-keys "ime: candidates" "1 c l 3 ret" \
		--send-keys "kalamos: modified" "alt-s" \
		--send-keys "kalamos: saved Data:/ime.txt" "ctrl-q" \
		--send-keys "Kalamos exited with code 0" "ctrl-spc" \
		--click "ime: 英" $(IME_MARK) 1 \
		--send-keys "ime: settings open" "right right down left left ret" \
		--type "ime: settings pinyin, 7 candidates" "shutdown\n" \
		--expect "ime: ready" --expect "windowserver: IME registered" \
		--expect "kalamos: new Data:/ime.txt" --expect "ime: 注" \
		--expect "ime: candidates" --expect "IME candidates at" --expect "kalamos: modified" \
		--expect "kalamos: saved Data:/ime.txt (6 bytes)" --expect "kalamos: closed" \
		--expect "ime: 英" --expect "windowserver: IME settings" --expect "ime: settings open" \
		--expect "ime: settings pinyin, 7 candidates" --expect "power: turning off" \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-ime.img \
		-device ide-hd,drive=sata1,bus=ide.1
	box=$$(sed -n 's/.*IME candidates at \([0-9,]*\).*/\1/p' $(BUILD)/test-ime.log | head -1); \
		python3 tools/fb_check.py $(BUILD)/ime.ppm --expect-rect $$box,#F2E8D5 --expect-text $$box,#F2E8D5,1
	rm -f $(BUILD)/ime.txt
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-ime.img out /ime.txt $(BUILD)/ime.txt
	printf '你好' | cmp - $(BUILD)/ime.txt
	$(EHFS_TOOLS)/ehfs-cp --offset 83968 $(IMAGES)/ahci-ime.img out /.stela/ime/settings $(BUILD)/ime-settings
	printf 'layout=pinyin\ncandidates=7\n' | cmp - $(BUILD)/ime-settings
	# P14 K1: an IME that ends is started again, once (boot option
	# imecrash: it ends at once, so the second one ends within 10 s too);
	# the new one reads the settings.
	rm -rf $(ESP_IME)
	cp -r $(ESP) $(ESP_IME)
	printf 'imecrash\n' > $(ESP_IME)/STELA/boot.cfg
	python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-ime-crash.log \
		--type "STELA shell" "echo imecrash-ok\n" \
		--expect "ime: ready (libchewing 0.5.1, pinyin, 7 candidates)" --expect "ime: imecrash: ending" \
		--expect "windowserver: the IME ended; starting it again" \
		--expect "ime: imecrash: ending" \
		--expect "windowserver: the IME ended again; keys go straight to the windows" \
		--expect "imecrash-ok" \
		-- $(QEMU) $(QEMU_BASE) -drive format=raw,file=fat:rw:$(ESP_IME) -display none -serial stdio $(SYSTEM_DRIVE) \
		-drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-ime.img \
		-device ide-hd,drive=sata1,bus=ide.1
	$(EHFS_TOOLS)/fsck.ehfs --offset 83968 $(IMAGES)/ahci-ime.img | grep -q "unmounted cleanly"

# The browser (P11, docs/apps.md §4): Periplus starts on its home page
# with the address field ready; the host serves build/www (tools/
# with_http.py). /sub redirects to /sub/ (the host's server does that for
# folders); Tab and Enter follow its link to page2.html; Alt-Left goes
# back.
# P13 K23: its own port and folder (make -j runs it next to test-net).
BROWSER_PORT := 8647
test-browser: esp $(BUILD)/OVMF_VARS.fd
	mkdir -p $(BUILD)/www-browser/sub
	printf '<!DOCTYPE html>\n<html><head><title>Sub page</title></head>\n<body><h1>Sub page</h1><p>Go to <a href="../page2.html">the second page</a>.</p></body></html>\n' > $(BUILD)/www-browser/sub/index.html
	printf '<html><head><title>Second page</title></head><body><p>Back to <a href="sub/">the sub page</a>.</p></body></html>\n' > $(BUILD)/www-browser/page2.html
	python3 tools/with_http.py $(BUILD)/www-browser $(BROWSER_PORT) -- \
		python3 tools/qemu_test.py --timeout 120 --log $(BUILD)/test-browser.log \
		--type "STELA shell" "run Periplus.app\n" \
		--type "periplus: loaded stela:home" "http://10.0.2.2:$(BROWSER_PORT)/sub\n" \
		--send-keys "Sub page" "tab ret" \
		--send-keys "Second page" "alt-left" \
		--send-keys "Sub page" "ctrl-l shift-s y s t e m shift-semicolon slash d o c slash g u i d e dot h t m l ret" \
		--expect "periplus: loaded stela:home" \
		--expect 'periplus: loaded http://10.0.2.2:$(BROWSER_PORT)/sub/ (title "Sub page")' \
		--expect 'periplus: loaded http://10.0.2.2:$(BROWSER_PORT)/page2.html (title "Second page")' \
		--expect 'periplus: loaded http://10.0.2.2:$(BROWSER_PORT)/sub/ (title "Sub page")' \
		--expect 'periplus: loaded System:/doc/guide.html (title "STELA 使用手冊")' \
		-- $(QEMU) $(QEMU_FLAGS) -display none -serial stdio

# The eHFS host tools (tools/ehfs), installed under their usual names.
EHFS_TOOLS  := $(BUILD)/host/ehfs
HOST_TARGET_DIR := $(BUILD)/target/release

ehfs-tools:
	+cargo build --release -p ehfs-tools
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
STORAGE_FLAGS = -drive if=none,id=sata1,format=raw,file=$(IMAGES)/ahci-storage.img \
	-device ide-hd,drive=sata1,bus=ide.1 \
	-drive if=none,id=nvm1,format=raw,file=$(IMAGES)/nvme-storage.img \
	-device nvme,serial=stela-nvme,drive=nvm1,logical_block_size=4096,physical_block_size=4096

test-images: ehfs-tools
	python3 tools/make_test_images.py storage $(IMAGES) $(EHFS_TOOLS)
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-pristine.img

# Boots the self-test kernel twice with the test disks: the second boot
# must find what the first one wrote (raw blocks and eHFS files); then the
# host's fsck.ehfs checks both eHFS volumes.
STORAGE_EXPECT := --expect "vfs: mounted Boot (FAT, ro, 39 MiB), Data (eHFS, rw, 86 MiB), Archive (eHFS, rw, 62 MiB)"
test-storage: esp-test test-images $(BUILD)/OVMF_VARS.fd
	# P13 K23: on copies (the others copy ahci.img while this runs, make -j).
	cp $(IMAGES)/ahci.img $(IMAGES)/ahci-storage.img
	cp $(IMAGES)/nvme.img $(IMAGES)/nvme-storage.img
	python3 tools/qemu_test.py --exit-code 33 $(SEND_KEYS) $(STORAGE_EXPECT) \
		--expect "storage: ahci0p1 boot 1" --expect "storage: nvme0n1 boot 1" \
		--expect "fstest: boot 1" --expect "fstest: OK" \
		--expect "selftest: OK" -- $(QEMU) $(QEMU_TEST_FLAGS) $(STORAGE_FLAGS)
	python3 tools/qemu_test.py --exit-code 33 $(SEND_KEYS) $(STORAGE_EXPECT) \
		--expect "storage: ahci0p1 boot 2" --expect "storage: nvme0n1 boot 2" \
		--expect "fstest: boot 2" --expect "fstest: OK" \
		--expect "selftest: OK" -- $(QEMU) $(QEMU_TEST_FLAGS) $(STORAGE_FLAGS)
	$(EHFS_TOOLS)/fsck.ehfs --offset 83968 $(IMAGES)/ahci-storage.img
	$(EHFS_TOOLS)/fsck.ehfs --offset 2048 $(IMAGES)/nvme-storage.img
	$(EHFS_TOOLS)/ehfs-ls --offset 83968 $(IMAGES)/ahci-storage.img /

# The ports (P12, docs/ports.md): build, package and test every port of
# the ports repository (PORTS_DIR, default ~/stela-ports) in QEMU. Local
# only (not in `make test` or CI): it needs the ports repository and
# downloads or copies their sources.
PORTS_DIR ?= $(HOME)/stela-ports
test-ports: musl esp test-images ehfs-tools
	STELA_ROOT=$(CURDIR) python3 $(PORTS_DIR)/tools/portbuild.py all all
	STELA_ROOT=$(CURDIR) python3 $(PORTS_DIR)/tools/portbuild.py index

clippy: $(CHEWING_DATA)
	+cargo clippy --release -p stela-boot --target $(UEFI_TARGET) -- -D warnings
	+cargo clippy --release $(addprefix -p ,$(KERNEL_CRATES)) --target $(KERNEL_TARGET) -- -D warnings
	+cargo clippy --release $(addprefix -p ,$(KERNEL_CRATES)) --target $(KERNEL_TARGET) --features kernel/selftest -- -D warnings
	+cargo clippy --release $(addprefix -p ,$(USER_CRATES)) --target $(KERNEL_TARGET) -- -D warnings
	+cargo clippy --tests $(addprefix -p ,$(HOST_CRATES)) -- -D warnings
	+cargo clippy --tests --lib -p classic -- -D warnings
	+cargo clippy --bins -p ehfs-tools -- -D warnings
	+cargo clippy --bins -p fuzz -- -D warnings
	+cargo clippy --lib --tests $(addprefix -p ,$(HOST_LIB_CRATES)) -- -D warnings

fmt:
	+cargo fmt --all

clean:
	rm -rf $(BUILD)
