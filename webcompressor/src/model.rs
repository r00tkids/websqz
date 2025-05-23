use crate::utils::U24_MAX;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy)]
struct NOrderBytePredData(u32);

impl Default for NOrderBytePredData {
    fn default() -> Self {
        // Start with half probability
        Self(U24_MAX >> 1)
    }
}

impl NOrderBytePredData {
    fn count(&self) -> u32 {
        (self.0 & 0xFF000000) >> 24
    }

    fn prob(&self) -> i32 {
        (self.0 & U24_MAX) as i32
    }

    fn set_count(&mut self, new_count: u32) {
        self.0 = ((new_count << 24) & 0xFF000000) | self.0 & U24_MAX;
    }

    fn set_prob(&mut self, new_prob: i32) {
        self.0 = (new_prob as u32 & U24_MAX) | (self.0 & 0xFF000000);
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
            "Hash table Size: {} MiB",
            (256 * 4 * context_size) / (1024 * 1024)
        );
        Self {
            table: vec![[NOrderBytePredData::default(); 256]; context_size],
            hash_shift: 32 - pow2_size,
        }
    }

    pub fn hash(&self, value: u32) -> u32 {
        hash(value, self.hash_shift) as u32
    }

    pub fn len(&self) -> usize {
        self.table.len()
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
    mask: u64,

    bit_ctx: u32,
}

impl NOrderBytePred {
    pub fn new(byte_mask: u8, hash_table: Rc<RefCell<HashTable>>, max_count: u32) -> Self {
        assert!(max_count <= 255);

        let mut bit_mask: u64 = 0;
        for i in 0..8 {
            bit_mask |= ((byte_mask >> i) & 1) as u64 * (0xff << (i * 8));
        }

        Self {
            ctx: 0,
            bit_ctx: 1,
            magic_num: hash(byte_mask as u32, 2).wrapping_mul(3),
            max_count: max_count,
            hash_table: hash_table,
            prev_bytes: 0,
            mask: bit_mask,
        }
    }

    pub fn prob(&self) -> Option<u32> {
        let entry = self.hash_table.borrow().get(self.ctx, self.bit_ctx).clone();
        if entry.count() == 0 {
            return None;
        }

        Some(entry.prob() as u32)
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
            prob += (U24_MAX as f64
                * ((bit as f64 - (prob as f64 / U24_MAX as f64)) / (count as f64 + 0.1)))
                as i32;
            inst.set_count(count);
            inst.set_prob(prob);
        }

        self.bit_ctx = (self.bit_ctx << 1) | bit as u32;
        if self.bit_ctx >= 256 {
            self.bit_ctx &= 0xff;

            self.prev_bytes = ((self.prev_bytes << 8) | self.bit_ctx as u64) & self.mask;
            // Remove the extra leading bit before using it in the ctx
            let new_c_hash = (hash((self.prev_bytes >> 32) as u32, 3)
                .wrapping_mul(9)
                .wrapping_add(hash(self.prev_bytes as u32, 3)))
            .wrapping_mul(self.magic_num);
            self.ctx = new_c_hash % self.hash_table.borrow().len() as u32;

            // Reset bit_ctx
            self.bit_ctx = 1;
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelDef {
    pub byte_mask: u8,
    pub weight: f64,
}

pub struct ModelWithWeight {
    pub model: NOrderBytePred,
    pub weight: f64,
}

pub struct LnMixerPred {
    pub models_with_weight: Vec<ModelWithWeight>,
    last_stretched_p: Vec<Option<f64>>,
}

impl LnMixerPred {
    pub fn new(model_defs: &Vec<ModelDef>) -> Self {
        let hash_table = Rc::new(RefCell::new(HashTable::new(19)));

        let mut models_with_weight = Vec::new();
        for model_def in model_defs {
            models_with_weight.push(ModelWithWeight {
                model: NOrderBytePred::new(model_def.byte_mask, hash_table.clone(), 255),
                weight: model_def.weight,
            });
        }

        Self {
            last_stretched_p: vec![None; models_with_weight.len()],
            models_with_weight: models_with_weight,
        }
    }

    pub fn prob(&mut self) -> f64 {
        let mut sum = 0.;

        let mut i = 0;
        for model in &self.models_with_weight {
            if let Some(prob) = model.model.prob() {
                let p = prob as f64 / U24_MAX as f64;
                let p_stretched = (p / (1. - p)).ln();
                self.last_stretched_p[i] = Some(p_stretched);
                sum += model.weight * p_stretched;
            } else {
                self.last_stretched_p[i] = None;
            }

            i += 1;
        }

        // Squash it
        1. / (1. + f64::exp(-sum))
    }

    pub fn update(&mut self, pred_err: f64, bit: u8) {
        const LEARNING_RATE: f64 = 0.0009;
        let mut i = 0;
        for model in &mut self.models_with_weight {
            model.model.update(bit);
            if let Some(p) = self.last_stretched_p[i] {
                model.weight += LEARNING_RATE * pred_err * p;
            }
            i += 1;
        }
    }
}

pub struct Float20x2([u8; 5]);

impl Float20x2 {
    pub fn extract(&self) -> (f64, f64) {
        let bytes = &self.0;
        (
            Self::unpack_f64(u32::from_le_bytes([bytes[0], bytes[1], bytes[2] & 0x0F, 0])),
            Self::unpack_f64(u32::from_le_bytes([
                (bytes[2] >> 4) & 0x0F,
                bytes[3],
                bytes[4],
                0,
            ])),
        )
    }

    pub fn pack(&mut self, values: (f64, f64)) -> Self {
        let mut bytes = [0; 5];
        let (val1, val2) = values;
        bytes[0..3].copy_from_slice(&u32::to_le_bytes(Self::pack_f64(val1))[0..3]);

        let val2_packed = Self::pack_f64(val2);
        bytes[2] |= ((val2_packed & 0x0F) << 4) as u8;
        bytes[3..5].copy_from_slice(&u32::to_le_bytes(val2_packed >> 4)[0..3]);
        Self(bytes)
    }

    fn pack_f64(val: f64) -> u32 {
        // Clamp value to range [-8.0, 8.0)
        let clamped = val.max(-8.0).min(8.);
        let sign = if clamped < 0.0 { 1 } else { 0 };
        let abs_val = clamped.abs();
        let int_part = abs_val as u32 & 0x7;
        let frac = ((abs_val.fract()) * (1 << 16) as f64) as u32 & 0xFFFF;
        (sign << 19) | (int_part << 16) | frac
    }

    /// Assumes the top bit is sign, next 3 bits are integer, next 16 bits are fraction (range: [-8, 8)).
    fn unpack_f64(input: u32) -> f64 {
        // Mask to 20 bits
        let val = input & 0xFFFFF;
        // Extract sign (bit 19)
        let sign = if (val & 0x80000) != 0 { -1.0 } else { 1.0 };
        // Extract integer part (bits 16..18)
        let int_part = ((val >> 16) & 0x7) as f64;
        // Extract fraction (bits 0..15)
        let frac = (val & 0xFFFF) as f64 / (1 << 16) as f64;
        sign * (int_part + frac)
    }
}

pub struct BitHistory {
    weights: [u8; 5],
}

pub struct AdaptiveProbabilityMap {}

impl AdaptiveProbabilityMap {
    pub fn prob(&mut self) -> f64 {
        0.
    }

    pub fn update(&mut self, pred_err: f64, bit: u8) {}
}

/*
        let id_escape_chars = [
            '\n', '=', '.', '+', '-', '*', '/', ' ', '\t', ';', ',', '(', ')',
        ];
*/
