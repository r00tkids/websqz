use crate::utils::{prob_squash, prob_stretch, U24_MAX};
use std::{
    cell::RefCell,
    ops::{Index, IndexMut},
    rc::Rc,
    vec,
};

#[derive(Clone, Copy)]
pub struct NOrderBytePredData(u32);

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

pub struct HashTable<Record> {
    table: Vec<Record>,
    hash_mask: usize,
}

fn hash(mut value: u32, shift: u32) -> u32 {
    const K_MUL: u32 = 0x1e35a7bd;
    value ^= value >> shift;
    K_MUL.wrapping_mul(value) >> shift
}

impl<Record> HashTable<Record>
where
    Record: Default + Clone,
{
    pub fn new(pow2_size: u32) -> Self {
        let context_size = (1 << pow2_size) as usize;
        println!(
            "Hash table Size: {} MiB",
            (size_of::<Record>() * context_size) / (1024 * 1024)
        );

        Self {
            table: vec![Record::default(); context_size],
            hash_mask: (1 << pow2_size) - 1,
        }
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn get<'a>(&'a self, key: u32) -> &'a Record {
        &self.table[key as usize & self.hash_mask]
    }

    pub fn get_mut<'a>(&'a mut self, key: u32) -> &'a mut Record {
        &mut self.table[key as usize & self.hash_mask]
    }
}

#[derive(Clone, Copy)]
pub struct NOrderBytePredDataRec([NOrderBytePredData; 255]);
impl Default for NOrderBytePredDataRec {
    fn default() -> Self {
        Self([NOrderBytePredData::default(); 255])
    }
}

impl NOrderBytePredDataRec {
    fn get(&self, bit_ctx: u32) -> &NOrderBytePredData {
        &self.0[bit_ctx as usize - 1]
    }

    fn get_mut(&mut self, bit_ctx: u32) -> &mut NOrderBytePredData {
        &mut self.0[bit_ctx as usize - 1]
    }
}

pub struct NOrderBytePred {
    ctx: u32,
    hash_table: Rc<RefCell<HashTable<NOrderBytePredData>>>,
    max_count: u32,

    magic_num: u32,
    prev_bytes: u64,
    mask: u64,

    bit_ctx: u32,
}

