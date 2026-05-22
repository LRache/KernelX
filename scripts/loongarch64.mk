# LoongArch64-specific compiler flags.

RUSTFLAGS += -C no-vectorize-loops -C no-vectorize-slp
RUSTFLAGS += -C target-feature=-lsx,-lasx
