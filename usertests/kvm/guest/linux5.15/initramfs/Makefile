CROSS_COMPILE ?= riscv64-linux-gnu-
BUSYBOX_VERSION ?= 1.36.0
BUSYBOX_DIR = busybox-$(BUSYBOX_VERSION)
BUSYBOX_TAR = $(BUSYBOX_DIR).tar.bz2
BUSYBOX_URL = https://busybox.net/downloads/$(BUSYBOX_TAR)
BUSYBOX_SRC = $(BUSYBOX_DIR)/Makefile
BUSYBOX_CONFIG = $(BUSYBOX_DIR)/.config
BUSYBOX_INSTALL = $(BUSYBOX_DIR)/_install
BUSYBOX = $(BUSYBOX_INSTALL)/bin/busybox

INIT_EXEC = init
INITRAMFS = initramfs.cpio.gz
ROOTFS_DIR = initramfs

MAKE_JOBS ?= $(shell nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)

.PHONY: all initramfs clean

all: initramfs

$(BUSYBOX_TAR):
	$(info + DOWNLOADING BUSYBOX)
	@ wget -O $@ $(BUSYBOX_URL)

$(BUSYBOX_SRC): $(BUSYBOX_TAR)
	$(info + EXTRACTING BUSYBOX)
	@ tar -xjf $(BUSYBOX_TAR)

$(BUSYBOX_CONFIG): busybox_defconfig $(BUSYBOX_SRC)
	@ cp busybox_defconfig $(BUSYBOX_CONFIG)

$(BUSYBOX): $(BUSYBOX_CONFIG)
	$(info + BUILDING BUSYBOX)
	@ $(MAKE) -C $(BUSYBOX_DIR) ARCH=riscv CROSS_COMPILE=$(CROSS_COMPILE) olddefconfig
	@ $(MAKE) -C $(BUSYBOX_DIR) ARCH=riscv CROSS_COMPILE=$(CROSS_COMPILE) CFLAGS="-march=rv64g" install -j$(MAKE_JOBS)

$(INITRAMFS): $(BUSYBOX) $(INIT_EXEC)
	$(info + CREATING INITRAMFS)
	@ rm -rf $(ROOTFS_DIR)
	@ cp -r $(BUSYBOX_INSTALL) $(ROOTFS_DIR)
	@ cp $(INIT_EXEC) $(ROOTFS_DIR)/
	@ chmod +x $(ROOTFS_DIR)/$(INIT_EXEC)
	@ mkdir -p $(ROOTFS_DIR)/dev $(ROOTFS_DIR)/proc $(ROOTFS_DIR)/sys $(ROOTFS_DIR)/tmp
	@ cd $(ROOTFS_DIR) && find . | cpio -H newc -o | gzip -9 > ../$(INITRAMFS)

initramfs: $(INITRAMFS)

clean:
	@ rm -rf $(ROOTFS_DIR) $(INITRAMFS)
