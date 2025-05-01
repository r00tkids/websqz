use std::io::Read;

use crate::model::{LnMixerPred, ModelDef};
use anyhow::Result;

pub struct ModelFinder {
    pub model_defs: Vec<ModelDef>,
}

impl ModelFinder {
    pub fn new() -> Self {
        let mut model_defs = Vec::new();

        let mut byte_mask = 0;
        model_defs.push(ModelDef {
            byte_mask: byte_mask,
            weight: 0.,
        });
        for i in 0..8 {
            byte_mask |= 1 << i;
            model_defs.push(ModelDef {
                byte_mask: byte_mask,
                weight: 0.,
            });
        }

        let mut byte_mask = 0;
        for i in 0..4 {
            byte_mask |= 1 << (i + 1);
            model_defs.push(ModelDef {
                byte_mask: byte_mask,
                weight: 0.,
            });
        }

        // let mut byte_mask = 0;
        // for i in 0..4 {
        //     byte_mask |= 1 << (i + 2);
        //     model_defs.push(ModelDef {
        //         byte_mask: byte_mask,
        //         weight: 0.,
        //     });
        // }

        // let mut byte_mask = 0;
        // for i in 0..4 {
        //     byte_mask |= 1 << (i + 1);
        //     model_defs.push(ModelDef {
        //         byte_mask: byte_mask,
        //         weight: 0.,
        //     });
        // }

        // for i in 0..32 {
        //     model_defs.push(ModelDef {
        //         byte_mask: i,
        //         weight: 0.,
        //     });
        // }

        Self {
            model_defs: model_defs,
        }
    }

    pub fn learn_from(&mut self, mut byte_stream: impl Read) -> Result<()> {
        let mut best_model = LnMixerPred::new(&self.model_defs);
        let mut bytes = Vec::<u8>::new();
        byte_stream.read_to_end(&mut bytes)?;
        for b in bytes {
            for i in 0..8 {
                let prob = best_model.prob();
                let bit = (b >> (7 - i)) & 1;
                best_model.update(bit as f64 - prob, bit);
            }
        }

        // Normalize and update weights
        let mut i = 0;
        let one_over_total_weight = 1.
            / best_model
                .models_with_weight
                .iter()
                .map(|m| m.weight)
                .sum::<f64>();
        for model_with_weight in &best_model.models_with_weight {
            self.model_defs[i].weight = model_with_weight.weight * one_over_total_weight;
            i += 1;
        }

        Ok(())
    }
}
