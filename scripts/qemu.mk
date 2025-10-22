include config/config.mk

IMAGE = build/$(PLATFORM)/Image
VMKERNELX = build/$(PLATFORM)/vmkernelx

QEMU = qemu-system-riscv64
QEMU_FLAGS += -M $(QEMU_MACHINE) -m $(QEMU_MEMORY) -nographic
QEMU_FLAGS += -kernel $(IMAGE)
QEMU_FLAGS += -drive file=$(DISK),if=none,id=x0,format=raw 
QEMU_FLAGS += -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
QEMU_FLAGS += -smp $(QEMU_CPUS)

qemu-run:
	$(QEMU) $(QEMU_FLAGS)

qemu-gdb:	
	$(QEMU) $(QEMU_FLAGS) -s -S

qemu-dts:
	$(QEMU) $(QEMU_FLAGS) -machine dumpdtb=qemu-virt-riscv64.dtb
	@ dtc -I dtb -O dts qemu-virt-riscv64.dtb -o qemu-virt-riscv64.dts

.PHONY: qemu-run qemu-gdb qemu-dts
