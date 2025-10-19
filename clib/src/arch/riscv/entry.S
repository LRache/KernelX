.global entry

.section .text.entry

entry:
    li tp, 0
    la sp, __stack_end
    call main
.spin:
    wfi
    j .spin