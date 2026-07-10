# LoongArch64-specific compiler flags.

RUSTFLAGS += -C no-vectorize-loops -C no-vectorize-slp
RUSTFLAGS += -C target-feature=-lsx,-lasx

CARGO_FLAGS += -Z build-std=core,alloc,compiler_builtins
CARGO_FLAGS += -Z build-std-features=compiler-builtins-mem
