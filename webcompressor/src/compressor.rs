use std::{
    collections::HashMap,
    io::{Read, Write},
};

use crate::coder::{ArithmeticDecoder, ArithmeticEncoder};
use crate::{
    model::Model,
    utils::{prob_squash, U24_MAX},
};
use anyhow::Result;

struct Encoder<W: Write> {
    coder: ArithmeticEncoder<W>,
    model: Box<dyn Model>,
}

impl<W: Write> Encoder<W> {
    pub fn new(model: Box<dyn Model>, output: W) -> Result<Self> {
        Ok(Self {
            coder: ArithmeticEncoder::new(output)?,
            model: model,
        })
    }

    // 9-bit symbols
    // if MSB = 0
    //  encode as a normal byte
    // if MSB = 1
    //  encode dict. pos = { idx: 12 bit, len: 4 bit }

    pub fn encode_bytes(mut self, mut byte_stream: impl Read) -> Result<W> {
        let mut bytes = Vec::<u8>::new();
        byte_stream.read_to_end(&mut bytes)?;
        //bytes = bwt_optimized(&bytes);

        let mut b_idx = 0;
        while b_idx < bytes.len() {
            let b = bytes[b_idx];
            for i in 0..8 {
                let prob = prob_squash(self.model.pred());
                let int_24_prob = (prob * U24_MAX as f64) as u32;
                let bit = (b >> (7 - i)) & 1;
                self.coder.encode(bit, int_24_prob)?;
                self.model.learn(bit as f64 - prob, bit);
            }

            b_idx += 1;
        }
        Ok((self.coder.finish()?))
    }

    pub fn warm_up(&mut self, mut byte_stream: impl Read) -> Result<()> {
        let mut bytes = Vec::<u8>::new();
        byte_stream.read_to_end(&mut bytes)?;
        for b in bytes {
            for i in 0..8 {
                let prob = prob_squash(self.model.pred());
                let bit = (b >> (7 - i)) & 1;
                self.model.learn(bit as f64 - prob, bit);
            }
        }

        Ok(())
    }
}

struct Decoder<R: Read> {
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
            for i in 0..8 {
                let prob = prob_squash(self.model.pred());
                let int_24_prob = (prob * U24_MAX as f64) as u32;
                let bit = self.coder.decode(int_24_prob)?;
                self.model.learn(bit as f64 - prob, bit);
                res[byte_idx] |= bit << (7 - i);
            }
        }

        Ok(res)
    }

    pub fn warm_up(&mut self, mut byte_stream: impl Read) -> Result<()> {
        let mut bytes = Vec::<u8>::new();
        byte_stream.read_to_end(&mut bytes)?;
        for b in bytes {
            for i in 0..8 {
                let prob = prob_squash(self.model.pred());
                let bit = (b >> (7 - i)) & 1;
                self.model.learn(bit as f64 - prob, bit);
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

        let mut model_finder = ModelFinder::new();
        let encoded_data = {
            let model = model_finder.default_model;
            let encoded_data: Vec<u8> = Vec::new();
            let mut encoder = Encoder::new(model, encoded_data).unwrap();
            encoder.warm_up(bootstrap_text.as_bytes()).unwrap();
            encoder.encode_bytes(test_bytes).unwrap()
        };

        println!(
            "Size of input: {}\nSize of encoded data: {}",
            test_data.len(),
            encoded_data.len()
        );

        let mut model_finder = ModelFinder::new();
        let model = model_finder.default_model;
        let mut decoder = Decoder::new(model, encoded_data.as_slice()).unwrap();

        decoder.warm_up(bootstrap_text.as_bytes()).unwrap();

        let decode_res = decoder.decode(test_bytes.len()).unwrap();
        assert!(String::from_utf8(decode_res).unwrap() == test_data);
    }
}
