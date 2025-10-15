ifeq ($(KERNELX_HOME),)
$(error KERNELX_HOME is not set)
endif

ifeq ($(ARCH),)
$(error ARCH is not set)
endif

include $(KERNELX_HOME)/scripts/flags/${ARCH}${ARCH_BITS}.env

CC=clang

BUILD_DIR = build/$(ARCH)${ARCH_BITS}
VDSOLIB_BUILD_DIR = $(BUILD_DIR)/lib
VDSO_TARGET = $(VDSOLIB_BUILD_DIR)/libvdso.so
OBJ_TARGET = $(BUILD_DIR)/vdso.o

CMAKE_DEFINITIONS += ARCH=$(ARCH)
CMAKE_DEFINITIONS += ARCH_BITS=$(ARCH_BITS)
CMAKE_DEFINITIONS += KERNELX_HOME=$(KERNELX_HOME)
CMAKE_FLAGS += $(addprefix -D, $(CMAKE_DEFINITIONS))

all: $(OBJ_TARGET)

$(OBJ_TARGET): vdso
	@ mkdir -p $(BUILD_DIR)
	@ $(CC) $(CFLAGS) $(ARCH_COMMON_FLAGS) -c vdso.S -o $@ -DVDSO=\"$(VDSO_TARGET)\"

vdso:
	@ mkdir -p $(VDSOLIB_BUILD_DIR)
	@ cmake -B $(VDSOLIB_BUILD_DIR) $(CMAKE_FLAGS)
	@ cmake --build $(VDSOLIB_BUILD_DIR)
	@ python3 scripts/symbols.py --input $(BUILD_DIR)/lib/libvdso.so --output $(BUILD_DIR)/symbols.inc

.PHONY: all elf