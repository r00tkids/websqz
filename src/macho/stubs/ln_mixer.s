// AArch64 implementation of src/compressor/model.rs LnMixerPred.
//
// Context layout:
//     uint32_t num_models;
//     uint32_t bit_ctx;
//     uint32_t prev_byte;
//     uint32_t pad;
//     double last_total_p;
//     void **model_contexts;
//     double (**predict_fns)(void *);       // Rust Model::pred, stretched
//     void (**learn_fns)(void *, uint32_t);
//     double *base_weights;                 // num_models entries
//     double *ctx_weights;                  // 256 * 255 * num_models entries
//     uint8_t *ctx_initialized;             // 256 * 255 entries
//     double *last_p;                       // num_models entries

.text
.align 2

.equ LNM_NUM_MODELS,      0
.equ LNM_BIT_CTX,         4
.equ LNM_PREV_BYTE,       8
.equ LNM_LAST_TOTAL_P,   16
.equ LNM_MODEL_CONTEXTS, 24
.equ LNM_PREDICT_FNS,    32
.equ LNM_LEARN_FNS,      40
.equ LNM_BASE_WEIGHTS,   48
.equ LNM_CTX_WEIGHTS,    56
.equ LNM_CTX_INIT,       64
.equ LNM_LAST_P,         72
.equ LNM_SIZE,           80

