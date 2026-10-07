//! Minimal reader for the fixed binary layouts in `docs/PROTOCOL.md`.

use crate::Error;

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    what: &'static str,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self { bytes, what }
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let (head, rest) = self
            .bytes
            .split_first_chunk::<N>()
            .ok_or(Error::Malformed(self.what))?;
        self.bytes = rest;
        Ok(*head)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }

    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    /// A `u32be` length followed by that many bytes.
    pub(crate) fn bytes(&mut self) -> Result<&'a [u8], Error> {
        let len = self.u32()? as usize;
        if len > self.bytes.len() {
            return Err(Error::Malformed(self.what));
        }
        let (head, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Ok(head)
    }

    pub(crate) fn rest(self) -> &'a [u8] {
        self.bytes
    }

    pub(crate) fn finish(self) -> Result<(), Error> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(Error::Malformed(self.what))
        }
    }
}

pub(crate) fn push_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).expect("protocol fields are far below 4 GiB");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
}

/// A protocol version byte that must match ours.
pub(crate) fn check_version(version: u8, what: &'static str) -> Result<(), Error> {
    if version == crate::PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(Error::Malformed(what))
    }
}
