/*
 * LoongArch trap entry (shared EENTRY, dispatch by PRMD.PPLV).
 *
 * On entry:
 *   - CRMD: PLV=0, IE=0 (hardware cleared)
 *   - PRMD: previous CRMD (restored by ertn)
 *   - ERA:  PC to return to
 *
 * PRMD.PPLV non-zero → entered from user; jump to asm_usertrap_entry.
 * Otherwise save all 32 GPRs + ERA/PRMD on the existing kernel $sp
 * (reusable because PPLV=0 means $sp is already kernel-valid), call
 * `kerneltrap_handler`, restore, `ertn`.
 *
 * Frame layout (272 B, 16-byte aligned):
 *      0   ra
 *      8   (skip; pre-adjust sp — we don't need to restore it)
 *     16   tp
 *     24   a0..a7      ($r4..$r11)
 *     88   t0..t8      ($r12..$r20)
 *    160   r21 (percpu — handler reads via get_percpu_data)
 *    168   fp, s0..s8  ($r22..$r31)
 *    248   ERA (saved for future nested-exception support; handler
 *          does not touch it but symmetry is cheap)
 *    256   PRMD
 *    264   padding
 */

    .section .text.entry
    .global asm_kerneltrap_entry
    .type asm_kerneltrap_entry, @function
    .p2align 12                   /* EENTRY must be page-aligned (VS=0) */

asm_kerneltrap_entry:
    csrrd   $t0, 0x1              /* PRMD */
    andi    $t0, $t0, 0x3         /* PPLV */
    bnez    $t0, asm_usertrap_entry

    /* PLV0 path. */
    addi.d  $sp, $sp, -272

    st.d    $r1,  $sp,  0         /* ra */
    st.d    $r2,  $sp,  16        /* tp */
    st.d    $r4,  $sp,  24        /* a0..a7 */
    st.d    $r5,  $sp,  32
    st.d    $r6,  $sp,  40
    st.d    $r7,  $sp,  48
    st.d    $r8,  $sp,  56
    st.d    $r9,  $sp,  64
    st.d    $r10, $sp,  72
    st.d    $r11, $sp,  80
    st.d    $r12, $sp,  88        /* t0..t8 */
    st.d    $r13, $sp,  96
    st.d    $r14, $sp, 104
    st.d    $r15, $sp, 112
    st.d    $r16, $sp, 120
    st.d    $r17, $sp, 128
    st.d    $r18, $sp, 136
    st.d    $r19, $sp, 144
    st.d    $r20, $sp, 152
    st.d    $r21, $sp, 160        /* percpu */
    st.d    $r22, $sp, 168        /* fp */
    st.d    $r23, $sp, 176        /* s0..s8 */
    st.d    $r24, $sp, 184
    st.d    $r25, $sp, 192
    st.d    $r26, $sp, 200
    st.d    $r27, $sp, 208
    st.d    $r28, $sp, 216
    st.d    $r29, $sp, 224
    st.d    $r30, $sp, 232
    st.d    $r31, $sp, 240

    csrrd   $t0, 0x6              /* ERA */
    st.d    $t0, $sp, 248
    csrrd   $t0, 0x1              /* PRMD */
    st.d    $t0, $sp, 256

    bl      kerneltrap_handler

    ld.d    $t0, $sp, 256
    csrwr   $t0, 0x1              /* PRMD */
    ld.d    $t0, $sp, 248
    csrwr   $t0, 0x6              /* ERA */

    ld.d    $r1,  $sp,  0
    ld.d    $r2,  $sp,  16
    ld.d    $r4,  $sp,  24
    ld.d    $r5,  $sp,  32
    ld.d    $r6,  $sp,  40
    ld.d    $r7,  $sp,  48
    ld.d    $r8,  $sp,  56
    ld.d    $r9,  $sp,  64
    ld.d    $r10, $sp,  72
    ld.d    $r11, $sp,  80
    ld.d    $r12, $sp,  88
    ld.d    $r13, $sp,  96
    ld.d    $r14, $sp, 104
    ld.d    $r15, $sp, 112
    ld.d    $r16, $sp, 120
    ld.d    $r17, $sp, 128
    ld.d    $r18, $sp, 136
    ld.d    $r19, $sp, 144
    ld.d    $r20, $sp, 152
    ld.d    $r21, $sp, 160
    ld.d    $r22, $sp, 168
    ld.d    $r23, $sp, 176
    ld.d    $r24, $sp, 184
    ld.d    $r25, $sp, 192
    ld.d    $r26, $sp, 200
    ld.d    $r27, $sp, 208
    ld.d    $r28, $sp, 216
    ld.d    $r29, $sp, 224
    ld.d    $r30, $sp, 232
    ld.d    $r31, $sp, 240

    addi.d  $sp, $sp, 272
    ertn

    .size asm_kerneltrap_entry, . - asm_kerneltrap_entry
