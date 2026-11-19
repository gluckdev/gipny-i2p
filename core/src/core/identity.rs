//! Identity, display name, and prekey lifecycle.

use super::*;

impl Core {
    pub(super) fn load_or_create_identity(db: &Db) -> Result<Identity> {
        match (
            db.get_setting(SETTING_IDENTITY_SIGN)?
                .map(zeroize::Zeroizing::new),
            db.get_setting(SETTING_IDENTITY_DH)?
                .map(zeroize::Zeroizing::new),
        ) {
            (Some(s), Some(d)) if s.len() == 32 && d.len() == 32 => {
                let mut sign = zeroize::Zeroizing::new([0u8; 32]);
                sign.copy_from_slice(&s);
                let mut dh = zeroize::Zeroizing::new([0u8; 32]);
                dh.copy_from_slice(&d);
                Ok(Identity::try_from_bytes(*sign, *dh)?)
            }
            _ => {
                let id = Identity::try_generate()?;
                id.with_secret_bytes(|sign, dh| -> gipny_libcore::db::Result<()> {
                    db.set_setting(SETTING_IDENTITY_SIGN, sign)?;
                    db.set_setting(SETTING_IDENTITY_DH, dh)?;
                    Ok(())
                })??;
                Ok(id)
            }
        }
    }

    pub fn my_card(&self) -> IdentityCard {
        self.identity.card()
    }

    pub fn my_fingerprint(&self) -> [u8; 32] {
        self.identity.fingerprint()
    }

    pub fn my_i2p_address(&self) -> &str {
        &self.personal_address
    }

    pub fn my_b32(&self) -> String {
        gipny_libcore::card::compact_address(&self.relay_address()).unwrap_or_default()
    }

    pub fn display_name(&self) -> Result<String> {
        let v = self.db.get_setting("display_name")?;
        Ok(v.and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default())
    }

    pub(super) fn outgoing_sender_name(&self) -> Option<String> {
        let user = self
            .db
            .get_setting("display_name")
            .ok()
            .flatten()
            .and_then(|b| String::from_utf8(b).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Some(user.unwrap_or_else(|| hex_short(&self.identity.card().sign_pk)))
    }

    pub(super) async fn ensure_prekeys(&self) -> Result<()> {
        let id = self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?;
        if id.is_none() {
            let pair = PreKeyPair::generate();
            let secret = pair.expose_secret()?;
            let pid = self
                .db
                .add_prekey(PreKeyKind::Signed, &*secret, pair.public())?;
            self.db
                .set_setting(SETTING_SIGNED_PREKEY_ID, &pid.to_be_bytes())?;
        }
        let count = self.db.count_prekeys(PreKeyKind::OneTime)?;
        for _ in count..(TARGET_OPK as i64) {
            let pair = PreKeyPair::generate();
            let secret = pair.expose_secret()?;
            self.db
                .add_prekey(PreKeyKind::OneTime, &*secret, pair.public())?;
        }
        Ok(())
    }

    pub fn my_bundle(&self) -> Result<PreKeyBundle> {
        let signed_id = i64::from_be_bytes(
            self.db
                .get_setting(SETTING_SIGNED_PREKEY_ID)?
                .ok_or(CoreError::State)?
                .try_into()
                .map_err(|_| CoreError::State)?,
        );
        let signed = self.db.get_prekey(signed_id)?.ok_or(CoreError::State)?;
        let signed_pair = PreKeyPair::from_secret(to_arr32(signed.private.clone())?);
        let opk = self.db.peek_oldest_prekey(PreKeyKind::OneTime)?;
        let opk_pair = opk.as_ref().map(|p| {
            let sk = to_arr32(p.private.clone()).expect("prekey private size");
            (p.id, PreKeyPair::from_secret(sk))
        });
        let bundle = PreKeyBundle::new(
            &self.identity,
            &signed_pair,
            opk_pair.as_ref().map(|(id, kp)| (*id, kp)),
        );
        Ok(bundle)
    }
}
