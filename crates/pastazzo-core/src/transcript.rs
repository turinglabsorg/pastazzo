/// Unambiguous encoding of a domain-separation label and a list of fields,
/// used for every key derivation `info`, AEAD associated data, MAC and
/// signature input:
///
/// ```text
/// u32be(len(label)) || label || for each field: u32be(len(field)) || field
/// ```
///
/// Labels are always `pastazzo/v1/<purpose>`, so a value authenticated for
/// one purpose can never be accepted for another.
pub fn transcript(label: &str, fields: &[&[u8]]) -> Vec<u8> {
    let len = 4 + label.len() + fields.iter().map(|field| 4 + field.len()).sum::<usize>();
    let mut out = Vec::with_capacity(len);
    push(&mut out, label.as_bytes());
    for field in fields {
        push(&mut out, field);
    }
    out
}

fn push(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).expect("transcript fields are far below 4 GiB");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::transcript;

    #[test]
    fn fields_cannot_be_shifted() {
        assert_ne!(
            transcript("l", &[b"ab", b"c"]),
            transcript("l", &[b"a", b"bc"])
        );
        assert_ne!(transcript("l", &[b"", b"x"]), transcript("l", &[b"x"]));
        assert_ne!(transcript("la", &[b"b"]), transcript("l", &[b"ab"]));
    }

    #[test]
    fn vector() {
        assert_eq!(
            transcript("pastazzo/v1/test", &[b"a", b""]),
            b"\0\0\0\x10pastazzo/v1/test\0\0\0\x01a\0\0\0\0".to_vec()
        );
    }
}
