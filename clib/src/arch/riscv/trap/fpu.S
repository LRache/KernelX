    .section .text
    .align 2

    .global asm_save_float
    .type asm_save_float, @function
asm_save_float:
    fsw ft0, 0(a0)
    fsw ft1, 8(a0)
    fsw ft2, 16(a0)
    fsw ft3, 24(a0)
    fsw ft4, 32(a0)
    fsw ft5, 40(a0)
    fsw ft6, 48(a0)
    fsw ft7, 56(a0)
    fsw fs0, 64(a0)
    fsw fs1, 72(a0)
    fsw fa0, 80(a0)
    fsw fa1, 88(a0)
    fsw fa2, 96(a0)
    fsw fa3, 104(a0)
    fsw fa4, 112(a0)
    fsw fa5, 120(a0)
    fsw fa6, 128(a0)
    fsw fa7, 136(a0)
    fsw fs2, 144(a0)
    fsw fs3, 152(a0)
    fsw fs4, 160(a0)
    fsw fs5, 168(a0)
    fsw fs6, 176(a0)
    fsw fs7, 184(a0)
    fsw fs8, 192(a0)
    fsw fs9, 200(a0)
    fsw fs10, 208(a0)
    fsw fs11, 216(a0)

    csrr t0, fcsr
    sw t0, 224(a0)
    
    ret

    .global asm_save_double
    .type asm_save_double, @function
asm_save_double:
    fsd ft0, 0(a0)
    fsd ft1, 8(a0)
    fsd ft2, 16(a0)
    fsd ft3, 24(a0)
    fsd ft4, 32(a0)
    fsd ft5, 40(a0)
    fsd ft6, 48(a0)
    fsd ft7, 56(a0)
    fsd fs0, 64(a0)
    fsd fs1, 72(a0)
    fsd fa0, 80(a0)
    fsd fa1, 88(a0)
    fsd fa2, 96(a0)
    fsd fa3, 104(a0)
    fsd fa4, 112(a0)
    fsd fa5, 120(a0)
    fsd fa6, 128(a0)
    fsd fa7, 136(a0)
    fsd fs2, 144(a0)
    fsd fs3, 152(a0)
    fsd fs4, 160(a0)
    fsd fs5, 168(a0)
    fsd fs6, 176(a0)
    fsd fs7, 184(a0)
    fsd fs8, 192(a0)
    fsd fs9, 200(a0)
    fsd fs10, 208(a0)
    fsd fs11, 216(a0)
    
    csrr t0, fcsr
    sw t0, 224(a0)
    
    ret

    .global asm_restore_float
    .type asm_restore_float, @function
asm_restore_float:
    flw ft0, 0(a0)
    flw ft1, 8(a0)
    flw ft2, 16(a0)
    flw ft3, 24(a0)
    flw ft4, 32(a0)
    flw ft5, 40(a0)
    flw ft6, 48(a0)
    flw ft7, 56(a0)
    flw fs0, 64(a0)
    flw fs1, 72(a0)
    flw fa0, 80(a0)
    flw fa1, 88(a0)
    flw fa2, 96(a0)
    flw fa3, 104(a0)
    flw fa4, 112(a0)
    flw fa5, 120(a0)
    flw fa6, 128(a0)
    flw fa7, 136(a0)
    flw fs2, 144(a0)
    flw fs3, 152(a0)
    flw fs4, 160(a0)
    flw fs5, 168(a0)
    flw fs6, 176(a0)
    flw fs7, 184(a0)
    flw fs8, 192(a0)
    flw fs9, 200(a0)
    flw fs10, 208(a0)
    flw fs11, 216(a0)
    
    ld t0, 224(a0)
    csrw fcsr, t0
    
    ret

    .global asm_restore_double
    .type asm_restore_double, @function
asm_restore_double:
    fsd ft0, 0(a0)
    fsd ft1, 8(a0)
    fsd ft2, 16(a0)
    fsd ft3, 24(a0)
    fsd ft4, 32(a0)
    fsd ft5, 40(a0)
    fsd ft6, 48(a0)
    fsd ft7, 56(a0)
    fsd fs0, 64(a0)
    fsd fs1, 72(a0)
    fsd fa0, 80(a0)
    fsd fa1, 88(a0)
    fsd fa2, 96(a0)
    fsd fa3, 104(a0)
    fsd fa4, 112(a0)
    fsd fa5, 120(a0)
    fsd fa6, 128(a0)
    fsd fa7, 136(a0)
    fsd fs2, 144(a0)
    fsd fs3, 152(a0)
    fsd fs4, 160(a0)
    fsd fs5, 168(a0)
    fsd fs6, 176(a0)
    fsd fs7, 184(a0)
    fsd fs8, 192(a0)
    fsd fs9, 200(a0)
    fsd fs10, 208(a0)
    fsd fs11, 216(a0)
    
    ld t0, 224(a0)
    csrw fcsr, t0
    
    ret
