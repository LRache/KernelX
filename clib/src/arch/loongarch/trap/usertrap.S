/*
 * LoongArch user-trap entry / return.
 *
 * Entry: reached from asm_kerneltrap_entry when PRMD.PPLV != 0.
 *   1. `csrrd $t0, SAVE0` — SAVE0 was set by return_to_user to
 *      &UserContext, so $t0 now points at it.
 *   2. Save $r1..$r31 into UserContext.gpr[1..32] (skip $r0; skip $r12
 *      which IS $t0 — recorded as 0, the user's $t0 is lost on the fast
 *      path; Linux does the same).
 *   3. Load kernel_sp and kernel_percpu from UserContext ($r21 is the
 *      per-CPU pointer; user code may have clobbered it).
 *   4. Call usertrap_handler() → tail-calls return_to_user() →
 *      asm_usertrap_return.
 *
 * Return (asm_usertrap_return): caller (return_to_user) has already
 * programmed SAVE0 / PGDL / ERA / PRMD and flushed the TLB. We restore
 * GPRs from UserContext and `ertn`.
 *
 * UserContext layout (src/arch/loongarch/context.rs, #[repr(C)]):
 *     byte off  field
 *     0..256    gpr[0..32]
 *     256       kernel_sp
 *     264       kernel_percpu
 *     272       user_pgd
 *     280       kernel_pgd
 *     288       usertrap_handler
 *     296..     fpregs / fcc / fcsr / user_entry / fpregs_dirty
 */

    .section .text
    .global asm_usertrap_entry
    .type   asm_usertrap_entry, @function
    .global asm_usertrap_return
    .type   asm_usertrap_return, @function

asm_usertrap_entry:
    csrrd   $t0, 0x30                /* $t0 = &UserContext (from SAVE0) */

    /* Save $r1..$r31 (skip $r0 and $r12/$t0). */
    st.d    $r1,  $t0,   8
    st.d    $r2,  $t0,  16
    st.d    $r3,  $t0,  24         /* $r3 = sp */
    st.d    $r4,  $t0,  32
    st.d    $r5,  $t0,  40
    st.d    $r6,  $t0,  48
    st.d    $r7,  $t0,  56
    st.d    $r8,  $t0,  64
    st.d    $r9,  $t0,  72
    st.d    $r10, $t0,  80
    st.d    $r11, $t0,  88
    st.d    $r13, $t0, 104
    st.d    $r14, $t0, 112
    st.d    $r15, $t0, 120
    st.d    $r16, $t0, 128
    st.d    $r17, $t0, 136
    st.d    $r18, $t0, 144
    st.d    $r19, $t0, 152
    st.d    $r20, $t0, 160
    st.d    $r21, $t0, 168
    st.d    $r22, $t0, 176
    st.d    $r23, $t0, 184
    st.d    $r24, $t0, 192
    st.d    $r25, $t0, 200
    st.d    $r26, $t0, 208
    st.d    $r27, $t0, 216
    st.d    $r28, $t0, 224
    st.d    $r29, $t0, 232
    st.d    $r30, $t0, 240
    st.d    $r31, $t0, 248

    /* User's $t0 on the fast path is lost. Record 0 so the slot is defined. */
    st.d    $zero, $t0, 96

    /* Switch to kernel stack (UserContext.kernel_sp @ 256). */
    ld.d    $sp, $t0, 256

    /* Restore kernel $r21 (per-CPU pointer) from kernel_percpu @ 264.
     * User code was free to clobber $r21; without this, any code reading
     * per-CPU data via $r21 would dereference user garbage. */
    ld.d    $r21, $t0, 264

    bl      usertrap_handler
    /* usertrap_handler does not return. */
1:  idle    0
    b       1b
    .size asm_usertrap_entry, . - asm_usertrap_entry


/*
 * fn asm_usertrap_return(user_context: *const UserContext) -> !;
 * $a0 = &UserContext.
 *
 * Caller has already written CSR.{SAVE0, PGDL, ERA, PRMD} and invtlb'd.
 * We restore GPRs and `ertn`. `ertn` atomically does:
 *   CRMD <- PRMD  (PLV=3, IE=1)
 *   PC   <- ERA
 */
asm_usertrap_return:
    move    $t0, $a0                /* $t0 = &UserContext */

    ld.d    $r1,  $t0,   8
    ld.d    $r2,  $t0,  16
    ld.d    $r3,  $t0,  24
    ld.d    $r4,  $t0,  32
    ld.d    $r5,  $t0,  40
    ld.d    $r6,  $t0,  48
    ld.d    $r7,  $t0,  56
    ld.d    $r8,  $t0,  64
    ld.d    $r9,  $t0,  72
    ld.d    $r10, $t0,  80
    ld.d    $r11, $t0,  88
    ld.d    $r13, $t0, 104
    ld.d    $r14, $t0, 112
    ld.d    $r15, $t0, 120
    ld.d    $r16, $t0, 128
    ld.d    $r17, $t0, 136
    ld.d    $r18, $t0, 144
    ld.d    $r19, $t0, 152
    ld.d    $r20, $t0, 160
    ld.d    $r21, $t0, 168
    ld.d    $r22, $t0, 176
    ld.d    $r23, $t0, 184
    ld.d    $r24, $t0, 192
    ld.d    $r25, $t0, 200
    ld.d    $r26, $t0, 208
    ld.d    $r27, $t0, 216
    ld.d    $r28, $t0, 224
    ld.d    $r29, $t0, 232
    ld.d    $r30, $t0, 240
    ld.d    $r31, $t0, 248

    /* $t0 last. */
    ld.d    $t0, $t0, 96

    ertn
    .size asm_usertrap_return, . - asm_usertrap_return
