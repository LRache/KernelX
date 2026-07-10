ARCH ?= riscv64
CMAKE ?= cmake

ifeq ($(origin CROSS_COMPILE), undefined)
ifeq ($(ARCH),riscv64)
CROSS_COMPILE = riscv64-linux-gnu-
else
CROSS_COMPILE =
endif
endif

ifeq ($(origin CXX), default)
CXX = $(CROSS_COMPILE)g++
endif

BUILD_DIR = build/$(ARCH)

CMAKE_ARGS += -S .
CMAKE_ARGS += -B $(BUILD_DIR)
CMAKE_ARGS += -DCMAKE_SYSTEM_NAME=Linux
CMAKE_ARGS += -DCMAKE_SYSTEM_PROCESSOR=$(ARCH)
CMAKE_ARGS += -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY
CMAKE_ARGS += -DCMAKE_CXX_COMPILER=$(CXX)
CMAKE_ARGS += -DCMAKE_EXPORT_COMPILE_COMMANDS=1

.PHONY: all configure clean

all: configure
	@ $(CMAKE) --build $(BUILD_DIR)

configure:
	@ $(CMAKE) $(CMAKE_ARGS)

clean:
	@ rm -rf $(BUILD_DIR)