.globl _rootsqz_ln_mixer_predict_stretched
_rootsqz_ln_mixer_predict_stretched:
    stp     x29, x30, [sp, #-32]!
    mov     x29, sp
    str     x19, [sp, #16]
    mov     x19, x0

    bl      _rootsqz_ln_mixer_predict_sum
    str     d0, [sp, #24]
    bl      _rootsqz_prob_squash
    str     d0, [x19, #LNM_LAST_TOTAL_P]
    ldr     d0, [sp, #24]

    ldr     x19, [sp, #16]
    ldp     x29, x30, [sp], #32
    ret

_rootsqz_ln_mixer_predict_sum:
    stp     x29, x30, [sp, #-112]!
    mov     x29, sp
    stp     x19, x20, [sp, #16]
    stp     x21, x22, [sp, #32]
    stp     x23, x24, [sp, #48]
    stp     x25, x26, [sp, #64]
    stp     x27, x28, [sp, #80]

    mov     x19, x0
    ldr     w20, [x19, #LNM_NUM_MODELS]
    ldr     x21, [x19, #LNM_MODEL_CONTEXTS]
    ldr     x22, [x19, #LNM_PREDICT_FNS]
    ldr     x23, [x19, #LNM_BASE_WEIGHTS]
    ldr     x24, [x19, #LNM_CTX_WEIGHTS]
    ldr     x25, [x19, #LNM_CTX_INIT]
    ldr     x26, [x19, #LNM_LAST_P]

    ldr     w9, [x19, #LNM_PREV_BYTE]
    ldr     w10, [x19, #LNM_BIT_CTX]
    sub     w10, w10, #1
    mov     w11, #255
    madd    w9, w9, w11, w10       // row = prev_byte * 255 + bit_ctx - 1
    ldrb    w28, [x25, x9]
    umull   x10, w9, w20
    add     x24, x24, x10, lsl #3  // current ctx weight row

    fmov    d0, xzr
    str     d0, [sp, #96]          // sum
    mov     x27, #0

1:
    cmp     x27, x20
    b.hs    3f

    ldr     x0, [x21, x27, lsl #3]
    ldr     x9, [x22, x27, lsl #3]
    blr     x9

    str     d0, [x26, x27, lsl #3]
    ldr     d1, [x23, x27, lsl #3]
    cbz     w28, 2f
    ldr     d2, [x24, x27, lsl #3]
    adrp    x9, L_rootsqz_ln_mixer_ctx_weight_scale@PAGE
    add     x9, x9, L_rootsqz_ln_mixer_ctx_weight_scale@PAGEOFF
    ldr     d3, [x9]
    fmadd   d1, d2, d3, d1
2:
    fmul    d0, d0, d1
    ldr     d4, [sp, #96]
    fadd    d4, d4, d0
    str     d4, [sp, #96]

    add     x27, x27, #1
    b       1b

3:
    ldr     d0, [sp, #96]
    ldp     x27, x28, [sp, #80]
    ldp     x25, x26, [sp, #64]
    ldp     x23, x24, [sp, #48]
    ldp     x21, x22, [sp, #32]
    ldp     x19, x20, [sp, #16]
    ldp     x29, x30, [sp], #112
    ret

.globl _rootsqz_ln_mixer_learn
_rootsqz_ln_mixer_learn:
    stp     x29, x30, [sp, #-128]!
    mov     x29, sp
    stp     x19, x20, [sp, #16]
    stp     x21, x22, [sp, #32]
    stp     x23, x24, [sp, #48]
    stp     x25, x26, [sp, #64]
    stp     x27, x28, [sp, #80]

    mov     x19, x0
    and     w28, w1, #1
    ldr     w20, [x19, #LNM_NUM_MODELS]
    ldr     x21, [x19, #LNM_MODEL_CONTEXTS]
    ldr     x22, [x19, #LNM_LEARN_FNS]
    ldr     x23, [x19, #LNM_BASE_WEIGHTS]
    ldr     x24, [x19, #LNM_CTX_WEIGHTS]
    ldr     x25, [x19, #LNM_CTX_INIT]
    ldr     x26, [x19, #LNM_LAST_P]

    ldr     w9, [x19, #LNM_PREV_BYTE]
    ldr     w10, [x19, #LNM_BIT_CTX]
    sub     w10, w10, #1
    mov     w11, #255
    madd    w9, w9, w11, w10       // row
    mov     w10, w9
    umull   x11, w9, w20
    add     x24, x24, x11, lsl #3  // current ctx weight row

    ldrb    w11, [x25, x10]
    cbnz    w11, 2f
    mov     x27, #0
1:
    cmp     x27, x20
    b.hs    11f
    ldr     d0, [x23, x27, lsl #3]
    str     d0, [x24, x27, lsl #3]
    add     x27, x27, #1
    b       1b
11:
    mov     w11, #1
    strb    w11, [x25, x10]

2:
    ucvtf   d0, w28
    ldr     d1, [x19, #LNM_LAST_TOTAL_P]
    fsub    d0, d0, d1
    str     d0, [sp, #96]          // pred_err

    mov     x27, #0
3:
    cmp     x27, x20
    b.hs    4f

    ldr     x0, [x21, x27, lsl #3]
    mov     w1, w28
    ldr     x9, [x22, x27, lsl #3]
    blr     x9

    ldr     d0, [sp, #96]
    ldr     d1, [x26, x27, lsl #3]
    fmul    d0, d0, d1             // pred_err * last_p[i]

    adrp    x9, L_rootsqz_ln_mixer_learning_rate@PAGE
    add     x9, x9, L_rootsqz_ln_mixer_learning_rate@PAGEOFF
    ldr     d2, [x9]
    ldr     d3, [x23, x27, lsl #3]
    fmul    d4, d0, d2
    fadd    d3, d3, d4
    str     d3, [x23, x27, lsl #3]

    adrp    x9, L_rootsqz_ln_mixer_learning_rate_ctx@PAGE
    add     x9, x9, L_rootsqz_ln_mixer_learning_rate_ctx@PAGEOFF
    ldr     d2, [x9]
    ldr     d3, [x24, x27, lsl #3]
    fmul    d4, d0, d2
    fadd    d3, d3, d4
    str     d3, [x24, x27, lsl #3]

    add     x27, x27, #1
    b       3b

4:
    ldr     w9, [x19, #LNM_BIT_CTX]
    lsl     w9, w9, #1
    orr     w9, w9, w28
    cmp     w9, #256
    b.hs    5f
    str     w9, [x19, #LNM_BIT_CTX]
    b       6f
5:
    and     w9, w9, #0xff
    str     w9, [x19, #LNM_PREV_BYTE]
    mov     w9, #1
    str     w9, [x19, #LNM_BIT_CTX]

6:
    ldp     x27, x28, [sp, #80]
    ldp     x25, x26, [sp, #64]
    ldp     x23, x24, [sp, #48]
    ldp     x21, x22, [sp, #32]
    ldp     x19, x20, [sp, #16]
    ldp     x29, x30, [sp], #128
    ret

.section __TEXT,__literal8,8byte_literals
.p2align 3
L_rootsqz_ln_mixer_ctx_weight_scale:
    .double 0.3
L_rootsqz_ln_mixer_learning_rate:
    .double 0.0004
L_rootsqz_ln_mixer_learning_rate_ctx:
    .double 0.022
