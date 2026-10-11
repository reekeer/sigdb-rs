use crate::error::{Error, Result};

/// Decoded varint and the offset just past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeResult {
    pub value: u64,
    pub offset: usize,
}

/// LEB128 encoding of an unsigned integer.
pub fn encode_varint(value: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    write_varint(&mut out, value);
    out
}

pub(crate) fn write_varint(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            out.push(b | 0x80);
        } else {
            out.push(b);
            break;
        }
    }
}

/// Decodes a varint at `offset`, reading at most `max_bytes` bytes (10 in the Python package).
///
/// Values wider than 64 bits saturate to `u64::MAX`; Python keeps them exact, but every caller
/// compares them against buffer sizes, so both reject them the same way.
pub fn decode_varint(data: &[u8], offset: usize, max_bytes: usize) -> Result<DecodeResult> {
    let mut shift: u32 = 0;
    let mut result: u64 = 0;
    let mut saturated = false;
    let start = offset;
    let mut offset = offset;
    loop {
        let Some(&b) = data.get(offset) else {
            return Err(Error::format("truncated varint"));
        };
        offset += 1;
        let part = (b & 0x7f) as u64;
        if part != 0 {
            if shift >= 64 || (part << shift) >> shift != part {
                saturated = true;
            } else {
                result |= part << shift;
            }
        }
        if b & 0x80 == 0 {
            let value = if saturated { u64::MAX } else { result };
            return Ok(DecodeResult { value, offset });
        }
        shift += 7;
        if offset - start >= max_bytes {
            return Err(Error::format("varint too long"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for v in [0u64, 1, 127, 128, 255, 300, 16383, 16384, u32::MAX as u64, u64::MAX] {
            let enc = encode_varint(v);
            let dec = decode_varint(&enc, 0, 10).unwrap();
            assert_eq!(dec, DecodeResult { value: v, offset: enc.len() });
        }
        assert_eq!(encode_varint(300), vec![0xac, 0x02]);
    }

    #[test]
    fn errors() {
        assert_eq!(decode_varint(&[0x80], 0, 10).unwrap_err().message(), "truncated varint");
        assert_eq!(decode_varint(&[0xff; 11], 0, 10).unwrap_err().message(), "varint too long");
        assert_eq!(decode_varint(&[], 0, 10).unwrap_err().message(), "truncated varint");
    }
}
