//! Bounded UTF-8 fields. Plaintext is wiped even on partial decoding failures.
use super::Error;
use serde::{
    de::{Error as _, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use zeroize::Zeroizing;

pub struct Text<const MAX: usize>(Zeroizing<Vec<u8>>);

impl<const MAX: usize> Text<MAX> {
    pub fn new(value: &str) -> Result<Self, Error> {
        if value.len() > MAX {
            return Err(Error::Limit);
        }
        Ok(Self(Zeroizing::new(value.as_bytes().to_vec())))
    }
    pub fn as_str(&self) -> &str {
        // Both constructors enforce UTF-8; the backing bytes are private.
        std::str::from_utf8(&self.0).expect("validated portable text")
    }
}

impl<const MAX: usize> Serialize for Text<MAX> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for Text<MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Bytes<const MAX: usize>;
        impl<'de, const MAX: usize> Visitor<'de> for Bytes<MAX> {
            type Value = Text<MAX>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "at most {MAX} UTF-8 bytes")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let hint = seq.size_hint().unwrap_or(0);
                if hint > MAX {
                    return Err(A::Error::custom("text limit"));
                }
                let mut bytes = Zeroizing::new(Vec::with_capacity(hint));
                while let Some(byte) = seq.next_element::<u8>()? {
                    if bytes.len() == MAX {
                        return Err(A::Error::custom("text limit"));
                    }
                    if bytes.len() == bytes.capacity() {
                        let capacity = bytes.len().saturating_mul(2).max(64).min(MAX);
                        let mut larger = Zeroizing::new(Vec::with_capacity(capacity));
                        larger.extend_from_slice(&bytes);
                        bytes = larger;
                    }
                    bytes.push(byte);
                }
                if std::str::from_utf8(&bytes).is_err() {
                    return Err(A::Error::custom("invalid UTF-8"));
                }
                Ok(Text(bytes))
            }
        }
        // Bytes carry a bounded length before allocation. deserialize_str can
        // allocate from the untrusted prefix before invoking its visitor.
        d.deserialize_seq(Bytes::<MAX>)
    }
}
