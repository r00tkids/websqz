use crate::compress_config::ModelConfig;
use bitflags::bitflags;

bitflags! {
    /// Represents a set of flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ModelRef: u32 {
        const None = 0b00000000;
        const NOrderByte = 0b00000001;
        const Mixer = 0b00000010;
        const AdaptiveProbabilityMap = 0b00000100;
        const Word = 0b00001000;
        const HashTable = 0b00010000;
    }
}

pub fn generate_js_code(model_config: &ModelConfig, features_used: &mut ModelRef) -> String {
    let mut static_src: String = "".to_owned();
    let out_src = match model_config {
        ModelConfig::NOrderByte { byte_mask } => {
            *features_used |= ModelRef::NOrderByte;
            *features_used |= ModelRef::HashTable;
            format!("NOrderByte({})", byte_mask)
        }
        ModelConfig::Mixer(model_configs) => {
            *features_used |= ModelRef::Mixer;
            let models_js: Vec<String> = model_configs
                .into_iter()
                .map(|c| generate_js_code(c, features_used))
                .collect();
            format!("LnMixerPred([{}])", models_js.join(", "))
        }
        ModelConfig::AdaptiveProbabilityMap(model_config) => {
            *features_used |= ModelRef::AdaptiveProbabilityMap;
            let inner_js = generate_js_code(model_config, features_used);
            format!("AdaptiveProbabilityMap(19, {})", inner_js)
        }
        ModelConfig::Word => {
            *features_used |= ModelRef::Word;
            "WordPred(21, 255)".to_string()
        }
    };

    if features_used.contains(ModelRef::NOrderByte) {
        static_src += include_str!("js_source/norder_byte.js");
    }

    if features_used.contains(ModelRef::Mixer) {
        static_src += include_str!("js_source/mixer.js");
    }

    if features_used.contains(ModelRef::AdaptiveProbabilityMap) {
        static_src += include_str!("js_source/adaptive_probability_map.js");
    }

    if features_used.contains(ModelRef::Word) {
        static_src += include_str!("js_source/word.js");
    }

    static_src + out_src.as_str()
}
