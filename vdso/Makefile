ifeq ($(KERNELX_HOME),)
$(error KERNELX_HOME is not set)
endif

TOOLCHAIN_FILE = $(KERNELX_HOME)/vdso/cmake/toolchain/$(ARCH)$(ARCH_BITS).cmake

BUILD_DIR = build/$(ARCH)${ARCH_BITS}
VDSOLIB_BUILD_DIR = $(BUILD_DIR)/lib
VDSOLIB_TARGET = $(VDSOLIB_BUILD_DIR)/libvdso.so
SYM_TARGET = $(BUILD_DIR)/symbols.inc
OBJ_TARGET = $(BUILD_DIR)/vdso.o

CMAKE_DEFINITIONS += ARCH=$(ARCH)
CMAKE_DEFINITIONS += KERNELX_HOME=$(KERNELX_HOME)
CMAKE_DEFINITIONS += CMAKE_TOOLCHAIN_FILE=$(TOOLCHAIN_FILE)
CMAKE_FLAGS += $(addprefix -D, $(CMAKE_DEFINITIONS))

ifneq ($(SYSROOT),)
CLANG_FLAGS += --sysroot=$(SYSROOT)
endif

all: $(VDSOLIB_TARGET) $(SYM_TARGET)

$(VDSOLIB_TARGET):
	@ mkdir -p $(VDSOLIB_BUILD_DIR)
	@ cmake -B $(VDSOLIB_BUILD_DIR) $(CMAKE_FLAGS)
	@ cmake --build $(VDSOLIB_BUILD_DIR)

$(SYM_TARGET): $(VDSOLIB_TARGET)
	@ python3 scripts/symbols.py --input $< --output $@

clean:
	@ rm -rf $(BUILD_DIR)

.PHONY: all elf