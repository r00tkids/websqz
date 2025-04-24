use anyhow::{anyhow, bail, Result};
use bytes::Buf;
use std::{
    io::{BufReader, BufWriter, Read, Write},
    u32,
};

struct ArithmeticEncoder<W: Write> {
    low: u32,
    high: u32,

    output: BufWriter<W>,
}

impl<W: Write> ArithmeticEncoder<W> {
    pub fn new(stream: W) -> Result<Self> {
        Ok(Self {
            low: 0,
            high: u32::MAX,
            output: BufWriter::new(stream),
        })
    }

    pub fn encode(&mut self, bit: u8, p: u32) -> Result<()> {
        assert!(p <= 0xffff);
        assert!(bit == 0 || bit == 1);
        assert!(self.high > self.low);
        let mid = self.low + (((self.high - self.low) as u64 * p as u64) >> 16) as u32;

        assert!(self.high > mid && mid >= self.low);
        if bit == 1 {
            self.high = mid;
        } else {
            self.low = mid + 1;
        }

        // Renormalize and tell the decoder about it
        // This happens when the MSB of low and high is equal
        // since at that point there isn't enough precision left in the range.
        while (self.high ^ self.low) < 0x1000000 {
            self.output.write(&[(self.high >> 24) as u8])?;
            self.low <<= 8;
            self.high = self.high << 8 | 255;
        }

        Ok(())
    }

    pub fn finish(mut self) -> Result<W> {
        self.output.write(&[(self.high >> 24) as u8])?;
        Ok(self
            .output
            .into_inner()
            .map_err(|e| anyhow!("failed to flush: {:?}", e.error()))?)
    }
}

struct ArithmeticDecoder<R: Read> {
    low: u32,
    high: u32,
    state: u32,
    input: BufReader<R>,
}

impl<R: Read> ArithmeticDecoder<R> {
    pub fn new(stream: R) -> Result<Self> {
        let mut input = BufReader::new(stream);

        let mut state: u32 = 0;
        let mut buf = [0u8; 1];
        for i in 0..4 {
            if input.read(&mut buf)? == 0 {
                buf[0] = 0;
            }
            state = (state << 8) | buf[0] as u32;
        }

        Ok(Self {
            low: 0,
            input: input,
            state: state,
            high: u32::MAX,
        })
    }

    pub fn decode(&mut self, p: u32) -> Result<u8> {
        assert!(p <= 0xffff);
        assert!(self.high > self.low);

        let mid = self.low + (((self.high - self.low) as u64 * p as u64) >> 16) as u32;

        assert!(self.high > mid && mid >= self.low);
        let mut bit = 0;
        if self.state <= mid {
            bit = 1;
            self.high = mid;
        } else {
            self.low = mid + 1;
        }

        while (self.high ^ self.low) < 0x1000000 {
            self.low <<= 8;
            self.high = self.high << 8 | 255;
            let mut c = [0u8];
            if self.input.read(&mut c)? == 0 {
                c[0] = 0;
            }
            self.state = self.state << 8 | c[0] as u32;
        }

        Ok(bit)
    }
}

struct Predictor {
    ctx: usize,
    count: [[u32; 2]; 512],
}

impl Predictor {
    pub fn new() -> Self {
        Self {
            ctx: 0,
            count: [[0; 2]; 512],
        }
    }

    fn prob(&self) -> u32 {
        (0x10000 * (self.count[self.ctx][1] + 1))
            / (self.count[self.ctx][1] + self.count[self.ctx][0] + 2)
    }

    fn update(&mut self, bit: u8) {
        // Update count
        self.count[self.ctx][bit as usize] += 1;
        if self.count[self.ctx][bit as usize] > 0xffff {
            // Rescale count
            self.count[self.ctx][0] >>= 1;
            self.count[self.ctx][1] >>= 1;
        }

        self.ctx = (self.ctx << 1) | bit as usize;
        if self.ctx >= 512 {
            // We overflowed
            self.ctx = bit as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read};

    use crate::coder::Predictor;

    use super::{ArithmeticDecoder, ArithmeticEncoder};

    #[test]
    pub fn round_trip() {
        let mut hello = String::new();
        File::open("test_data.txt")
            .unwrap()
            .read_to_string(&mut hello)
            .unwrap();
        let hello_bytes = hello.as_bytes();
        let encoded_data = {
            let mut predictor = Predictor::new();
            let encoded_data: Vec<u8> = Vec::new();
            let mut encoder = ArithmeticEncoder::new(encoded_data).unwrap();

            for b in hello_bytes {
                for i in 0..8 {
                    let prob = predictor.prob();
                    let bit = (b >> i) & 1;
                    encoder.encode(bit, prob).unwrap();
                    predictor.update(bit);
                }
            }
            encoder.finish().unwrap()
        };

        println!(
            "Size of input: {}\nSize of encoded data: {}",
            hello.len(),
            encoded_data.len()
        );

        let mut decoder = ArithmeticDecoder::new(encoded_data.as_slice()).unwrap();

        let mut predictor = Predictor::new();
        let mut decode_buf = vec![0; hello_bytes.len()];
        for i in 0..(hello_bytes.len()) {
            for bit in 0..8 {
                let prob = predictor.prob();
                let r = decoder.decode(prob).unwrap();
                decode_buf[i] |= r << bit;
                predictor.update(r);
            }
        }

        assert!(String::from_utf8(decode_buf.to_vec()).unwrap() == hello);
    }
}
