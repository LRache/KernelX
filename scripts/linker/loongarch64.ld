/*
 * LoongArch64 kernel linker script for KernelX.
 *
 * QEMU `-M virt` memory layout we target (verified via `-machine dumpdtb`):
 *   - RAM:        PA 0x00000000..(m-1)           (256 MiB default, size configurable)
 *   - boot_info:  PA 0x00000000..0x000fffff      (QEMU writes cmdline/systab here)
 *   - FDT:        PA 0x00100000..0x001fffff      (FDT_BASE = 0x100000, up to 1 MiB)
 *   - UART0:      PA 0x1fe001e0                  (ns16550a, access via DMW0)
 *   - Flash0:     PA 0x1c000000..0x1cffffff      (aux_boot_code lives here at reset)
 *
 * So the first usable PA for the kernel is 0x200000 (2 MiB mark). Anything
 * lower overlaps with FDT / boot_info and QEMU refuses to run.
 *
 * Virtual layout: DMW1 = VSEG 0x9, MAT=CC, PLV0, so VA = PA | 0x9000_0000_0000_0000.
 * That keeps `virt = phys + 0x9000_0000_0000_0000` throughout the kernel image and
 * lets `arch::paddr_to_kaddr` / `kaddr_to_paddr` be pure bit-OR / bit-AND operations.
 *
 * NOTE: Section names are kept identical to scripts/linker/riscv64.ld so generic
 * kernel code (which references __init_start / __bss_end / __trampoline_start) works.
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
        /*
         * Generic text. Exclude the TLB refill page (handled in its own
         * section below) so the 4 KiB-alignment guarantee there doesn't
         * depend on whatever came before in link order.
         */
        *(EXCLUDE_FILE(*tlbrefill*) .text)
        *(EXCLUDE_FILE(*tlbrefill*) .text.*)
        . = ALIGN(4K);
        __text_end = .;
        __trampoline_start = .;
        KEEP(*(.trampoline.text))
        . = ALIGN(4K);
        __trampoline_end = .;
    }

    /*
     * LoongArch TLB refill handler (TLBRENTRY target). Hardware requires
     * the CSR to hold a 4 KiB-aligned PA, and the handler executes with
     * the MMU off. We pin this section to its own 4 KiB page and expose
     * `__tlbrefill_start`; Rust's Arch::init masks off the DMW1 high
     * bits and writes the resulting PA into TLBRENTRY.
     */
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
