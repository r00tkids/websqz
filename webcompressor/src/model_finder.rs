use std::{cell::RefCell, io::Read, rc::Rc};

use crate::{
    coder::ArithmeticEncoder,
    compress_config::ModelConfig,
    model::{HashTable, LnMixerPred, Model, ModelDef, NOrderBytePredData},
    utils::U24_MAX,
};
use anyhow::Result;

pub struct ModelFinder {
    pub model_defs: Vec<ModelDef>,
    pub default_model: Box<dyn Model>,
}

impl ModelFinder {
    pub fn new() -> Self {
        let mut byte_masks = Vec::new();

        let mut byte_mask = 0;
        byte_masks.push(byte_mask);
        for i in 0..8 {
            byte_mask |= 1 << i;
            byte_masks.push(byte_mask);
        }

        let mut byte_mask = 0;
        for i in 0..4 {
            byte_mask |= 1 << i;
            byte_masks.push(byte_mask << 1);
        }

        let mut byte_mask = 0;
        for i in 0..4 {
            byte_mask |= 1 << i;
            byte_masks.push(byte_mask << 2);
        }

        let norder_byte_preds = byte_masks
            .into_iter()
            .map(|mask| ModelConfig::NOrderBytePred {
                byte_mask: format!("0b{:08b}", mask),
            })
            .collect::<Vec<_>>();

        let model = Box::new(ModelConfig::AdaptiveProbabilityMap(Box::new(
            ModelConfig::Mixer(norder_byte_preds),
        )));

        Self {
            model_defs: vec![],
            default_model: model
                .create_model(Rc::new(RefCell::new(HashTable::<NOrderBytePredData>::new(
                    28,
                ))))
                .unwrap(),
        }
    }

    pub fn learn_from(&mut self, mut byte_stream: impl Read) -> Result<()> {
        // let mut bytes = Vec::<u8>::new();
        // byte_stream.read_to_end(&mut bytes)?;

        // let mut current_models = Vec::new();
        // let current_best_size = usize::MAX;
        // for byte_mask in 0..=255 {
        //     current_models.push(ModelDef {
        //         byte_mask: byte_mask,
        //         weight: 0.,
        //     });

        //     let size = Self::test_model_defs(&current_models, bytes.as_slice());
        //     if size < current_best_size {
        //         current_best_size = size;

        //         // Try remove other models
        //         for (idx, model) in current_models
        //             .iter()
        //             .take(current_models.len() - 1)
        //             .enumerate()
        //         {
        //             let test_models = current_models.to_vec();
        //             test_models.remove(idx);

        //             let s = Self::test_model_defs(&test_models, bytes.as_slice());
        //             if s < current_best_size {
        //                 current_models = test_models;
        //                 break;
        //             }
        //         }
        //     }
        // }

        // let mut best_model = LnMixerPred::new(&self.model_defs);
        // for b in bytes {
        //     for i in 0..8 {
        //         let prob = best_model.prob();
        //         let bit = (b >> (7 - i)) & 1;
        //         best_model.update(bit as f64 - prob, bit);
        //     }
        // }

        // Normalize and update weights
        for i in 0..self.model_defs.len() {
            self.model_defs[i].weight = 1. / self.model_defs.len() as f64; //model_with_weight.weight;
        }

        Ok(())
    }

    pub fn test_model_defs(model_defs: &Vec<ModelDef>, bytes: &[u8]) -> usize {
        // let mut model = LnMixerPred::new(model_defs);
        // let result = Vec::new();
        // let mut coder = ArithmeticEncoder::new(result).unwrap();
        // for b in bytes {
        //     for i in 0..8 {
        //         let prob = model.pred();
        //         let bit = (b >> (7 - i)) & 1;
        //         let int_24_prob = (prob * U24_MAX as f64) as u32;

        //         coder.encode(bit, int_24_prob);
        //         model.learn(bit as f64 - prob, bit);
        //     }
        // }

        // coder.finish().unwrap().len()
        0
    }
}
