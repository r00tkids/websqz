/// A 20-bit floating point representation with 2 values packed into 5 bytes.
/// Byte 0, byte 1, and the lower 4 bits of byte 2 represent the first value.
/// Byte 3, byte 4, and the upper 4 bits of byte 2 represent the second value.
pub struct Float20x2([u8; 5]);

impl Float20x2 {
    pub fn extract(&self) -> (f64, f64) {
        let bytes = &self.0;
        (
            Self::unpack_f64(u32::from_le_bytes([bytes[0], bytes[1], bytes[2] & 0x0F, 0])),
            Self::unpack_f64(u32::from_le_bytes([bytes[3], bytes[4], bytes[2] >> 4, 0])),
        )
    }

    pub fn pack(&mut self, values: (f64, f64)) -> Self {
        let mut bytes: [u8; 5] = [0; 5];
        let (val1, val2) = values;
        bytes[0..3].copy_from_slice(&u32::to_le_bytes(Self::pack_f64(val1))[0..3]);

        let val2_packed = Self::pack_f64(val2);
        bytes[3..5].copy_from_slice(&u32::to_le_bytes(val2_packed)[0..2]);
        bytes[2] |= ((val2_packed >> 12) & 0xF0) as u8;
        Self(bytes)
    }

    /// Packs a f64 value into a 20-bit representation.
    /// The value is clamped to the range (-8.0, 8.0) and packed into a u32.
    /// The top bit is the sign, the next 3 bits are the integer part, and the next 16 bits are the fractional part.
    fn pack_f64(val: f64) -> u32 {
        // Clamp value to range (-8.0, 8.0)
        let clamped = val.max(-7.9999847412109375).min(7.9999847412109375);
        let sign = if clamped < 0.0 { 1 } else { 0 };
        let abs_val = clamped.abs();
        let int_part = abs_val as u32 & 0x7;
        let frac = ((abs_val.fract()) * (1 << 16) as f64) as u32 & 0xFFFF;
        (sign << 19) | (int_part << 16) | frac
    }

    /// Assumes the top bit is sign, next 3 bits are integer, next 16 bits are fraction (range: (-8, 8)).
    fn unpack_f64(input: u32) -> f64 {
        let val = input & 0xFFFFF;
        // Extract sign (bit 19)
        let sign = if (val & 0x80000) != 0 { -1.0 } else { 1.0 };
        // Extract integer part (bits 16..18)
        let int_part = ((val >> 16) & 0x7) as f64;
        // Extract fraction (bits 0..15)
        let frac = (val & 0xFFFF) as f64 / (1 << 16) as f64;
        sign * (int_part + frac)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    pub fn test_f20x2() {
        let mut f = super::Float20x2([0; 5]);
        let val = (1.5, -7.9999847412109375);
        let packed = f.pack(val);
        let unpacked = packed.extract();
        assert_eq!(val, unpacked);
    }

    #[test]
    pub fn test_f20x2_out_of_bounds() {
        let mut f = super::Float20x2([0; 5]);
        let val = (8.2, -8.1);
        let packed = f.pack(val);
        let unpacked = packed.extract();
        assert_eq!(val, unpacked);
    }
}
