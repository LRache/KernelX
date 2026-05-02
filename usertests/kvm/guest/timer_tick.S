.equ SBI_EXT_TIME, 0x54494d45
.equ SBI_TIME_SET_TIMER, 0
.equ SBI_LEGACY_CONSOLE_PUTCHAR, 1
.equ SBI_LEGACY_SHUTDOWN, 8

.equ SSTATUS_SIE, (1 << 1)
.equ SIE_STIE, (1 << 5)
.equ SCAUSE_INTERRUPT_BIT, (1 << 63)
.equ SCAUSE_TIMER, 5

.equ TIMEBASE_FREQ, 10000000
.equ TRAP_FRAME_SIZE, 240

.section .text
.global _start

_start:
    la sp, stack_top

    la t0, trap_entry
    csrw stvec, t0

    la a0, boot_msg
    call puts

    call set_next_timer

    li t0, SIE_STIE
    csrs sie, t0
    li t0, SSTATUS_SIE
    csrs sstatus, t0

idle:
    wfi
    j idle

trap_entry:
    addi sp, sp, -TRAP_FRAME_SIZE
    sd ra, 0(sp)
    sd gp, 8(sp)
    sd tp, 16(sp)
    sd t0, 24(sp)
    sd t1, 32(sp)
    sd t2, 40(sp)
    sd s0, 48(sp)
    sd s1, 56(sp)
    sd a0, 64(sp)
    sd a1, 72(sp)
    sd a2, 80(sp)
    sd a3, 88(sp)
    sd a4, 96(sp)
    sd a5, 104(sp)
    sd a6, 112(sp)
    sd a7, 120(sp)
    sd s2, 128(sp)
    sd s3, 136(sp)
    sd s4, 144(sp)
    sd s5, 152(sp)
    sd s6, 160(sp)
    sd s7, 168(sp)
    sd s8, 176(sp)
    sd s9, 184(sp)
    sd s10, 192(sp)
    sd s11, 200(sp)
    sd t3, 208(sp)
    sd t4, 216(sp)
    sd t5, 224(sp)
    sd t6, 232(sp)

    csrr t0, scause
    li t1, SCAUSE_INTERRUPT_BIT
    and t2, t0, t1
    beqz t2, unexpected_trap
    xor t0, t0, t1
    li t1, SCAUSE_TIMER
    bne t0, t1, unexpected_trap

    la a0, tick_msg
    call puts
    call set_next_timer
    j trap_return

unexpected_trap:
    la a0, unexpected_msg
    call puts
    li a7, SBI_LEGACY_SHUTDOWN
    ecall
1:
    wfi
    j 1b

trap_return:
    ld ra, 0(sp)
    ld gp, 8(sp)
    ld tp, 16(sp)
    ld t0, 24(sp)
    ld t1, 32(sp)
    ld t2, 40(sp)
    ld s0, 48(sp)
    ld s1, 56(sp)
    ld a0, 64(sp)
    ld a1, 72(sp)
    ld a2, 80(sp)
    ld a3, 88(sp)
    ld a4, 96(sp)
    ld a5, 104(sp)
    ld a6, 112(sp)
    ld a7, 120(sp)
    ld s2, 128(sp)
    ld s3, 136(sp)
    ld s4, 144(sp)
    ld s5, 152(sp)
    ld s6, 160(sp)
    ld s7, 168(sp)
    ld s8, 176(sp)
    ld s9, 184(sp)
    ld s10, 192(sp)
    ld s11, 200(sp)
    ld t3, 208(sp)
    ld t4, 216(sp)
    ld t5, 224(sp)
    ld t6, 232(sp)
    addi sp, sp, TRAP_FRAME_SIZE
    sret

set_next_timer:
    csrr t0, time
    li t1, TIMEBASE_FREQ
    add a0, t0, t1
    li a6, SBI_TIME_SET_TIMER
    li a7, SBI_EXT_TIME
    ecall
    ret

puts:
    mv t0, a0
1:
    lbu a0, 0(t0)
    beqz a0, 2f
    li a6, 0
    li a7, SBI_LEGACY_CONSOLE_PUTCHAR
    ecall
    addi t0, t0, 1
    j 1b
2:
    ret

.section .rodata
boot_msg:
    .asciz "timer guest: start\n"
tick_msg:
    .asciz "tick\n"
unexpected_msg:
    .asciz "timer guest: unexpected trap\n"

.section .bss
.align 12
stack:
    .space 4096
stack_top:
