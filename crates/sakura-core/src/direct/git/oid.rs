//! A git object id: the SHA-1 of an object. Repositories of SHA-256 are not read.

use std::fmt;

/// The twenty bytes that name an object.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid(pub [u8; 20]);

/// The tree with nothing in it, which every repository knows without storing it.
pub const EMPTY_TREE: Oid = Oid([
    0x4b, 0x82, 0x5d, 0xc6, 0x42, 0xcb, 0x6e, 0xb9, 0xa0, 0x60, 0xe5, 0x4b, 0xf8, 0xd6, 0x92, 0x88,
    0xbf, 0xee, 0x49, 0x04,
]);

impl Oid {
    /// The id written as forty lowercase hexadecimal digits; anything else is none.
    pub fn from_hex(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 40 {
            return None;
        }
        let mut id = [0u8; 20];
        for (slot, pair) in id.iter_mut().zip(bytes.chunks_exact(2)) {
            *slot = (digit(pair[0])? << 4) | digit(pair[1])?;
        }
        Some(Self(id))
    }

    /// The id from its twenty raw bytes, as trees and indexes hold it.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    /// The id of a blob holding `content`, as git computes it.
    pub fn of_blob(content: &[u8]) -> Self {
        let mut hash = sha1_smol::Sha1::new();
        hash.update(format!("blob {}\0", content.len()).as_bytes());
        hash.update(content);
        Self(hash.digest().bytes())
    }

    /// The forty hexadecimal digits.
    pub fn hex(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({self})")
    }
}

fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
