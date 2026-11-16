//! Length bounds are checked while decoding, before trusting sequence hints.

use serde::{
    de::{Error, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use std::{fmt, marker::PhantomData, ops::Deref};

#[derive(Serialize)]
#[serde(transparent)]
pub struct List<T, const MAX: usize>(Vec<T>);

impl<T, const MAX: usize> List<T, MAX> {
    pub fn new(values: Vec<T>) -> Result<Self, super::Error> {
        if values.len() > MAX {
            return Err(super::Error::Limit);
        }
        Ok(Self(values))
    }
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
}

impl<T, const MAX: usize> Deref for List<T, MAX> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<'de, T: Deserialize<'de>, const MAX: usize> Deserialize<'de> for List<T, MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Bounded<T, const MAX: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const MAX: usize> Visitor<'de> for Bounded<T, MAX> {
            type Value = List<T, MAX>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "at most {MAX} entries")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let hint = seq.size_hint().unwrap_or(MAX);
                if hint > MAX {
                    return Err(A::Error::custom("sequence limit"));
                }
                // Bounded full capacity avoids moving secret-bearing entries
                // into a larger allocation while decoding crypto collections.
                let mut values = Vec::with_capacity(hint);
                while let Some(value) = seq.next_element()? {
                    if values.len() == MAX {
                        return Err(A::Error::custom("sequence limit"));
                    }
                    values.push(value);
                }
                Ok(List(values))
            }
        }
        d.deserialize_seq(Bounded::<T, MAX>(PhantomData))
    }
}
