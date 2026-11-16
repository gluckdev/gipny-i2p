//! Exact native Windows package selection shared by download and staging.
use super::{release_version, Component, Result, UpdateError};

pub(super) fn target_suffix(component: Component, arch: &str) -> Option<(&'static str, &'static str)> {
    match (component, arch) {
        (Component::App, "x86_64") => Some(("gipny-i2p_", "_x64-setup.exe")),
        (Component::App, "aarch64") => Some(("gipny-i2p_", "_arm64-setup.exe")),
        (Component::Agent, "x86_64") => Some(("gipny-agent_", "_windows-amd64.tar.gz")),
        (Component::Agent, "aarch64") => Some(("gipny-agent_", "_windows-arm64.tar.gz")),
        _ => None,
    }
}

pub(super) fn package_version<'a>(name: &'a str, component: Component, arch: &str) -> Result<&'a str> {
    let (prefix, suffix) = target_suffix(component, arch)
        .ok_or_else(|| UpdateError::Unsupported("unsupported native Windows architecture".into()))?;
    name.strip_prefix(prefix).and_then(|value| value.strip_suffix(suffix))
        .filter(|version| release_version(version).is_some())
        .ok_or_else(|| UpdateError::Unsupported("unexpected native Windows package name".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_version_rejects_other_architecture_component_and_bad_versions() {
        for component in [Component::App, Component::Agent] {
            for arch in ["x86_64", "aarch64"] {
                let (prefix, suffix) = target_suffix(component, arch).unwrap();
                let name = format!("{prefix}0.5.0{suffix}");
                assert_eq!(package_version(&name, component, arch).unwrap(), "0.5.0");
                let other_arch = if arch == "x86_64" { "aarch64" } else { "x86_64" };
                assert!(package_version(&name, component, other_arch).is_err());
                let other = if matches!(component, Component::App) { Component::Agent } else { Component::App };
                assert!(package_version(&name, other, arch).is_err());
                // The updater keeps its existing stable-release policy.
                for version in ["", "../0.5.0", "garbage", "0.5.0-beta.1", "0.5.0-extra.exe/"] {
                    assert!(package_version(&format!("{prefix}{version}{suffix}"), component, arch).is_err());
                }
            }
        }
        assert!(package_version("anything", Component::App, "x86").is_err());
    }
}
