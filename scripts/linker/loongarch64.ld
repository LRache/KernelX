/*
 * LoongArch64 kernel linker script for KernelX.
 *
 * Layout choice (decided in Phase 0):
 *   - Load ELF straight via `qemu-system-loongarch64 -M virt -bios /dev/null -kernel vmkernelx`.
 *   - QEMU reserves physical 0x0..0x100000 for its `boot_info` region, so kernel segments
 *     must start past that. We use physical 0x9000_0000 which is the conventional LA virt
 *     RAM base and is inside DMW1's direct-mapped range.
 *   - DMW1 = 0x9000_0000_0000_0000 -> PA 0 (cached). So VA for the kernel = PA | DMW1 mask.
 *     VIRT_BASE = 0x9000_0000_9000_0000 keeps virt==phys+DMW1 so the pre-paging entry code
 *     can treat it as identity within that window.
 *
 * Keeping section names identical to scripts/linker/riscv64.ld so common kernel code
 * (which references e.g. __init_start / __bss_end / __trampoline_start) just works.
 */

VIRT_BASE = 0x9000000090000000;

ENTRY(_entry)
EXTERN(_entry)

SECTIONS {
    . = VIRT_BASE;

    __kernel_start = .;
    __init_start = .;
    .init : {
        KEEP(*(.text.entry))
        KEEP(*(.text.init))
        KEEP(*(.data.init))
    }
    . = ALIGN(4K);
    __init_end = .;

    .text : {
        __text_start = .;
        *(.text*)
        . = ALIGN(4K);
        __text_end = .;
        __trampoline_start = .;
        KEEP(*(.trampoline.text))
        . = ALIGN(4K);
        __trampoline_end = .;
    }

    .rodata : {
        __rodata_start = .;
        *(.got*)
        *(.rodata*)
        *(.srodata*)
        . = ALIGN(4K);
        __rodata_end = .;
    }

    .data : {
        __data_start = .;
        *(.data*)
        *(.sdata*)
        __data_end = .;
    }

    .bss : {
        __bss_start = .;
        *(.bss*)
        *(.sbss*)
        *(.scommon)
        __bss_end = .;
    }

    . = ALIGN(4K);

    __stack_start = .;
    . = . + 64 * 0x1000;
    __stack_end = .;
    __ktrap_temp_stack_start = .;
    . = . + 0x8000;
    __ktrap_temp_stack_end = .;
    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
    }
}
