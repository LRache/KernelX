ARCH ?= riscv64
CROSS_COMPILE ?= riscv64-unknown-linux-gnu-

CC = $(CROSS_COMPILE)gcc

# LTP 源码以 submodule 形式放在 ltp/，按架构在源码树外构建并安装到 /tests/ltp。
LTP_SRC = $(abspath ltp)
LTP_PREFIX = /tests/ltp
LTP_HOST = $(notdir $(patsubst %-,%,$(CROSS_COMPILE)))

BUILD_DIR = build/$(ARCH)
OUTPUT_DIR = $(BUILD_DIR)/output
# LTP 用 CURDIR（物理路径）推导源码目录，构建目录必须是解析过符号链接的绝对路径。
LTP_BUILD := $(shell mkdir -p $(BUILD_DIR)/ltp-build && cd $(BUILD_DIR)/ltp-build && pwd -P)
LTP_MAKE = $(MAKE) -C $(LTP_BUILD) -f $(LTP_SRC)/Makefile top_srcdir=$(LTP_SRC) top_builddir=$(LTP_BUILD)

CFLAGS += -Wall -Wextra -O2

ALL_CASES := ltp
CASE_SOURCES_ltp := ltp runner.c kx.runtest kernel.config
CASE_COMMAND_ltp := kx-ltp
include ../mk/case-selection.mk

all: $(foreach case,$(CASES),$(OUTPUT_DIR)/$(case)/kx-ltp)

$(LTP_SRC)/configure:
	$(info + AUTOTOOLS ltp)
	@ $(MAKE) -C $(LTP_SRC) autotools

# 交叉编译时 pkg-config 只能查找目标 sysroot，否则会探测到宿主机的 libtirpc、libmnl 等库，
# 链接目标程序时失败。编译器没有独立 sysroot（如 Debian 的交叉工具链）时直接禁用查找。
LTP_SYSROOT := $(shell $(CC) -print-sysroot 2>/dev/null)
LTP_PKG_CONFIG_LIBDIR ?= $(if $(filter-out /,$(LTP_SYSROOT)),$(LTP_SYSROOT)/usr/lib/pkgconfig:$(LTP_SYSROOT)/usr/share/pkgconfig,$(LTP_BUILD)/no-pkgconfig)

$(LTP_BUILD)/config.status: $(LTP_SRC)/configure
	$(info + CONFIGURE ltp ($(LTP_HOST)))
	@ mkdir -p $(LTP_BUILD)
	@ cd $(LTP_BUILD) && \
		PKG_CONFIG_LIBDIR="$(LTP_PKG_CONFIG_LIBDIR)" PKG_CONFIG_PATH= \
		$(if $(filter-out /,$(LTP_SYSROOT)),PKG_CONFIG_SYSROOT_DIR="$(LTP_SYSROOT)") \
		$(LTP_SRC)/configure \
		--host=$(LTP_HOST) \
		--prefix=$(LTP_PREFIX) \
		CC="$(CC)" \
		AR=$(CROSS_COMPILE)ar \
		RANLIB=$(CROSS_COMPILE)ranlib \
		STRIP=$(CROSS_COMPILE)strip \
		CFLAGS="-O2"

# LTP 自身负责增量构建，因此每次都进入其构建目录；安装直接落到 case 输出目录。
.PHONY: ltp-build
ltp-build: $(LTP_BUILD)/config.status
	$(info + MAKE ltp)
	@ $(LTP_MAKE)

$(OUTPUT_DIR)/ltp/kx-ltp: runner.c kx.runtest kernel.config ltp-build
	$(info + INSTALL ltp)
	@ rm -rf $(@D)
	@ mkdir -p $(@D)
	@ $(LTP_MAKE) install DESTDIR=$(abspath $(@D)/stage) >/dev/null
	@ mv $(@D)/stage$(LTP_PREFIX)/* $(@D)/
	@ rm -rf $(@D)/stage
	@ find $(@D)/testcases/bin -type f -exec sh -c 'head -c 4 "$$1" | grep -q ELF' _ {} \; -exec $(CROSS_COMPILE)strip {} +
	@ cp kx.runtest kernel.config $(@D)/
	$(info + CC $<)
	@ $(CC) $(CFLAGS) $< -o $@

clean:
	$(info + CLEAN)
	@ rm -rf $(BUILD_DIR)
