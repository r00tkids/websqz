use std::io::{Read, Write};

use crate::coder::{ArithmeticDecoder, ArithmeticEncoder};
use crate::{model::Model, utils::prob_squash};
use anyhow::{Context, Result};

pub struct Encoder<W: Write> {
    coder: ArithmeticEncoder<W>,
    model: Box<dyn Model>,
    reset_points: Vec<u32>,
}

impl<W: Write> Encoder<W> {
    pub fn new(model: Box<dyn Model>, output: W) -> Result<Self> {
        Ok(Self {
            coder: ArithmeticEncoder::new(output)?,
            model: model,
            reset_points: Vec::new(),
        })
    }

    /// Encodes a section using the arithmetic coder and the provided model.
    /// Each section resets the statistics of the model, but keeps the learned probabilities.
    /// Similar type of data should be encoded in the same section.
    pub fn encode_section(&mut self, mut byte_stream: impl Read) -> Result<()> {
        if !self.reset_points.is_empty() {
            // Reset the model statistics at the start of the section
            self.model.reset();
        }

        let mut bytes = Vec::<u8>::new();
        byte_stream.read_to_end(&mut bytes)?;

        let mut b_idx = 0;
        while b_idx < bytes.len() {
            let b = bytes[b_idx];
            for i in 0..8 {
                let prob = prob_squash(self.model.pred());
                let bit = (b >> (7 - i)) & 1;
                self.coder.encode(bit, prob)?;
                self.model.learn(bit);
            }

            b_idx += 1;
        }
        self.reset_points.push(self.coder.len() as u32);
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        self.coder.finish().context("Failed to finish encoding")
    }

    /// Warms up the model by reading a byte stream and learning from it.
    pub fn warm_up(&mut self, mut byte_stream: impl Read) -> Result<()> {
        let mut bytes = Vec::<u8>::new();
        byte_stream.read_to_end(&mut bytes)?;
        for b in bytes {
            for i in 0..8 {
                let bit = (b >> (7 - i)) & 1;
                self.model.learn(bit);
            }
        }

        Ok(())
    }
}

#[allow(dead_code)]
pub struct Decoder<R: Read> {
    coder: ArithmeticDecoder<R>,
    model: Box<dyn Model>,
}

impl<R: Read> Decoder<R> {
    pub fn new(model: Box<dyn Model>, read_stream: R) -> Result<Self> {
        Ok(Self {
            coder: ArithmeticDecoder::new(read_stream)?,
            model: model,
        })
    }

    pub fn decode(&mut self, size: usize) -> Result<Vec<u8>> {
        let mut res: Vec<u8> = vec![0; size];
        for byte_idx in 0..size {
            for _ in 0..8 {
                let prob = prob_squash(self.model.pred());
                let bit = self.coder.decode(prob)?;
                self.model.learn(bit);
                res[byte_idx] = (res[byte_idx] << 1) | bit;
            }
        }

        Ok(res)
    }

    pub fn warm_up(&mut self, mut byte_stream: impl Read) -> Result<()> {
        let mut bytes = Vec::<u8>::new();
        byte_stream.read_to_end(&mut bytes)?;
        for b in bytes {
            for i in 0..8 {
                let bit = (b >> (7 - i)) & 1;
                self.model.learn(bit);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read};

    use crate::compressor::{Decoder, Encoder};
    use crate::model_finder::ModelFinder;

    #[test]
    pub fn round_trip() {
        let bootstrap_text = "for(w=c.width=185,e=c.getContext('2d'),e.drawImage(this,p=0,0),n='',d=e.getImageData(0,0,w,150).data;t=d[p+=4];)n+=String.fromCharCode(t);(1,eval)(e)";

        let mut test_data = String::new();
        // "tests/ray_tracer/index.js"
        File::open("tests/reore/reore_decompressed.bin")
            .unwrap()
            .read_to_string(&mut test_data)
            .unwrap();

        let test_bytes = test_data.as_bytes();

        let model_finder = ModelFinder::new();
        let mut encoded_data: Vec<u8> = Vec::new();
        {
            let model = model_finder.default_model;
            let mut encoder = Encoder::new(model, &mut encoded_data).unwrap();
            encoder.warm_up(bootstrap_text.as_bytes()).unwrap();
            encoder.encode_section(test_bytes).unwrap();
            encoder.finish().unwrap();
        }

        println!(
            "Size of input: {}\nSize of encoded data: {}",
            test_data.len(),
            encoded_data.len()
        );

        let model_finder = ModelFinder::new();
        let model = model_finder.default_model;
        let mut decoder = Decoder::new(model, encoded_data.as_slice()).unwrap();

        decoder.warm_up(bootstrap_text.as_bytes()).unwrap();

        let decode_res = decoder.decode(test_bytes.len()).unwrap();
        assert!(String::from_utf8(decode_res).unwrap() == test_data);
    }
}
