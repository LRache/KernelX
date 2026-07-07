/*
 * LoongArch64 kernel linker script.
 *
 * QEMU `-M virt` layout (from `-machine dumpdtb`):
 *   RAM:       PA 0..(m-1)
 *   boot_info: PA 0..0x000fffff     (QEMU writes cmdline/systab here)
 *   FDT:       PA 0x00100000..0x001fffff
 *   UART0:     PA 0x1fe001e0
 *   Flash0:    PA 0x1c000000..0x1cffffff
 *
 * First kernel-usable PA is 0x200000.
 *
 * Virtual = physical | 0x9000_0000_0000_0000 (DMW1, MAT=CC). This keeps
 * `arch::paddr_to_kaddr` / `kaddr_to_paddr` as pure bit-OR / bit-AND.
 *
 * Section names match scripts/linker/riscv64.ld so generic kernel code
 * can reference __init_start / __bss_end / __trampoline_start uniformly.
 */

PHYS_BASE = 0x200000;
DMW1_MASK = 0x9000000000000000;
VIRT_BASE = DMW1_MASK + PHYS_BASE;

ENTRY(_entry)
EXTERN(_entry)

SECTIONS {
    . = VIRT_BASE;

    __kernel_start = .;
    __init_start = .;
    .init : AT(PHYS_BASE) {
        KEEP(*(.text.entry))
        KEEP(*(.text.init))
        KEEP(*(.data.init))
    }
    . = ALIGN(4K);
    __init_end = .;

    .text : {
        __text_start = .;
        /* Exclude TLB refill (pinned to its own 4 KiB section below) so
         * its alignment is independent of link-order of preceding text. */
        *(EXCLUDE_FILE(*tlbrefill*) .text)
        *(EXCLUDE_FILE(*tlbrefill*) .text.*)
        . = ALIGN(4K);
        __text_end = .;
        __trampoline_start = .;
        KEEP(*(.trampoline.text))
        . = ALIGN(4K);
        __trampoline_end = .;
    }

    /* TLB refill handler (TLBRENTRY target). Runs with MMU off; needs a
     * 4 KiB-aligned PA. Arch::init writes `__tlbrefill_start & PA_MASK`
     * into TLBRENTRY. */
    . = ALIGN(4K);
    __tlbrefill_start = .;
    .tlbrefill : {
        KEEP(*(.text.tlbrefill))
    }
    . = ALIGN(4K);
    __tlbrefill_end = .;

    .rodata : {
        __rodata_start = .;
        *(.got*)
        . = ALIGN(8);
        __kmodule_exports_start = .;
        KEEP(*(.kmodule.exports))
        KEEP(*(.kmodule.exports.*))
        __kmodule_exports_end = .;
        . = ALIGN(8);
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
