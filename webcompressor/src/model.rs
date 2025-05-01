use std::{cell::RefCell, rc::Rc};

// Only predicts next bit based on similar bit patterns (e.g. without context)
pub struct NOrderPred {
    ctx: usize,
    count: Vec<[u32; 2]>,
    hash_size: usize,
}

impl NOrderPred {
    pub fn new(byte_window: usize, hash_size: usize) -> Self {
        let hash_size = hash_size;
        let context_size = 1 << (byte_window * hash_size);

        Self {
            ctx: 1,
            hash_size: hash_size,
            count: vec![[1; 2]; context_size],
        }
    }

    pub fn prob(&self) -> u32 {
        (0x10000 * (self.count[self.ctx][1])) / (self.count[self.ctx][1] + self.count[self.ctx][0])
    }

    pub fn update(&mut self, bit: u8) {
        // Update count
        self.count[self.ctx][bit as usize] += 1;
        if self.count[self.ctx][bit as usize] > 0xfffe {
            // Rescale count
            self.count[self.ctx][0] >>= 1;
            self.count[self.ctx][1] >>= 1;
        }

        self.ctx = if self.hash_size == 8 {
            let mut n_ctx = ((self.ctx << 1) | bit as usize) % self.count.len();
            // if n_ctx >= self.count.len() {
            //     // We overflowed
            //     n_ctx = bit as usize;
            // }
            n_ctx
        } else {
            (self.ctx * (3 << self.hash_size) + bit as usize) % self.count.len()
        };
    }
}

#[derive(Clone, Copy)]
struct NOrderBytePredData {
    data: u32,
}

impl Default for NOrderBytePredData {
    fn default() -> Self {
        Self {
            // Start with half probability
            data: 0xFFFFFF / 2,
        }
    }
}

impl NOrderBytePredData {
    fn count(&self) -> u32 {
        (self.data & 0xFF000000) >> 24
    }

    fn prob(&self) -> i32 {
        (self.data & 0xFFFFFF) as i32
    }

    fn set_count(&mut self, new_count: u32) {
        self.data = ((new_count << 24) & 0xFF000000) | self.data & 0xFFFFFF;
    }

    fn set_prob(&mut self, new_prob: i32) {
        self.data = (new_prob as u32 & 0xFFFFFF) | (self.data & 0xFF000000);
    }
}

pub struct HashTable {
    table: Vec<[NOrderBytePredData; 256]>,
    hash_shift: u32,
}

fn hash(mut value: u32, shift: u32) -> u32 {
    const K_MUL: u32 = 0x1e35a7bd;
    value ^= value >> shift;
    K_MUL.wrapping_mul(value) >> shift
}

impl HashTable {
    pub fn new(pow2_size: u32) -> Self {
        let context_size = (1 << pow2_size) as usize;
        println!(
            "Hash Table Size: {} MiB, hash_shift: {}",
            (256 * 4 * context_size) / (1024 * 1024),
            32 - pow2_size
        );
        Self {
            table: vec![[NOrderBytePredData::default(); 256]; context_size],
            hash_shift: 32 - pow2_size,
        }
    }

    pub fn hash(&self, value: u32) -> u32 {
        hash(value, self.hash_shift) as u32
    }

    pub fn get<'a>(&'a self, key: u32, bit_ctx: u32) -> &'a NOrderBytePredData {
        &self.table[key as usize][bit_ctx as usize]
    }

    pub fn get_mut<'a>(&'a mut self, key: u32, bit_ctx: u32) -> &'a mut NOrderBytePredData {
        &mut self.table[key as usize][bit_ctx as usize]
    }
}

pub struct NOrderBytePred {
    ctx: u32,
    hash_table: Rc<RefCell<HashTable>>,
    max_count: u32,

    magic_num: u32,
    prev_bytes: u64,
    mask: u32,

    bit_ctx: u32,
}

impl NOrderBytePred {
    pub fn new(byte_window: u32, hash_table: Rc<RefCell<HashTable>>, max_count: u32) -> Self {
        assert!(max_count <= 255);

        Self {
            ctx: 0,
            bit_ctx: 0,
            magic_num: hash(byte_window, 2),
            max_count: max_count,
            hash_table: hash_table,
            prev_bytes: 0,
            mask: (1 << (byte_window * 8)) - 1,
        }
    }

    pub fn prob(&self) -> u32 {
        self.hash_table.borrow().get(self.ctx, self.bit_ctx).prob() as u32
    }

