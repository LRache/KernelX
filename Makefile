BIOS_FIRMWARE = ./lib/opensbi/build/platform/generic/firmware/fw_jump.bin

all:
	@ mkdir -p ./build
	@ cmake -B ./build -DCMAKE_TOOLCHAIN_FILE=./cmake/riscv64-toolchain.cmake -DCMAKE_EXPORT_COMPILE_COMMANDS=1
	@ cmake --build ./build

run:
	qemu-system-riscv64 -M virt -m 256M -nographic \
	-bios ${BIOS_FIRMWARE} \
	-kernel ./build/kernelx-riscv64

gdb:
	qemu-system-riscv64 -machine virt -nographic -bios none -semihosting -kernel ./build/kernelx-riscv64 -s -S

clean:
	rm -rf ./build
