//! Length hiding for encrypted payloads.
//!
//! Payloads are padded with Padmé (Nikitin et al., "Reducing Metadata
//! Leakage from Encrypted Files and Communication with PURBs", PETS 2019):
//! the padded length keeps only the top `⌊log2 E⌋ + 1` bits of the length,
//! so it leaks `O(log log L)` bits; from 256 bytes up the overhead is at most
//! 6.25%. Everything up to [`MIN_PADDED_LEN`] bytes pads to the same size,
//! which hides the length of short secrets such as passwords and tokens
//! entirely.

use crate::Error;

/// Every padded payload is at least this long.
pub const MIN_PADDED_LEN: usize = 256;

/// Padmé padded length for a payload of `len` bytes.
pub fn padded_len(len: usize) -> usize {
    let len = len.max(MIN_PADDED_LEN);
    // E = ⌊log2 L⌋, S = ⌊log2 E⌋ + 1; zero the lowest E - S bits, rounding up.
    let e = usize::BITS - 1 - len.leading_zeros();
    let s = u32::BITS - e.leading_zeros();
    let mask = (1usize << (e - s)) - 1;
    (len + mask) & !mask
}

/// `u32be(len(data)) || data || zeros`, padded to [`padded_len`].
pub fn pad(data: &[u8]) -> Result<Vec<u8>, Error> {
    let len = u32::try_from(data.len()).map_err(|_| Error::TooLarge)?;
    let mut out = Vec::with_capacity(padded_len(4 + data.len()));
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(data);
    out.resize(padded_len(4 + data.len()), 0);
    Ok(out)
}

/// Inverse of [`pad`]. Rejects anything that isn't canonically padded.
pub fn unpad(padded: &[u8]) -> Result<&[u8], Error> {
    let (len, rest) = padded
        .split_first_chunk::<4>()
        .ok_or(Error::Malformed("padding"))?;
    let len = u32::from_be_bytes(*len) as usize;
    if len > rest.len()
        || padded.len() != padded_len(4 + len)
        || rest[len..].iter().any(|&b| b != 0)
    {
        return Err(Error::Malformed("padding"));
    }
    Ok(&rest[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padme_lengths() {
        for (len, padded) in [
            (0, 256),
            (1, 256),
            (256, 256),
            (257, 272),
            (1000, 1024),
            (1025, 1088),
            (1 << 20, 1 << 20),
            ((1 << 20) + 1, (1 << 20) + (1 << 15)),
            (25 * 1024 * 1024, 25 * 1024 * 1024),
            (25 * 1024 * 1024 + 1, 25 * 1024 * 1024 + (1 << 19)),
        ] {
            assert_eq!(padded_len(len), padded, "len {len}");
        }
    }

    #[test]
    fn padding_never_shrinks_and_overhead_is_bounded() {
        for len in (MIN_PADDED_LEN..200_000).step_by(97) {
            let padded = padded_len(len);
            assert!(padded >= len);
            assert!((padded - len) * 16 <= len, "len {len} padded {padded}");
        }
    }

    #[test]
    fn roundtrip_and_strictness() {
        for data in [&b""[..], b"x", &[7u8; 300], &[1u8; 5000]] {
            let padded = pad(data).unwrap();
            assert_eq!(padded.len(), padded_len(4 + data.len()));
            assert_eq!(unpad(&padded).unwrap(), data);
        }

        let mut padded = pad(b"secret").unwrap();
        *padded.last_mut().unwrap() = 1;
        assert!(unpad(&padded).is_err());

        let mut padded = pad(b"secret").unwrap();
        padded.push(0);
        assert!(unpad(&padded).is_err());

        let mut padded = pad(b"secret").unwrap();
        padded[3] = 255;
        assert!(unpad(&padded).is_err());
    }
}
