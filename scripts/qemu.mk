include config/config.mk

IMAGE = build/$(PLATFORM)/Image
VMKERNELX = build/$(PLATFORM)/vmkernelx

QEMU = qemu-system-riscv64
QEMU_FLAGS += -M $(CONFIG_QEMU_MACHINE) -m $(CONFIG_QEMU_MEMORY) -nographic
QEMU_FLAGS += -kernel $(IMAGE)
QEMU_FLAGS += -drive file=$(CONFIG_DISK_IMAGE),if=none,id=x0,format=raw 
QEMU_FLAGS += -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
QEMU_FLAGS += -smp $(CONFIG_QEMU_CPUS)

# Set bootargs
ifneq ($(CONFIG_INITPATH),)
BOOTARGS += init=$(CONFIG_INITPATH)
endif

ifneq ($(CONFIG_INITCWD),)
BOOTARGS += initcwd=$(CONFIG_INITCWD)
endif

ifneq ($(CONFIG_ROOT_DEVICE),)
BOOTARGS += root=$(CONFIG_ROOT_DEVICE)
endif

ifneq ($(CONFIG_ROOT_FSTYPE),)
BOOTARGS += rootfstype=$(CONFIG_ROOT_FSTYPE)
endif

QEMU_FLAGS += -append "$(BOOTARGS)"

qemu-run:
	$(QEMU) $(QEMU_FLAGS)

qemu-gdb:	
	$(QEMU) $(QEMU_FLAGS) -s -S

qemu-dts:
	$(QEMU) $(QEMU_FLAGS) -machine dumpdtb=qemu-virt-riscv64.dtb
	@ dtc -I dtb -O dts qemu-virt-riscv64.dtb -o qemu-virt-riscv64.dts

.PHONY: qemu-run qemu-gdb qemu-dts
