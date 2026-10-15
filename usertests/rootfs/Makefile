# 构建测试镜像的基础 rootfs：busybox、libc 运行时、/etc 骨架和 autorun init。
# 产物位于 build/<isa>/root，由 ../build.py 收集后再放入测试产物。
ISA ?= riscv64
CROSS_COMPILE ?= riscv64-unknown-linux-gnu-
MUSL_CROSS_COMPILE ?= riscv64-linux-musl-
GLIBC_CC ?= $(CROSS_COMPILE)gcc
MUSL_CC ?= $(MUSL_CROSS_COMPILE)gcc
AUTORUN ?= y

BUILD_DIR = build/$(ISA)
ROOT = $(abspath $(BUILD_DIR)/root)

GLIBC_LOADER_riscv64 = lib/ld-linux-riscv64-lp64d.so.1
GLIBC_LOADER_loongarch64 = lib64/ld-linux-loongarch-lp64d.so.1
MUSL_LOADER_riscv64 = lib/ld-musl-riscv64.so.1
MUSL_LOADER_loongarch64 = lib/ld-musl-loongarch64-lp64d.so.1
GLIBC_LOADER = $(GLIBC_LOADER_$(ISA))
MUSL_LOADER = $(MUSL_LOADER_$(ISA))

ifeq ($(GLIBC_LOADER),)
$(error unsupported rootfs ISA=$(ISA))
endif

GLIBC_RUNTIME = libc.so.6 libm.so.6 libpthread.so.0 libdl.so.2 librt.so.1 libgcc_s.so.1

# 运行时库优先从这些目录中查找，找不到时询问编译器（-print-file-name）。
GLIBC_LIB_DIR ?= $(LIBC_DIR)
MUSL_LIB_DIR ?= $(if $(MUSL_LIBC),$(dir $(MUSL_LIBC)),$(LIBC_DIR))
MUSL_LIBC_NAME = $(if $(MUSL_LIBC),$(notdir $(MUSL_LIBC)),libc.so)

# $(call install_lib,<rootfs 内路径>,<文件名>,<优先查找目录>,<编译器>)
install_lib = src=; \
	if [ -n "$(3)" ] && [ -f "$(3)/$(2)" ]; then src="$(3)/$(2)"; \
	else src=$$($(4) -print-file-name=$(2) 2>/dev/null); fi; \
	if [ -f "$$src" ]; then mkdir -p $(dir $(ROOT)/$(1)) && cp -L "$$src" $(ROOT)/$(1); \
	else echo "warning: $(2) not found for $(4), skipping"; fi

.DEFAULT_GOAL := all

include busybox.mk

.PHONY: all clean

all: $(BUSYBOX)
	$(info + ROOTFS $(ISA))
	@ rm -rf $(ROOT)
	@ mkdir -p $(ROOT)/tmp
	@ $(BUSYBOX_MAKE) install CONFIG_PREFIX=$(ROOT) >/dev/null
	@ cp -a skel/. $(ROOT)/
	@ $(call install_lib,$(GLIBC_LOADER),$(notdir $(GLIBC_LOADER)),$(GLIBC_LIB_DIR),$(GLIBC_CC))
	@ $(foreach lib,$(GLIBC_RUNTIME),$(call install_lib,lib/$(lib),$(lib),$(GLIBC_LIB_DIR),$(GLIBC_CC));)
	@ $(call install_lib,$(MUSL_LOADER),$(MUSL_LIBC_NAME),$(MUSL_LIB_DIR),$(MUSL_CC))
ifeq ($(AUTORUN),y)
	$(info + CC autorun-init.c)
	@ $(GLIBC_CC) -Wall -Wextra -O2 autorun-init.c -o $(ROOT)/init
endif

clean:
	$(info + CLEAN)
	@ rm -rf build
