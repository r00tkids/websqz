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
    cbnz    w0, 1f
    mov     w0, #1
    b       2f
1:
    movz    w9, #0xffff
    movk    w9, #0x00ff, lsl #16
    cmp     w0, w9
    b.ne    2f
    sub     w0, w9, #1
2:
    adrp    x9, _websqz_u24_max_double@PAGE
    add     x9, x9, _websqz_u24_max_double@PAGEOFF
    ucvtf   d0, w0
    ldr     d1, [x9]
    fdiv    d0, d0, d1
    ret

.globl _websqz_prob_stretch_u24
_websqz_prob_stretch_u24:
    // d0 = log(p / (1.0 - p)), where p is the clamped u24 probability.
    stp     x29, x30, [sp, #-16]!
    mov     x29, sp

    bl      _websqz_probability_from_u24
    fmov    d2, d0
    fmov    d1, #1.00000000
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
    fmov    d1, #1.00000000
    fadd    d0, d0, d1
    fdiv    d0, d1, d0

    ldp     x29, x30, [sp], #16
    ret

.section __TEXT,__literal8,8byte_literals
.p2align 3
.globl _websqz_u24_max_double
_websqz_u24_max_double:
    .double 16777215.0
