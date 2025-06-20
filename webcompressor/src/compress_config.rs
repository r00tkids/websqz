use std::{cell::RefCell, rc::Rc};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    js_code_generator::generate_js_code,
    model::{
        AdaptiveProbabilityMap, HashTable, LnMixerPred, Model, ModelDef, NOrderBytePred,
        NOrderBytePredData, WordPred,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressConfig {
    pub model: ModelConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ModelConfig {
    NOrderBytePred { byte_mask: String },
    Mixer(Vec<ModelConfig>),
    AdaptiveProbabilityMap(Box<ModelConfig>),
    Word,
}

impl ModelConfig {
    pub fn create_model(
        &self,
        hash_table: Rc<RefCell<HashTable<NOrderBytePredData>>>,
    ) -> Result<Box<dyn Model>> {
        Ok(match self {
            ModelConfig::NOrderBytePred { byte_mask } => {
                let byte_mask = u8::from_str_radix(byte_mask.trim_start_matches("0b"), 2)?;
                Box::new(NOrderBytePred::new(byte_mask, hash_table, 255))
            }
            ModelConfig::Mixer(model_configs) => Box::new(LnMixerPred::new(
                model_configs
                    .iter()
                    .map(|config| config.create_model(hash_table.clone()))
                    .collect::<Result<Vec<_>>>()?,
            )),
            ModelConfig::AdaptiveProbabilityMap(model_config) => Box::new(
                AdaptiveProbabilityMap::new(19, model_config.create_model(hash_table.clone())?),
            ),
            ModelConfig::Word => Box::new(WordPred::new(21, 255)),
        })
    }

    pub fn generate_js_code(&self) -> String {
        generate_js_code(self)
    }
}
