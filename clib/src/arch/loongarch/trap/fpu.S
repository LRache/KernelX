    .section .text
    .align 2

    .global asm_save_lsx
    .type   asm_save_lsx, @function
/*
 * void asm_save_lsx(u128 *fpregs, u64 *fcc, u64 *fcsr);
 */
asm_save_lsx:
    vst     $vr0,  $a0, 0*16
    vst     $vr1,  $a0, 1*16
    vst     $vr2,  $a0, 2*16
    vst     $vr3,  $a0, 3*16
    vst     $vr4,  $a0, 4*16
    vst     $vr5,  $a0, 5*16
    vst     $vr6,  $a0, 6*16
    vst     $vr7,  $a0, 7*16
    vst     $vr8,  $a0, 8*16
    vst     $vr9,  $a0, 9*16
    vst     $vr10, $a0, 10*16
    vst     $vr11, $a0, 11*16
    vst     $vr12, $a0, 12*16
    vst     $vr13, $a0, 13*16
    vst     $vr14, $a0, 14*16
    vst     $vr15, $a0, 15*16
    vst     $vr16, $a0, 16*16
    vst     $vr17, $a0, 17*16
    vst     $vr18, $a0, 18*16
    vst     $vr19, $a0, 19*16
    vst     $vr20, $a0, 20*16
    vst     $vr21, $a0, 21*16
    vst     $vr22, $a0, 22*16
    vst     $vr23, $a0, 23*16
    vst     $vr24, $a0, 24*16
    vst     $vr25, $a0, 25*16
    vst     $vr26, $a0, 26*16
    vst     $vr27, $a0, 27*16
    vst     $vr28, $a0, 28*16
    vst     $vr29, $a0, 29*16
    vst     $vr30, $a0, 30*16
    vst     $vr31, $a0, 31*16

    movfcsr2gr $t0, $fcsr0
    st.w    $t0, $a2, 0
    st.w    $zero, $a2, 4

    movcf2gr $t0, $fcc0
    st.b    $t0, $a1, 0
    movcf2gr $t0, $fcc1
    st.b    $t0, $a1, 1
    movcf2gr $t0, $fcc2
    st.b    $t0, $a1, 2
    movcf2gr $t0, $fcc3
    st.b    $t0, $a1, 3
    movcf2gr $t0, $fcc4
    st.b    $t0, $a1, 4
    movcf2gr $t0, $fcc5
    st.b    $t0, $a1, 5
    movcf2gr $t0, $fcc6
    st.b    $t0, $a1, 6
    movcf2gr $t0, $fcc7
    st.b    $t0, $a1, 7

    ret
    .size asm_save_lsx, . - asm_save_lsx


    .global asm_restore_lsx
    .type   asm_restore_lsx, @function
/*
 * void asm_restore_lsx(const u128 *fpregs, const u64 *fcc, const u64 *fcsr);
 */
asm_restore_lsx:
    vld     $vr0,  $a0, 0*16
    vld     $vr1,  $a0, 1*16
    vld     $vr2,  $a0, 2*16
    vld     $vr3,  $a0, 3*16
    vld     $vr4,  $a0, 4*16
    vld     $vr5,  $a0, 5*16
    vld     $vr6,  $a0, 6*16
    vld     $vr7,  $a0, 7*16
    vld     $vr8,  $a0, 8*16
    vld     $vr9,  $a0, 9*16
    vld     $vr10, $a0, 10*16
    vld     $vr11, $a0, 11*16
    vld     $vr12, $a0, 12*16
    vld     $vr13, $a0, 13*16
    vld     $vr14, $a0, 14*16
    vld     $vr15, $a0, 15*16
    vld     $vr16, $a0, 16*16
    vld     $vr17, $a0, 17*16
    vld     $vr18, $a0, 18*16
    vld     $vr19, $a0, 19*16
    vld     $vr20, $a0, 20*16
    vld     $vr21, $a0, 21*16
    vld     $vr22, $a0, 22*16
    vld     $vr23, $a0, 23*16
    vld     $vr24, $a0, 24*16
    vld     $vr25, $a0, 25*16
    vld     $vr26, $a0, 26*16
    vld     $vr27, $a0, 27*16
    vld     $vr28, $a0, 28*16
    vld     $vr29, $a0, 29*16
    vld     $vr30, $a0, 30*16
    vld     $vr31, $a0, 31*16

    ld.wu   $t0, $a2, 0
    movgr2fcsr $fcsr0, $t0

    ld.bu   $t0, $a1, 0
    movgr2cf $fcc0, $t0
    ld.bu   $t0, $a1, 1
    movgr2cf $fcc1, $t0
    ld.bu   $t0, $a1, 2
    movgr2cf $fcc2, $t0
    ld.bu   $t0, $a1, 3
    movgr2cf $fcc3, $t0
    ld.bu   $t0, $a1, 4
    movgr2cf $fcc4, $t0
    ld.bu   $t0, $a1, 5
    movgr2cf $fcc5, $t0
    ld.bu   $t0, $a1, 6
    movgr2cf $fcc6, $t0
    ld.bu   $t0, $a1, 7
    movgr2cf $fcc7, $t0

    ret
    .size asm_restore_lsx, . - asm_restore_lsx
