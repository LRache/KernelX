.DEFAULT_GOAL := qperf-run

include scripts/qemu.mk

QPERF_DIR ?= tools/qperf
QPERF_PLUGIN ?= $(QPERF_DIR)/target/release/libqperf.so
QPERF_SYMBOLIZER_SRC ?= scripts/qperf_symbolize.rs
QPERF_SYMBOLIZER ?= build/tools/qperf-symbolize
QPERF_ADDR2LINE ?= addr2line
QPERF_NM ?= nm
ifeq ($(ARCH),riscv)
QPERF_ADDRESS_MAP ?= 0x80200000:0xffffffc000000000
endif
QPERF_ADDRESS_MAP_ARG := $(if $(QPERF_ADDRESS_MAP),--address-map $(QPERF_ADDRESS_MAP))

QPERF_RUN_TIMESTAMP ?= $(shell date +%Y%m%d-%H%M%S)
QPERF_FREQ ?= 137
QPERF_OUT ?= build/$(ARCH)$(ARCH_BITS)/qperf.bin
QPERF_CONTROL ?=
QPERF_OUT_DIR := $(dir $(QPERF_OUT))
QPERF_FOLDED_OUTPUT_DIR ?= output/qperf
QPERF_FOLDED ?= $(QPERF_FOLDED_OUTPUT_DIR)/kernelx-qperf-$(QPERF_RUN_TIMESTAMP).folded
QPERF_FOLDED_DIR := $(dir $(QPERF_FOLDED))
QPERF_SVG ?= $(basename $(QPERF_FOLDED)).svg
QPERF_SVG_DIR := $(dir $(QPERF_SVG))
QPERF_UNRESOLVED ?= $(basename $(QPERF_FOLDED)).unresolved.tsv
QPERF_UNRESOLVED_DIR := $(dir $(QPERF_UNRESOLVED))
QPERF_CONSOLE_LOG ?= $(QPERF_FOLDED_OUTPUT_DIR)/kernelx-qperf-$(QPERF_RUN_TIMESTAMP).console.log
QPERF_CONSOLE_LOG_DIR := $(dir $(QPERF_CONSOLE_LOG))
QPERF_FLAMEGRAPH ?= tools/FlameGraph/flamegraph.pl
QPERF_REPORT_SCRIPT ?= scripts/qperf_report.py
QPERF_REPORT_DIR ?= $(basename $(QPERF_FOLDED)).report
QPERF_REPORT_SUMMARY ?= $(QPERF_REPORT_DIR)/summary.md
QPERF_REPORT_DB ?= $(QPERF_REPORT_DIR)/profile.sqlite
QPERF_PLUGIN_ARGS = file=$(QPERF_PLUGIN),freq=$(QPERF_FREQ),out=$(QPERF_OUT)
ifneq ($(strip $(QPERF_CONTROL)),)
QPERF_PLUGIN_ARGS := $(QPERF_PLUGIN_ARGS),control=$(QPERF_CONTROL)
endif
QPERF_FLAGS = -plugin $(QPERF_PLUGIN_ARGS)

qperf-plugin:
	@test -f $(QPERF_DIR)/Cargo.toml || { \
		echo "Missing $(QPERF_DIR). Run: git submodule update --init tools/qperf"; \
		exit 1; \
	}
	cargo build --release --manifest-path $(QPERF_DIR)/Cargo.toml

$(QPERF_SYMBOLIZER): $(QPERF_SYMBOLIZER_SRC)
	@ mkdir -p $(dir $(QPERF_SYMBOLIZER))
	rustc --edition=2024 -O $(QPERF_SYMBOLIZER_SRC) -o $(QPERF_SYMBOLIZER)

qperf-symbolizer: $(QPERF_SYMBOLIZER)
	@test -x $(QPERF_SYMBOLIZER) || { \
		echo "Missing $(QPERF_SYMBOLIZER)"; \
		exit 1; \
	}
	@ command -v $(QPERF_ADDR2LINE) >/dev/null || { \
		echo "Missing addr2line tool: $(QPERF_ADDR2LINE)"; \
		exit 1; \
	}
	@ command -v $(QPERF_NM) >/dev/null || { \
		echo "Missing nm tool: $(QPERF_NM)"; \
		exit 1; \
	}

qperf-flamegraph:
	@test -f $(QPERF_FLAMEGRAPH) || { \
		echo "Missing $(QPERF_FLAMEGRAPH). Run: git submodule update --init tools/FlameGraph"; \
		exit 1; \
	}

qperf-report:
	@test -f $(QPERF_REPORT_SCRIPT) || { \
		echo "Missing $(QPERF_REPORT_SCRIPT)"; \
		exit 1; \
	}

qperf-run: QEMU_DEBUG_CONSOLE_LOG = $(QPERF_CONSOLE_LOG)
qperf-run: qperf-plugin qperf-symbolizer qperf-flamegraph qperf-report
ifeq ($(SECOND_DISK_IMAGE),)
	truncate -s $(TMPDISK_SIZE) $(TMPDISK)
endif
	@ mkdir -p $(QEMU_DEBUG_CONSOLE_LOG_DIR) $(QPERF_OUT_DIR) $(QPERF_FOLDED_DIR) $(QPERF_SVG_DIR) $(QPERF_UNRESOLVED_DIR) $(QPERF_CONSOLE_LOG_DIR)
	$(QEMU_SWAP_RUN) $(QEMU) $(QEMU_FLAGS) $(QPERF_FLAGS)
	@ $(QPERF_SYMBOLIZER) --elf $(VMKERNELX) --addr2line $(QPERF_ADDR2LINE) --nm $(QPERF_NM) \
		$(QPERF_ADDRESS_MAP_ARG) \
		--unresolved $(QPERF_UNRESOLVED) $(QPERF_OUT) $(QPERF_FOLDED)
	@ $(QPERF_FLAMEGRAPH) --title "KernelX qperf $(QPERF_RUN_TIMESTAMP)" $(QPERF_FOLDED) > $(QPERF_SVG)
	@ python3 $(QPERF_REPORT_SCRIPT) build $(QPERF_FOLDED) --output $(QPERF_REPORT_DIR) --force
	@ echo "QPerf folded output: $(QPERF_FOLDED)"
	@ echo "QPerf unresolved IPs: $(QPERF_UNRESOLVED)"
	@ echo "QPerf SVG output: $(QPERF_SVG)"
	@ echo "QPerf console log: $(QPERF_CONSOLE_LOG)"
	@ echo "QPerf LLM summary: $(QPERF_REPORT_SUMMARY)"
	@ echo "QPerf query database: $(QPERF_REPORT_DB)"
ifeq ($(SECOND_DISK_IMAGE),)
	@ rm -f $(TMPDISK)
endif

.PHONY: qperf-plugin qperf-symbolizer qperf-flamegraph qperf-report qperf-run
