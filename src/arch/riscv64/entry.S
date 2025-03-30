.global _entry

.section .text.entry

_entry:
    la sp, __stack_pointer
    call main
.spin:
    wfi
    j .spin