impl NOrderBytePred {
    pub fn new(
        byte_mask: u8,
        hash_table: Rc<RefCell<HashTable<NOrderBytePredData>>>,
        max_count: u32,
    ) -> Self {
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

    pub fn pred(&self) -> Option<f64> {
        let entry = self
            .hash_table
            .borrow()
            .get(self.ctx ^ self.bit_ctx)
            .clone();
        if entry.count() == 0 {
            return None;
        }

        Some(entry.prob() as f64 / U24_MAX as f64)
    }

    pub fn learn(&mut self, bit: u8) {
        {
            let mut hash_table = self.hash_table.borrow_mut();
            let inst = hash_table.get_mut(self.ctx ^ self.bit_ctx);

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
            self.ctx = (hash((self.prev_bytes >> 32) as u32, 3)
                .wrapping_mul(9)
                .wrapping_add(hash(self.prev_bytes as u32, 3)))
            .wrapping_mul(self.magic_num);

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
    weights: Vec<Vec<Vec<f64>>>,
    prev_byte: u32,
    bit_ctx: u32,
    word_pred: WordPred,
    word_pred_weight: f64,
}

impl LnMixerPred {
    pub fn new(model_defs: &Vec<ModelDef>) -> Self {
        let hash_table = Rc::new(RefCell::new(HashTable::<NOrderBytePredData>::new(27)));

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
            weights: vec![vec![vec![]; 255]; 256],
            bit_ctx: 1,
            prev_byte: 0,
            word_pred: WordPred::new(21, 255),
            word_pred_weight: 0.9,
        }
    }

    pub fn pred(&mut self) -> f64 {
        let mut sum = 0.;

        let weights = &mut self.weights[self.prev_byte as usize][self.bit_ctx as usize - 1];
        let mut i = 0;
        for model in &self.models_with_weight {
            let model_weight = if weights.is_empty() {
                model.weight
            } else {
                weights[i] * 0.3 + model.weight
            };

            if let Some(p) = model.model.pred() {
                let p_stretched = prob_stretch(p);
                self.last_stretched_p[i] = Some(p_stretched);
                sum += model_weight * p_stretched;
            } else {
                self.last_stretched_p[i] = None;
            }

            i += 1;
        }

        let mut p_stretched = prob_stretch(self.word_pred.pred());
        p_stretched *= self.word_pred_weight;

        sum + p_stretched
    }

    pub fn learn(&mut self, pred_err: f64, bit: u8) {
        let weights = &mut self.weights[self.prev_byte as usize][self.bit_ctx as usize - 1];

        const LEARNING_RATE: f64 = 0.0004;
        const LEARNING_RATE_CTX: f64 = 0.04;
        let mut i = 0;
        for model in &mut self.models_with_weight {
            model.model.learn(bit);
            if let Some(p) = self.last_stretched_p[i] {
                model.weight += LEARNING_RATE * pred_err * p;
                if !weights.is_empty() {
                    weights[i] += LEARNING_RATE_CTX * pred_err * p;
                }
            }
            i += 1;
        }

        if weights.is_empty() {
            weights.reserve(self.models_with_weight.len());
            for i in 0..self.models_with_weight.len() {
                weights.push(self.models_with_weight[i].weight);
            }
        }

        self.word_pred_weight += 0.004 * pred_err * prob_stretch(self.word_pred.pred());
        self.word_pred.learn(bit);

        self.bit_ctx = (self.bit_ctx << 1) | bit as u32;

        if self.bit_ctx >= 256 {
            self.bit_ctx &= 0xff;
            self.prev_byte = self.bit_ctx;
            self.bit_ctx = 1;
        }
    }
}

#[derive(Clone, Default)]
pub struct SSEPredData([NOrderBytePredData; 32]);

impl Index<usize> for SSEPredData {
    type Output = NOrderBytePredData;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<usize> for SSEPredData {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

pub struct AdaptiveProbabilityMap {
    ctx: u32,
    hash_table: HashTable<SSEPredData>,
    max_count: u32,

    current_prob_idx: usize,

    prev_bytes: u64,
    mask: u64,

    bit_ctx: u32,

    input_model: Box<dyn Model>,
}

impl AdaptiveProbabilityMap {
    pub fn new(pow2_size: u32, input_model: impl Model + 'static) -> AdaptiveProbabilityMap {
        AdaptiveProbabilityMap {
            ctx: 0,
            hash_table: HashTable::<SSEPredData>::new(pow2_size),
            max_count: 255,
            current_prob_idx: 0,

            prev_bytes: 0,
            mask: 0xffffff,

            bit_ctx: 1,
            input_model: Box::new(input_model),
        }
    }

    pub fn pred(&mut self) -> f64 {
        let p = self.input_model.pred();
        // TODO: Interpolate probabilities
        let p_ptr = p.max(-8.).min(7.5) * 2.;
        let p_idx_f = p_ptr.floor();
        let p_idx_c = p_ptr.ceil();

        let delta_f = p_ptr - p_idx_f;
        let delta_c = p_idx_c - p_ptr;
        let mut t;
        let mut next_idx;
        if delta_f <= delta_c {
            // Use floor
            self.current_prob_idx = (p_idx_f as i32 + 16) as usize;
            next_idx = self.current_prob_idx + 1;
            if next_idx >= 32 {
                next_idx = 31;
            }
            t = 1. - delta_f;
        } else {
            // Use ceil
            self.current_prob_idx = (p_idx_c as i32 + 16) as usize;
            next_idx = self.current_prob_idx - 1;
            t = 1. - delta_c;
        }
        if next_idx >= 32 {
            println!(
                "Warning: next_idx is out of bounds: {}, {}, {}, {}, {}",
                next_idx, self.current_prob_idx, t, p, p_ptr
            );
        }

        let counter1 = {
            let counter1: &mut NOrderBytePredData =
                &mut self.hash_table.get_mut(self.ctx ^ self.bit_ctx)[self.current_prob_idx];
            if counter1.count() == 0 {
                let prob = (prob_squash(p) * U24_MAX as f64) as i32 & U24_MAX as i32;
                counter1.set_prob(prob);
            }
            counter1.clone()
        };

        let counter2 = {
            let counter2: &mut NOrderBytePredData =
                &mut self.hash_table.get_mut(self.ctx ^ self.bit_ctx)[next_idx];
            if counter2.count() == 0 {
                let prob = (prob_squash(p) * U24_MAX as f64) as i32 & U24_MAX as i32;
                counter2.set_prob(prob);
            }
            counter2.clone()
        };

        let new_p = t * (counter1.prob() as f64 / U24_MAX as f64)
            + (1. - t) * (counter2.prob() as f64 / U24_MAX as f64);
        prob_stretch(new_p)
    }

    pub fn learn(&mut self, pred_err: f64, bit: u8) {
        {
            let inst = &mut self.hash_table.get_mut(self.ctx ^ self.bit_ctx)
                [self.current_prob_idx as usize];

            let (mut count, mut prob) = (inst.count(), inst.prob());
            if count < self.max_count {
                count += 1;
            }

            // Learning function
            prob += (U24_MAX as f64
                * ((bit as f64 - (prob as f64 / U24_MAX as f64)) / ((count + 30) as f64 + 1.5)))
                as i32;
            inst.set_count(count);
            inst.set_prob(prob);
        }

        self.bit_ctx = (self.bit_ctx << 1) | bit as u32;
        if self.bit_ctx >= 256 {
            self.bit_ctx &= 0xff;

            self.prev_bytes = ((self.prev_bytes << 8) | self.bit_ctx as u64) & self.mask;
            // Remove the extra leading bit before using it in the ctx
            self.ctx = hash((self.prev_bytes >> 32) as u32, 3)
                .wrapping_mul(9)
                .wrapping_add(hash(self.prev_bytes as u32, 3));

            // Reset bit_ctx
            self.bit_ctx = 1;
        }

        self.input_model.learn(pred_err, bit);
    }
}

pub struct WordPred {
    ctx: u32,
    hash_table: HashTable<NOrderBytePredData>,
    max_count: u32,

    current_word: u32,
    prev_words: [u32; 3],

    bit_ctx: u32,
}

impl WordPred {
    pub fn new(pow2_size: u32, max_count: u32) -> Self {
        assert!(max_count <= 255);

        Self {
            ctx: 0,
            bit_ctx: 1,
            max_count: max_count,
            hash_table: HashTable::new(pow2_size),
            prev_words: [0; 3],
            current_word: 0,
        }
    }

    pub fn pred(&self) -> f64 {
        let entry = self.hash_table.get(self.ctx ^ self.bit_ctx).clone();
        entry.prob() as f64 / U24_MAX as f64
    }

    pub fn learn(&mut self, bit: u8) {
        {
            let inst = self.hash_table.get_mut(self.ctx ^ self.bit_ctx);

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
            // Remove the extra leading bit before using it in the ctx
            self.bit_ctx &= 0xff;

            let next_char = self.bit_ctx as u8 as char;
            if next_char.is_alphanumeric() {
                self.current_word =
                    self.current_word ^ next_char.to_lowercase().next().unwrap() as u32;
                self.current_word = self.current_word.wrapping_mul(16777619);
            } else if self.current_word != 0 {
                // Shift previous words
                self.prev_words[0] = self.prev_words[1];
                self.prev_words[1] = self.current_word;
                self.current_word = 2166136261;
            }

            self.ctx = self.current_word.wrapping_mul(3);

            // Reset bit_ctx
            self.bit_ctx = 1;
        }
    }
}

pub trait Model {
    fn pred(&mut self) -> f64;
    fn learn(&mut self, pred_err: f64, bit: u8);
}

impl Model for NOrderBytePred {
    fn pred(&mut self) -> f64 {
        NOrderBytePred::pred(self).map_or(0.5, |p| prob_stretch(p))
    }

    fn learn(&mut self, pred_err: f64, bit: u8) {
        self.learn(bit);
    }
}

impl Model for LnMixerPred {
    fn pred(&mut self) -> f64 {
        self.pred()
    }

    fn learn(&mut self, pred_err: f64, bit: u8) {
        self.learn(pred_err, bit);
    }
}

impl Model for AdaptiveProbabilityMap {
    fn pred(&mut self) -> f64 {
        self.pred()
    }

    fn learn(&mut self, pred_err: f64, bit: u8) {
        self.learn(pred_err, bit);
    }
}

impl Model for WordPred {
    fn pred(&mut self) -> f64 {
        WordPred::pred(self)
    }

    fn learn(&mut self, pred_err: f64, bit: u8) {
        self.learn(bit);
    }
}
