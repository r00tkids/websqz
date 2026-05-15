// Thin exported aliases for NOrderByte's word-model mode.
//
// Use the same context layout as norder_byte.s, with:
//     prev_bytes    = 2166136261
//     mask          = UINT64_MAX
//     is_word_model = 1
//     magic_num     = hash(1337, 2)

.text
.align 2

.globl _websqz_word_predict
_websqz_word_predict:
    b       _websqz_norder_byte_predict

.globl _websqz_word_learn
_websqz_word_learn:
    b       _websqz_norder_byte_learn
