    .global __vdso_start
    .global __vdso_end

__vdso_start:
    .incbin VDSO
__vdso_end:
