ENTRY(_entry)

SECTIONS {
    . = 0x80200000;
    
    .text : {
        *(.text.entry)
        *(.text*)
    }
    
    .rodata : {
        *(.rodata*)
    }
    
    .data : {
        *(.data*)
    }
    
    .bss : {
        *(.bss*)
        *(.sbss*)
        *(.scommon)
    }

    .init_array : {
        PROVIDE_HIDDEN (__init_array_start = .);
        KEEP (*(SORT_BY_INIT_PRIORITY(.init_array.*) .init_array))
        PROVIDE_HIDDEN (__init_array_end = .);
    }

    .fini_array : {
        __fini_array_start = .;
        KEEP(*(.fini_array*))
        __fini_array_end = .;
    }

    __stack_top = ALIGN(0x1000);
    . = __stack_top + 0x1000;
    __stack_pointer = .;
    __heap_start = ALIGN(0x1000);
    . = __heap_start + 0x8000;
    __heap_end = ALIGN(0x4000);

    PROVIDE(printf = tfp_printf);
}
