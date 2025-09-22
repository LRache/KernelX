PHYS_BASE = 0xa0000000;
VIRT_BASE = 0xffffffffa0000000;

ENTRY(bootloader)

EXTERN(entry)

SECTIONS {
    . = PHYS_BASE;
    .bootloader : {
        KEEP(*(.text.bootloader))
        KEEP(*(.bss.bootloader))
    }

    . = VIRT_BASE + 0x5000;
    __kernel_start = .;
    lma = PHYS_BASE + 0x5000;

    .text : AT(lma) {
        __text_start = .;
        KEEP(*(.text.entry))
        *(.text*)
        __text_end = .;
        . = ALIGN(4K);
        __trampoline_start = .;
        KEEP(*(.trampoline.text))
        __trampoline_end = .;
    }
    lma += SIZEOF(.text);

    . = ALIGN(4K); lma = ALIGN(lma, 4K);
    .rodata : AT(lma) {
        __rodata_start = .;
        *(.got*)
        *(.rodata*)
        *(.srodata*)
        __rodata_end = .;
    }
    lma += SIZEOF(.rodata);

    . = ALIGN(4K); lma = ALIGN(lma, 4K);
    .data : AT(lma) {
        __data_start = .;
        *(.data*)
        *(.sdata*)
        __data_end = .;
    }
    lma += SIZEOF(.data);

    .bss : {
        __bss_start = .;
        *(.bss*)
        *(.sbss*)
        *(.scommon)
        __bss_end = .;
    }
    
    . = ALIGN(4K); lma = ALIGN(lma, 4K);

    __stack_start = .;
    . = . + 0x16000;
    __stack_end = .;

    . = ALIGN(4K);
    __heap_start = .;
    . = . + 0x1000000;
    __heap_end = .;

    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
    }
}
