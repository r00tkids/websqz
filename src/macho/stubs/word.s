// Thin exported aliases for NOrderByte's word-model mode.
//
// Use the same context layout as norder_byte.s, with:
//     prev_bytes    = 2166136261
//     mask          = UINT64_MAX
//     is_word_model = 1
//     magic_num     = hash(1337, 2)

.text
.align 2

.globl _rootsqz_word_predict
_rootsqz_word_predict:
    b       _rootsqz_norder_byte_predict

.globl _rootsqz_word_learn
_rootsqz_word_learn:
    b       _rootsqz_norder_byte_learn
