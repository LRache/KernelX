# 由 rootfs/Makefile 包含：构建静态链接的 busybox。
BUSYBOX_VERSION ?= 1.36.1
BUSYBOX_DIR = busybox-$(BUSYBOX_VERSION)
BUSYBOX_TAR = build/$(BUSYBOX_DIR).tar.bz2
BUSYBOX_URL = https://busybox.net/downloads/$(notdir $(BUSYBOX_TAR))

# 源码包和解压目录在各架构之间共享，每个架构在 build/<isa>/busybox 中进行源码树外构建。
BUSYBOX_SRC = $(abspath build/$(BUSYBOX_DIR))
BUSYBOX_OBJ = $(abspath $(BUILD_DIR)/busybox)
BUSYBOX_CONFIG = $(BUSYBOX_OBJ)/.config
BUSYBOX = $(BUSYBOX_OBJ)/busybox
BUSYBOX_MAKE = $(MAKE) -C $(BUSYBOX_SRC) O=$(BUSYBOX_OBJ) CROSS_COMPILE=$(CROSS_COMPILE)

$(BUSYBOX_TAR):
	$(info + DOWNLOAD $(notdir $@))
	@ mkdir -p $(@D)
	@ wget -q -O $@.tmp $(BUSYBOX_URL) && mv $@.tmp $@

$(BUSYBOX_SRC)/Makefile: $(BUSYBOX_TAR)
	$(info + EXTRACT $(notdir $<))
	@ tar -xjf $< -C build
	@ touch $@

# 在 defconfig 基础上静态链接，关闭 tc（新内核头文件已移除 CBQ，编译会失败）和 /linuxrc。
$(BUSYBOX_CONFIG): $(BUSYBOX_SRC)/Makefile busybox.mk
	$(info + CONFIG busybox ($(ISA)))
	@ mkdir -p $(BUSYBOX_OBJ)
	@ $(BUSYBOX_MAKE) defconfig >/dev/null
	@ sed -i \
		-e 's/^# CONFIG_STATIC is not set$$/CONFIG_STATIC=y/' \
		-e 's/^CONFIG_TC=y$$/# CONFIG_TC is not set/' \
		-e 's/^CONFIG_LINUXRC=y$$/# CONFIG_LINUXRC is not set/' \
		$@
	@ yes "" | $(BUSYBOX_MAKE) oldconfig >/dev/null

$(BUSYBOX): $(BUSYBOX_CONFIG)
	$(info + MAKE busybox ($(ISA)))
	@ $(BUSYBOX_MAKE) >/dev/null
