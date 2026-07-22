/*
 * Cooperative kernel-context switch for LoongArch.
 *
 *   void asm_kernel_switch(KernelContext *from, KernelContext *to);
 *                                              ^a0=$r4      ^a1=$r5
 *
 * KernelContext layout (src/arch/loongarch/context.rs):
 *      0   ra       ($r1)
 *      8   sp       ($r3)
 *     16   fp       ($r22)
 *     24   s0..s8   ($r23..$r31)
 *     96   a0       ($r4)          -- loaded last so to->a0 survives
 *
 * Save callee-saved on `from`, load them from `to`, `jr $ra`.
 */

    .section .text
    .global asm_kernel_switch
    .type asm_kernel_switch, @function

asm_kernel_switch:
    st.d    $r1,  $r4,   0
    st.d    $r3,  $r4,   8
    st.d    $r22, $r4,  16
    st.d    $r23, $r4,  24
    st.d    $r24, $r4,  32
    st.d    $r25, $r4,  40
    st.d    $r26, $r4,  48
    st.d    $r27, $r4,  56
    st.d    $r28, $r4,  64
    st.d    $r29, $r4,  72
    st.d    $r30, $r4,  80
    st.d    $r31, $r4,  88

    ld.d    $r1,  $r5,   0
    ld.d    $r3,  $r5,   8
    ld.d    $r22, $r5,  16
    ld.d    $r23, $r5,  24
    ld.d    $r24, $r5,  32
    ld.d    $r25, $r5,  40
    ld.d    $r26, $r5,  48
    ld.d    $r27, $r5,  56
    ld.d    $r28, $r5,  64
    ld.d    $r29, $r5,  72
    ld.d    $r30, $r5,  80
    ld.d    $r31, $r5,  88

    ld.d    $r4,  $r5,  96        /* to->a0 last, overwriting the caller-arg slot */

    jr      $r1

    .size asm_kernel_switch, . - asm_kernel_switch
