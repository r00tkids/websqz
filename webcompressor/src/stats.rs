use std::{
    fs::File,
    io::{BufWriter, Read, Write},
};

use crate::{
    model::{LnMixerPred, ModelDef},
    utils::prob_squash,
};
use anyhow::Result;

pub struct StatsGenerator {}

impl StatsGenerator {
    pub fn gather_and_dump(mut byte_stream: impl Read, model_defs: &Vec<ModelDef>) -> Result<()> {
        const ONE_OVER_8: f64 = 1. / 8.;
        let mut bytes = Vec::<u8>::new();
        byte_stream.read_to_end(&mut bytes)?;
        let mut model = LnMixerPred::new(&model_defs);

        let output_file = File::create("output.html")?;
        let mut output = BufWriter::new(output_file);

        write!(output, "<html><body>")?;

        for b in bytes {
            let mut avg_pred_err_byte = 0.;
            for i in 0..8 {
                let prob = prob_squash(model.pred());
                let bit = (b >> (7 - i)) & 1;
                let pred_err = bit as f64 - prob;
                avg_pred_err_byte += pred_err.abs();
                model.learn(pred_err, bit);
            }

            avg_pred_err_byte *= ONE_OVER_8;

            write!(
                output,
                "<i style=\"background: hsl({}, 100%, 40%);\">{}</i>",
                ((1. - avg_pred_err_byte) * 75.) as u8,
                b as char
            )?;
        }
        write!(output, "</body></html>")?;

        Ok(())
    }
}
