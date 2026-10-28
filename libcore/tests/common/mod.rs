//! Integration-only protected credential fixture. The production provider has
//! no file fallback; these secrets live outside the temporary profile trees.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Once};

use gipny_libcore::device_binding::{self, BindingError, DeviceKeyProvider, Result};
use zeroize::Zeroizing;

#[derive(Default)]
struct Credentials(Mutex<HashMap<String, Zeroizing<[u8; 32]>>>);

impl DeviceKeyProvider for Credentials {
    fn kind(&self) -> &'static str { "integration-test-os-store" }
    fn seal(&self, id: &str, secret: &[u8; 32]) -> Result<Vec<u8>> {
        self.0.lock().unwrap().insert(id.into(), Zeroizing::new(*secret));
        Ok(Vec::new())
    }
    fn unseal(&self, id: &str, _wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        self.0.lock().unwrap().get(id).map(|key| Zeroizing::new(**key))
            .ok_or(BindingError::MissingCredential)
    }
    fn delete(&self, id: &str) -> Result<()> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

pub fn protected_credentials() {
    static INIT: Once = Once::new();
    INIT.call_once(|| device_binding::install_provider(Arc::new(Credentials::default())).unwrap());
}
