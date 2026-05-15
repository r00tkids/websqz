// Shared AArch64 Mach-O helpers for assembly probability models.
//
// Model entrypoints that mirror Rust's Model::pred return stretched
// probabilities.  The final _websqz_model_predict callback used by decoder.s
// must squash that value to [0, 1] before returning.

.text
.align 2

.equ WEBSQZ_U24_MAX, 0x00ffffff
.equ WEBSQZ_NORDER_DATA_DEFAULT, 0x007fffff

.globl _websqz_model_hash
_websqz_model_hash:
    // uint32_t hash(uint32_t value, uint32_t shift)
    // value ^= value >> shift;
    // return (0x9E35A7BDu * value) >> shift;
    lsr     w2, w0, w1
    eor     w0, w0, w2
    movz    w2, #0xa7bd
    movk    w2, #0x9e35, lsl #16
    mul     w0, w0, w2
    lsr     w0, w0, w1
    ret

.globl _websqz_probability_from_u24
_websqz_probability_from_u24:
    // d0 = clamp((double)w0 / 0x00ffffff, eps, 1.0 - eps)
    adrp    x9, L_websqz_u24_max_double@PAGE
    add     x9, x9, L_websqz_u24_max_double@PAGEOFF
    ucvtf   d0, w0
    ldr     d1, [x9]
    fdiv    d0, d0, d1

    adrp    x9, L_websqz_prob_eps@PAGE
    add     x9, x9, L_websqz_prob_eps@PAGEOFF
    ldr     d1, [x9]
    fcmp    d0, d1
    b.ge    1f
    fmov    d0, d1
1:
    adrp    x9, L_websqz_prob_one_minus_eps@PAGE
    add     x9, x9, L_websqz_prob_one_minus_eps@PAGEOFF
    ldr     d1, [x9]
    fcmp    d0, d1
    b.le    2f
    fmov    d0, d1
2:
    ret

.globl _websqz_prob_stretch_u24
_websqz_prob_stretch_u24:
    // d0 = log(p / (1.0 - p)), where p is the clamped u24 probability.
    stp     x29, x30, [sp, #-16]!
    mov     x29, sp

    bl      _websqz_probability_from_u24
    fmov    d2, d0
    adrp    x9, L_websqz_one@PAGE
    add     x9, x9, L_websqz_one@PAGEOFF
    ldr     d1, [x9]
    fsub    d1, d1, d2
    fdiv    d0, d2, d1
    bl      _log

    ldp     x29, x30, [sp], #16
    ret

.globl _websqz_prob_squash
_websqz_prob_squash:
    // d0 = 1.0 / (1.0 + exp(-d0))
    stp     x29, x30, [sp, #-16]!
    mov     x29, sp

    fneg    d0, d0
    bl      _exp
    adrp    x9, L_websqz_one@PAGE
    add     x9, x9, L_websqz_one@PAGEOFF
    ldr     d1, [x9]
    fadd    d0, d0, d1
    fdiv    d0, d1, d0

    ldp     x29, x30, [sp], #16
    ret

.section __TEXT,__literal8,8byte_literals
.p2align 3
L_websqz_u24_max_double:
    .double 16777215.0
L_websqz_prob_eps:
    .double 0.000000059604648328104019
L_websqz_prob_one_minus_eps:
    .double 0.9999999403953517
L_websqz_one:
    .double 1.0
