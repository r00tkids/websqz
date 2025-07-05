const STX: u8 = 0x02;
const ETX: u8 = 0x03;

pub fn bwt_optimized(input: &[u8]) -> Vec<u8> {
    assert!(
        !input.contains(&STX) && !input.contains(&ETX),
        "Input cannot contain start or end marker bytes"
    );

    // Add start and end markers
    let mut s = Vec::with_capacity(input.len() + 2);
    s.push(STX);
    s.extend_from_slice(input);
    s.push(ETX);

    let n = s.len();

    // Create index list [0, 1, 2, ..., n-1]
    let mut indices: Vec<usize> = (0..n).collect();

    // Sort indices based on the rotated view of `s`
    indices.sort_by(|&i, &j| {
        (0..n)
            .map(|k| s[(i + k) % n].cmp(&s[(j + k) % n]))
            .find(|&ord| ord != std::cmp::Ordering::Equal)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Collect the last byte of each logical rotation
    indices.iter().map(|&i| s[(i + n - 1) % n]).collect()
}

#[cfg(test)]
mod tests {

    use super::bwt_optimized;

    #[test]
    pub fn bwt_encode() {
        let input = b"BANANA";
        let output = bwt_optimized(input);
        let expected_output = b"\x03ANNB\x02AA";
        assert_eq!(output, expected_output);
    }
}