    pub fn update(&mut self, bit: u8) {
        {
            let mut hash_table = self.hash_table.borrow_mut();
            let inst = hash_table.get_mut(self.ctx, self.bit_ctx);

            let (mut count, mut prob) = (inst.count(), inst.prob());
            if count < self.max_count {
                count += 1;
            }

            // Learning function
            prob += (0xffffff * bit as i32 - prob as i32) / (count + 1) as i32;
            inst.set_count(count);
            inst.set_prob(prob);
        }

        self.bit_ctx = (self.bit_ctx << 1) | bit as u32;
        if self.bit_ctx >= 256 {
            // Remove the extra leading bit before using it in the ctx
            self.bit_ctx &= 0xff;
            self.prev_bytes = ((self.prev_bytes << 8) | self.bit_ctx as u64) & self.mask as u64;

            self.ctx = self.hash_table.borrow().hash(
                (hash((self.prev_bytes >> 32) as u32, 2)
                    .wrapping_mul(self.magic_num.wrapping_mul(3))
                    + hash(self.prev_bytes as u32, 2))
                .wrapping_mul(self.magic_num.wrapping_mul(9)),
            );

            //self.ctx = ((self.ctx << 8) | self.bit_ctx as usize) % self.model.len();

            // Reset bit_ctx
            self.bit_ctx = 1;
        }
    }
}

pub struct MixerPred {
    model_5: NOrderBytePred,
    model_6: NOrderBytePred,
    model_7: NOrderBytePred,
    model_8: NOrderBytePred,
    model_9: NOrderBytePred,

    bit_count: usize,
}

impl MixerPred {
    pub fn new() -> Self {
        let hash_table = Rc::new(RefCell::new(HashTable::new(19)));
        Self {
            model_5: NOrderBytePred::new(0, hash_table.clone(), 10),
            model_6: NOrderBytePred::new(1, hash_table.clone(), 10),
            model_7: NOrderBytePred::new(2, hash_table.clone(), 3),
            model_8: NOrderBytePred::new(3, hash_table.clone(), 2),
            model_9: NOrderBytePred::new(4, hash_table.clone(), 2),
            bit_count: 0,
        }
    }

    pub fn prob(&self) -> u32 {
        let mut weights = 0.;
        // let mut sum = 0.1 * (self.model_1.prob() as f64 / 0xffff as f64);
        // weights += 0.1;
        // sum += 20.8 * (self.model_2.prob() as f64 / 0xffff as f64);
        // weights += 20.8;

        // if self.bit_count > 8 * 1024 {
        //     let m3_prob = self.model_3.prob();
        //     //if m3_prob != 0x8000 {
        //     sum += 0.00 * (m3_prob as f64 / 0xffff as f64);
        //     weights += 0.00;
        //     //}
        //     sum += 0.15 * (self.model_4.prob() as f64 / 0xffff as f64);
        //     weights += 0.15;
        // }

        // sum /= weights;
        // (sum * 0x10000 as f64) as u32

        if self.bit_count < 19900 {
            self.model_5.prob()
        } else {
            (self.model_8.prob() + self.model_7.prob() + self.model_6.prob()) / 3
            // } else {
            //     if self.bit_count > 260000 {
            //         (self.model_8.prob() + self.model_7.prob() + self.model_6.prob()) >> 1
            //     } else {
            //         self.model_6.prob()
            //     }
            // }
        }
    }

    pub fn update(&mut self, bit: u8) {
        self.bit_count += 1;
        // self.model_1.update(bit);
        // self.model_2.update(bit);
        // self.model_3.update(bit);
        // self.model_4.update(bit);
        //self.model_2.update(bit);
        self.model_5.update(bit);
        self.model_6.update(bit);
        self.model_7.update(bit);
        self.model_8.update(bit);
        self.model_9.update(bit);
    }
}

struct ModelWithWeight {
    model: NOrderBytePred,
    weight: f64,
}

pub struct LnMixerPred {
    models_with_weight: Vec<NOrderBytePred>,
}

// impl LnMixerPred {
//     pub fn new() -> Self {
//         Self {
//             models_with_weight: vec![
//                 ModelWithWeight {
//                     model: NOrderBytePred::new(1, 8, 4),
//                     weight: 0.,
//                 },
//                 ModelWithWeight {
//                     model: NOrderBytePred::new(2, 8, 3),
//                     weight: 0,
//                 },
//                 ModelWithWeight {
//                     model: NOrderBytePred::new(3, 6, 3),
//                     weight: 0,
//                 },
//             ],
//         }
//     }

//     pub fn prob(&self) -> u32 {}

//     pub fn update(&mut self, bit: u8) {
//         self.bit_count += 1;
//         for model in self.models {
//             model.update(bit);
//         }
//     }
// }
