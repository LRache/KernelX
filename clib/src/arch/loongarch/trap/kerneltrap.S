/*
 * LoongArch kernel-trap entry.
 *
 * EENTRY points here. On entry:
 *   - CRMD: PLV=0 (kernel), IE=0 (hardware cleared)
 *   - PRMD: previous CRMD (we'll copy it back on ertn)
 *   - ERA:  PC to return to
 *   - $sp:  whatever the interrupted kernel context had — we reuse it, this
 *           entry is for PLV0 traps only so $sp is already kernel-valid.
 *   - all other GPRs: the interrupted code's values
 *
 * We want to invoke a C-ABI Rust function (`kerneltrap_handler`). That means
 * we must save every caller-saved GPR (the handler can clobber them), plus a
 * handful of callee-saved ones that Rust could also clobber if we were
 * unlucky. Easiest: save all 32 GPRs + ERA + PRMD, call the handler, reload
 * them, `ertn`.
 *
 * We allocate the save frame on the existing $sp. 34 slots * 8 bytes = 272
 * bytes. The ABI wants 16-byte alignment, which 272 already satisfies.
 *
 * Frame layout (offsets from post-adjust $sp):
 *      0   $ra
 *      8   (skip: old $sp was in use, restored from $r1-adjacent area? NO —
 *           we save a COPY of sp for completeness even though it's unchanged)
 *     16   $r2 (tp)          - user-thread pointer; unused in kernel but
 *                              preserved to avoid leaking register state
 *     24   $r4  (a0)         - handler args; we set a0=0 in the Rust call
 *     32   $r5  (a1)
 *     40   $r6  (a2)
 *     48   $r7  (a3)
 *     56   $r8  (a4)
 *     64   $r9  (a5)
 *     72   $r10 (a6)
 *     80   $r11 (a7)
 *     88   $r12 (t0)
 *     96   $r13 (t1)
 *    104   $r14 (t2)
 *    112   $r15 (t3)
 *    120   $r16 (t4)
 *    128   $r17 (t5)
 *    136   $r18 (t6)
 *    144   $r19 (t7)
 *    152   $r20 (t8)
 *    160   $r21 (percpu)     - must preserve: handler reads it via get_percpu_data
 *    168   $r22 (fp)
 *    176   $r23 (s0)
 *    184   $r24 (s1)
 *    192   $r25 (s2)
 *    200   $r26 (s3)
 *    208   $r27 (s4)
 *    216   $r28 (s5)
 *    224   $r29 (s6)
 *    232   $r30 (s7)
 *    240   $r31 (s8)
 *    248   ERA  (save/restore)
 *    256   PRMD (save/restore)
 *    264   <padding to 272 = 17*16>
 *
 * We save ERA/PRMD because a nested exception (we won't allow one, but be
 * defensive) or a handler that tail-calls something expecting fresh CSR
 * state could otherwise clobber them. Phase 5 will actually need these for
 * user-trap re-entry.
 */

    .section .text.entry
    .global asm_kerneltrap_entry
    .type asm_kerneltrap_entry, @function
    .p2align 12                   /* EENTRY must be page-aligned (VS=0) */

asm_kerneltrap_entry:
    addi.d  $sp, $sp, -272

    st.d    $r1,  $sp,  0         /* ra   */
    /* skip slot 8 for the pre-adjust sp we already burned */
    st.d    $r2,  $sp,  16        /* tp   */
    st.d    $r4,  $sp,  24        /* a0   */
    st.d    $r5,  $sp,  32
    st.d    $r6,  $sp,  40
    st.d    $r7,  $sp,  48
    st.d    $r8,  $sp,  56
    st.d    $r9,  $sp,  64
    st.d    $r10, $sp,  72
    st.d    $r11, $sp,  80
    st.d    $r12, $sp,  88
    st.d    $r13, $sp,  96
    st.d    $r14, $sp, 104
    st.d    $r15, $sp, 112
    st.d    $r16, $sp, 120
    st.d    $r17, $sp, 128
    st.d    $r18, $sp, 136
    st.d    $r19, $sp, 144
    st.d    $r20, $sp, 152
    st.d    $r21, $sp, 160        /* percpu — must preserve */
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

    /* Save ERA and PRMD. The handler won't touch them, but a Phase 5 nested
     * trap (impossible for us with IE=0 but cheap to future-proof) would. */
    csrrd   $t0, 0x6              /* ERA  */
    st.d    $t0, $sp, 248
    csrrd   $t0, 0x1              /* PRMD */
    st.d    $t0, $sp, 256

    bl      kerneltrap_handler

    /* Restore ERA/PRMD first — handler may have poked them (it won't, but
     * symmetry). */
    ld.d    $t0, $sp, 256
    csrwr   $t0, 0x1              /* PRMD */
    ld.d    $t0, $sp, 248
    csrwr   $t0, 0x6              /* ERA  */

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
