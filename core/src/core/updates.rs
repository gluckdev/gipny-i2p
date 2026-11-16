use std::sync::Arc;
use std::time::Duration;

use super::*;

impl Core {
    pub(super) fn spawn_update_loop(self: Arc<Self>) {
        // No local HTTP proxy this run (Android, or attached to a router we
        // don't own) — nothing to dial, so don't even try.
        if !self.updater.is_configured() {
            gipny_libcore::debug_trace!(
                "[update] no local HTTP proxy this run — auto-update disabled"
            );
            return;
        }
        let this = self.clone();
        let handle = tokio::spawn(async move {
            use gipny_libcore::update::{UPDATE_CHECK_INITIAL_SECS, UPDATE_CHECK_INTERVAL_SECS};
            tokio::time::sleep(Duration::from_secs(UPDATE_CHECK_INITIAL_SECS)).await;
            // A failed check is retried soon, not in six hours. The first one
            // lands half a minute after start, when a freshly woken router
            // often has no client tunnels yet and the request through the
            // outproxy simply fails — and a phone rarely stays running long
            // enough to see the next six-hourly attempt. So: a minute, five,
            // twenty, an hour, and only then the ordinary rhythm.
            const RETRY_SECS: [u64; 4] = [60, 300, 1_200, 3_600];
            let mut failures = 0usize;
            loop {
                let ok = this.clone().check_and_emit_update().await.is_ok();
                let wait = if ok {
                    failures = 0;
                    UPDATE_CHECK_INTERVAL_SECS
                } else {
                    let w = RETRY_SECS[failures.min(RETRY_SECS.len() - 1)];
                    failures += 1;
                    w
                };
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }
}

impl Core {
    /// Whether a found update installs by itself.
    ///
    /// Enabled by default; honor the saved preference and the explicit
    /// `GIPNY_NO_AUTO_UPDATE=1` developer override on every check.
    pub fn auto_update_enabled(&self) -> bool {
        let Ok(setting) = self.db.get_setting(SETTING_AUTO_UPDATE) else {
            return false;
        };
        gipny_libcore::update::auto_update_allowed(
            setting.as_deref(),
            std::env::var("GIPNY_NO_AUTO_UPDATE").ok().as_deref(),
        )
    }

    /// Save the same preference read by automatic installation checks.
    pub fn set_auto_update(&self, enabled: bool) -> Result<()> {
        self.db
            .set_setting(SETTING_AUTO_UPDATE, if enabled { b"1" } else { b"0" })?;
        Ok(())
    }

    /// Checks GitHub for a newer release. With auto-update on (the default)
    /// and a matching asset, this prepares supported unattended updates before
    /// returning. Platforms requiring system consent or a package-manager
    /// password emit UpdateAvailable for the person to start installation.
    pub async fn check_and_emit_update(self: Arc<Self>) -> Result<Option<UpdateInfo>> {
        Ok(match self.check_update_detailed().await? {
            gipny_libcore::update::CheckOutcome::Update(i) => Some(i),
            _ => None,
        })
    }

    /// The check a person pressed a button for: it reports what it found,
    /// including each of the reasons there is nothing to install.
    pub async fn check_update_detailed(
        self: Arc<Self>,
    ) -> Result<gipny_libcore::update::CheckOutcome> {
        use gipny_libcore::update::CheckOutcome;
        let outcome = match self.updater.check_detailed(env!("CARGO_PKG_VERSION")).await {
            Ok(o) => o,
            Err(e) => {
                eprintln!("[update] check err: {:?}", e);
                return Err(e.into());
            }
        };
        let info = match outcome {
            CheckOutcome::Update(i) => i,
            other => return Ok(other),
        };
        if let Ok(Some(v)) = self.db.get_setting(SETTING_DISMISSED_UPDATE) {
            if String::from_utf8_lossy(&v) == info.version {
                // Already offered and put aside — which is not the same as
                // being current, and the interface no longer says it is.
                return Ok(CheckOutcome::Dismissed {
                    version: info.version,
                });
            }
        }
        *self.pending_update.lock().await = Some(info.clone());
        // .deb installs need a person because the package manager asks for the
        // administrator password. A password dialog appearing by itself, with no
        // context, is not an update — it is something people rightly refuse.
        // Android and macOS also need explicit system/package handoff.
        let needs_a_person = gipny_libcore::update::is_deb_install()
            || cfg!(any(target_os = "macos", target_os = "android"));
        if self.auto_update_enabled() && !needs_a_person {
            // Propagate install failures to the same retry/backoff loop as
            // transport errors instead of silently waiting another six hours.
            self.clone().install_update().await?;
        } else {
            let _ = self.events.try_send(CoreEvent::UpdateAvailable {
                version: info.version.clone(),
                notes: info.notes.clone(),
                size: info.asset.size,
            });
        }
        Ok(CheckOutcome::Update(info))
    }

    /// Downloads and installs the pending update (from `check_and_emit_update`
    /// or a manual "Update now" click). Completed preparation is remembered
    /// for this process; a system handoff never dismisses the release.
    pub async fn install_update(self: Arc<Self>) -> Result<()> {
        let _installation = self.update_install.lock().await;
        let info = self
            .pending_update
            .lock()
            .await
            .clone()
            .ok_or(CoreError::NotFound)?;
        if self.prepared_update.lock().await.as_deref() == Some(info.version.as_str()) {
            return Ok(());
        }
        let ev = self.events.clone();
        let ev_dl = ev.clone();
        let dl_dir = self.data_dir.join("update_dl");

        let path = match self
            .updater
            .download(&info, &dl_dir, move |done, t| {
                let pct = if t > 0 {
                    ((done * 100) / t).min(100) as u8
                } else {
                    0
                };
                let _ = ev_dl.try_send(CoreEvent::UpdateProgress {
                    downloaded: done,
                    total: t,
                    pct,
                });
            })
            .await
        {
            Ok(p) => p,
            Err(e) => {
                let _ = ev
                    .send(CoreEvent::UpdateFailed {
                        reason: e.to_string(),
                    })
                    .await;
                return Err(e.into());
            }
        };

        #[cfg(target_os = "android")]
        {
            let staged = crate::update_staging::stage_for_install(&path, &info.sha256.to_string());
            let apk_path = match staged {
                Ok(path) => path,
                Err(error) => {
                    let _ = ev
                        .send(CoreEvent::UpdateFailed {
                            reason: error.to_string(),
                        })
                        .await;
                    return Err(error.into());
                }
            };
            let install_result = match apk_path.to_str() {
                Some(path) => install_apk(path).await,
                None => Err("APK cache path is not valid UTF-8".into()),
            };
            match install_result {
                Ok(_) => {
                    // Android still needs the cache copy after startActivity.
                    // Leave the verified download available for another caller
                    // already staging the same release. The updater replaces it
                    // atomically on the next download; the URI uses its own copy.
                    // Starting Android's UI does not prove installation. A
                    // cancelled permission/install prompt must remain retryable.
                    let _ = ev
                        .send(CoreEvent::UpdateInstallerOpened {
                            version: info.version,
                            path: apk_path.display().to_string(),
                        })
                        .await;
                    return Ok(());
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&apk_path);
                    eprintln!("[update] Android APK install failed: {e}");
                    let _ = ev
                        .send(CoreEvent::UpdateFailed {
                            reason: format!("APK install failed: {e}"),
                        })
                        .await;
                    return Err(CoreError::Update(
                        gipny_libcore::update::UpdateError::Unsupported(e),
                    ));
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let outcome = match self.updater.install(&path, &self.data_dir, None) {
                Ok(o) => o,
                Err(e) => {
                    let _ = ev
                        .send(CoreEvent::UpdateFailed {
                            reason: e.to_string(),
                        })
                        .await;
                    return Err(e.into());
                }
            };
            match outcome {
                InstallOutcome::InstalledNow | InstallOutcome::StagedForNextLaunch => {
                    *self.prepared_update.lock().await = Some(info.version.clone());
                    let _ = ev
                        .send(CoreEvent::UpdateStaged {
                            version: info.version,
                        })
                        .await;
                }
                InstallOutcome::InstallerOpened(path) => {
                    let _ = ev
                        .send(CoreEvent::UpdateInstallerOpened {
                            version: info.version,
                            path: path.display().to_string(),
                        })
                        .await;
                }
                InstallOutcome::Unsupported(msg) => {
                    let _ = ev
                        .send(CoreEvent::UpdateFailed {
                            reason: msg.clone(),
                        })
                        .await;
                    return Err(UpdateError::Unsupported(msg).into());
                }
            }
            Ok(())
        }
    }

    pub async fn dismiss_update(&self, version: String) -> Result<()> {
        self.db
            .set_setting(SETTING_DISMISSED_UPDATE, version.as_bytes())?;
        *self.pending_update.lock().await = None;
        Ok(())
    }

    pub async fn list_apk_artifacts(&self) -> Result<(String, Vec<(String, u64)>)> {
        let release = self.updater.latest_release().await?;
        let mut out = Vec::new();
        for arch in ["arm64", "armv7"] {
            let name = format!("gipny-i2p_{}_android-{arch}.apk", release.version);
            if let Some(asset) = gipny_libcore::update::unique_asset(&release.assets, &name)? {
                out.push((arch.to_string(), asset.size));
            }
        }
        out.sort();
        Ok((release.version, out))
    }

    pub async fn download_apk(self: Arc<Self>, arch: String, dest_path: String) -> Result<()> {
        if !matches!(arch.as_str(), "arm64" | "armv7") {
            return Err(CoreError::NotFound);
        }
        let release = self.updater.latest_release().await?;
        let suffix = format!("_android-{arch}.apk");
        let wanted = format!("gipny-i2p_{}{suffix}", release.version);
        let asset = gipny_libcore::update::unique_asset(&release.assets, &wanted)?
            .ok_or(CoreError::NotFound)?
            .clone();
        let ev = self.events.clone();
        let ev_dl = ev.clone();
        let dest = std::path::PathBuf::from(dest_path);
        let sha256 = match self.updater.asset_sha256(&release, &asset.name).await {
            Ok(sha256) => sha256,
            Err(e) => {
                let _ = ev
                    .send(CoreEvent::UpdateFailed {
                        reason: e.to_string(),
                    })
                    .await;
                return Err(e.into());
            }
        };
        match self
            .updater
            .download_asset_to(&asset, &sha256, &dest, move |done, t| {
                let pct = if t > 0 {
                    ((done * 100) / t).min(100) as u8
                } else {
                    0
                };
                let _ = ev_dl.try_send(CoreEvent::UpdateProgress {
                    downloaded: done,
                    total: t,
                    pct,
                });
            })
            .await
        {
            Ok(_) => {
                let _ = ev
                    .send(CoreEvent::UpdateReady {
                        path: dest.display().to_string(),
                    })
                    .await;
                Ok(())
            }
            Err(e) => {
                let _ = ev
                    .send(CoreEvent::UpdateFailed {
                        reason: e.to_string(),
                    })
                    .await;
                Err(e.into())
            }
        }
    }

    /// Whether auto-update can work at all (see [`Updater::is_configured`]).
    pub fn update_configured(&self) -> bool {
        self.updater.is_configured()
    }
}
