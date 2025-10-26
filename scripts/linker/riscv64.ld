VIRT_BASE = 0xffffffc000000000;

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
        __text_end = .;
        . = ALIGN(4K);
        __trampoline_start = .;
        KEEP(*(.trampoline.text))
        __trampoline_end = .;
    }

    . = ALIGN(4K);
    .rodata : {
        __rodata_start = .;
        *(.got*)
        *(.rodata*)
        *(.srodata*)
        __rodata_end = .;
    }

    . = ALIGN(4K);
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
    . = . + 0x16000;
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
