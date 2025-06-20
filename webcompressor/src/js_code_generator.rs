use crate::compress_config::ModelConfig;
use bitflags::bitflags;

bitflags! {
    /// Represents a set of flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct ModelRef: u32 {
        const None = 0b00000000;
        const NOrderBytePred = 0b00000001;
        const Mixer = 0b00000010;
        const AdaptiveProbabilityMap = 0b00000100;
        const Word = 0b00001000;
    }
}

pub fn generate_js_code(model_config: &ModelConfig) -> String {
    let mut features_used = ModelRef::None;

    match model_config {
        ModelConfig::NOrderBytePred { byte_mask } => {
            features_used |= ModelRef::NOrderBytePred;
            format!("new NOrderBytePred({})", byte_mask)
        }
        ModelConfig::Mixer(model_configs) => {
            features_used |= ModelRef::Mixer;
            let models_js: Vec<String> = model_configs.into_iter().map(generate_js_code).collect();
            format!("new LnMixerPred([{}])", models_js.join(", "))
        }
        ModelConfig::AdaptiveProbabilityMap(model_config) => {
            features_used |= ModelRef::AdaptiveProbabilityMap;
            let inner_js = generate_js_code(model_config);
            format!("new AdaptiveProbabilityMap(19, {})", inner_js)
        }
        ModelConfig::Word => {
            features_used |= ModelRef::Word;
            "new WordPred(21, 255)".to_string()
        }
    }
}
