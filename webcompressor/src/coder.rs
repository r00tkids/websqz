use crate::utils::U24_MAX;
use anyhow::{anyhow, Result};
use std::{
    io::{BufReader, BufWriter, Read, Write},
    u32,
};

pub struct ArithmeticEncoder<W: Write> {
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
        // p is P(1) and has 24 bit precision
        assert!(p <= U24_MAX);
        assert!(bit == 0 || bit == 1);
        assert!(self.high > self.low);

        // mid = low + ((high - low) * p) / (p_max + 1)
        let p_f = p as f64 / U24_MAX as f64;
        let mid = self.low + ((self.high - self.low) as f64 * p_f) as u32;
        /*
        self.low + (((self.high - self.low) as u64 * p as u64) >> 24) as u32;*/

        // Below or equal to mid is the interval for bit = 1, and above mid is the inverval for bit = 0
        assert!(self.high > mid && mid >= self.low);
        if bit == 1 {
            // Set the subinterval to low -> mid
            self.high = mid;
        } else {
            // Set the subinterval to (mid + 1) -> high
            self.low = mid + 1;
        }

        // Renormalize and tell the decoder about it
        // as long as the MSB of low and high is equal
        // since at that point there isn't enough precision left in the MSB range to distinguish between 0 and 1 bit
        while (self.high ^ self.low) < (1 << 24) {
            self.output.write(&[(self.high >> 24) as u8])?;
            self.low <<= 8; // Shift in 0x00
            self.high = self.high << 8 | 0xFF; // Shift in 0xFF
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

pub struct ArithmeticDecoder<R: Read> {
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
        for _ in 0..4 {
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
        assert!(p <= U24_MAX);
        assert!(self.high > self.low);

        let p_f = p as f64 / U24_MAX as f64;
        let mid = self.low + ((self.high - self.low) as f64 * p_f) as u32;

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

#[cfg(test)]
mod tests {

    use super::{ArithmeticDecoder, ArithmeticEncoder};

    #[test]
    pub fn round_trip() {
        let hello = "hello world";
        let hello_bytes = hello.as_bytes();
        let encoded_data = {
            let encoded_data: Vec<u8> = Vec::new();
            let mut encoder = ArithmeticEncoder::new(encoded_data).unwrap();

            for b in hello_bytes {
                for i in 0..8 {
                    let bit = (b >> i) & 1;
                    encoder.encode(bit, 0x7FFFFF).unwrap();
                }
            }
            encoder.finish().unwrap()
        };

        let mut decoder = ArithmeticDecoder::new(encoded_data.as_slice()).unwrap();

        let mut decode_buf = vec![0; hello_bytes.len()];
        for i in 0..(hello_bytes.len()) {
            for bit in 0..8 {
                let r = decoder.decode(0x7FFFFF).unwrap();
                decode_buf[i] |= r << bit;
            }
        }

        assert!(String::from_utf8(decode_buf.to_vec()).unwrap() == hello);
    }

    #[test]
    pub fn prob_1_bit_0() {
        let bytes = [0, 0, 0, 0];
        let encoded_data = {
            let encoded_data: Vec<u8> = Vec::new();
            let mut encoder = ArithmeticEncoder::new(encoded_data).unwrap();

            for b in bytes {
                for i in 0..8 {
                    let bit = (b >> i) & 1;
                    encoder.encode(bit, 0xffffff).unwrap();
                }
            }
            encoder.finish().unwrap()
        };

        let mut decoder = ArithmeticDecoder::new(encoded_data.as_slice()).unwrap();

        let mut decode_buf = vec![0; bytes.len()];
        for i in 0..(bytes.len()) {
            for bit in 0..8 {
                let r = decoder.decode(0xffffff).unwrap();
                decode_buf[i] |= r << bit;
            }
        }

        assert!(decode_buf.to_vec() == bytes);
    }
}
