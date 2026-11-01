//! WASM secret ownership for the browser-only core.
//!
//! The browser provides no mlock/mprotect or core-dump exclusion. This module
//! makes no OS-page protection claim. Secrets stay in WASM memory and are
//! zeroized when the last owner is dropped; the app uses a dedicated Worker
//! and encrypted OPFS records. Native targets keep memory.rs unchanged.
use rand::rand_core::TryRng;
use std::{ops::Deref, sync::Arc};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("unsupported browser secret size")]
    Size,
    #[error("browser cryptographic randomness unavailable")]
    Random,
}

pub(crate) enum SecretKind {
    Identity,
    Ratchet,
    PreKey,
}

pub(crate) struct Sealed<const N: usize>(Arc<Zeroizing<[u8; N]>>);
impl<const N: usize> Clone for Sealed<N> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<const N: usize> Sealed<N> {
    pub(crate) fn from_slice(_: SecretKind, bytes: &[u8]) -> Result<Self, MemoryError> {
        if N == 0 || N > 2048 || bytes.len() != N {
            return Err(MemoryError::Size);
        }
        let mut value = Zeroizing::new([0; N]);
        value.copy_from_slice(bytes);
        Ok(Self(Arc::new(value)))
    }

    pub(crate) fn random(kind: SecretKind) -> Result<Self, MemoryError> {
        let mut value = Zeroizing::new([0; N]);
        rand::rngs::SysRng
            .try_fill_bytes(value.as_mut_slice())
            .map_err(|_| MemoryError::Random)?;
        Self::from_slice(kind, value.as_slice())
    }

    pub(crate) fn open(&self) -> Result<SecretGuard<'_, N>, MemoryError> {
        Ok(SecretGuard(&self.0))
    }
}

pub struct SecretGuard<'a, const N: usize>(&'a Zeroizing<[u8; N]>);
impl<const N: usize> Deref for SecretGuard<'_, N> {
    type Target = [u8; N];
    fn deref(&self) -> &Self::Target {
        self.0
    }
}
