.global entry

.section .text.entry

entry:
    la sp, __stack_end
    call main
.spin:
    wfi
    j .spin