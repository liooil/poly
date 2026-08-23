use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use serde::Deserialize;
use uv_distribution_filename::WheelFilename;
use uv_platform_tags::{AbiTag, LanguageTag, PlatformTag};
use uv_resolver::Lock;

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub requires_python: String,
    pub packages: Vec<LockedPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub filename: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct LockWire {
    requires_python: String,
    #[serde(default)]
    package: Vec<PackageWire>,
}

#[derive(Debug, Deserialize)]
struct PackageWire {
    name: String,
    version: Option<String>,
    #[serde(default)]
    source: BTreeMap<String, toml::Value>,
    #[serde(default)]
    wheels: Vec<WheelWire>,
}

#[derive(Debug, Deserialize)]
struct WheelWire {
    url: String,
    hash: Option<String>,
}

/// Validate a uv lockfile and select artifacts that can be staged for RustPython.
pub fn plan_lock(path: &Path) -> Result<InstallPlan, Error> {
    let contents = fs::read_to_string(path).map_err(|source| Error::ReadLock {
        path: path.to_path_buf(),
        source,
    })?;
    plan_lock_contents(&contents)
}

/// Build an install plan from validated uv lockfile contents.
///
/// The first implementation deliberately accepts only generic Python 3 wheels
/// with no ABI and no platform dependency. It does not build sdists or accept
/// CPython ABI wheels.
pub fn plan_lock_contents(contents: &str) -> Result<InstallPlan, Error> {
    let validated =
        Lock::from_toml(contents).map_err(|error| Error::InvalidLock(error.to_string()))?;
    let lock: LockWire = toml::from_str(contents)?;
    if validated.len() != lock.package.len() {
        return Err(Error::LockInvariant);
    }

    let mut packages = Vec::new();
    for package in lock.package {
        if !package.source.contains_key("registry") {
            continue;
        }

        let version = package.version.unwrap_or_else(|| "<unknown>".to_owned());
        let mut available = Vec::with_capacity(package.wheels.len());
        let mut selected = None;

        for wheel in package.wheels {
            let filename = wheel_filename_from_url(&wheel.url)?;
            available.push(filename.to_string());
            if selected.is_none() && is_generic_python3_wheel(&filename) {
                selected = Some((filename, wheel));
            }
        }

        let Some((filename, wheel)) = selected else {
            return Err(Error::NoPureWheel {
                name: package.name,
                version,
                available,
            });
        };
        let filename = filename.to_string();
        let Some(sha256) = wheel
            .hash
            .as_deref()
            .and_then(|hash| hash.strip_prefix("sha256:"))
            .map(ToOwned::to_owned)
        else {
            return Err(Error::MissingHash {
                name: package.name,
                version,
                filename,
            });
        };

        packages.push(LockedPackage {
            name: package.name,
            version,
            filename,
            url: wheel.url,
            sha256,
        });
    }

    packages.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(InstallPlan {
        requires_python: lock.requires_python,
        packages,
    })
}

fn wheel_filename_from_url(url: &str) -> Result<WheelFilename, Error> {
    let filename = url
        .split(['?', '#'])
        .next()
        .and_then(|url| url.rsplit('/').next())
        .unwrap_or(url);
    WheelFilename::from_str(filename).map_err(|error| Error::InvalidWheel {
        filename: filename.to_owned(),
        message: error.to_string(),
    })
}

pub(crate) fn is_generic_python3_wheel(filename: &WheelFilename) -> bool {
    filename.python_tags().iter().any(|tag| {
        matches!(
            tag,
            LanguageTag::Python {
                major: 3,
                minor: None
            }
        )
    }) && filename.abi_tags().contains(&AbiTag::None)
        && filename.platform_tags().contains(&PlatformTag::Any)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn selects_generic_python_wheel() {
        let lock = format!(
            r#"version = 1
revision = 3
requires-python = ">=3.12"

[[package]]
name = "click"
version = "8.3.1"
source = {{ registry = "https://pypi.org/simple" }}
wheels = [
    {{ url = "https://example.invalid/click-8.3.1-cp312-cp312-win_amd64.whl", hash = "sha256:{HASH}" }},
    {{ url = "https://example.invalid/click-8.3.1-py3-none-any.whl", hash = "sha256:{HASH}" }},
]

[[package]]
name = "project"
version = "0.1.0"
source = {{ virtual = "." }}
dependencies = [{{ name = "click" }}]
"#
        );

        let plan = plan_lock_contents(&lock).expect("build install plan");
        assert_eq!(plan.requires_python, ">=3.12");
        assert_eq!(plan.packages.len(), 1);
        assert_eq!(plan.packages[0].name, "click");
        assert_eq!(plan.packages[0].filename, "click-8.3.1-py3-none-any.whl");
        assert_eq!(plan.packages[0].sha256, HASH);
    }

    #[test]
    fn rejects_cpython_only_package() {
        let lock = format!(
            r#"version = 1
revision = 3
requires-python = ">=3.12"

[[package]]
name = "native-only"
version = "1.0.0"
source = {{ registry = "https://pypi.org/simple" }}
wheels = [
    {{ url = "https://example.invalid/native_only-1.0.0-cp312-cp312-win_amd64.whl", hash = "sha256:{HASH}" }},
]
"#
        );

        let error = plan_lock_contents(&lock).expect_err("reject native wheel");
        assert!(matches!(
            error,
            Error::NoPureWheel { ref name, .. } if name == "native-only"
        ));
    }

    #[test]
    fn lets_uv_reject_invalid_lockfile() {
        let error = plan_lock_contents("version = 999\nrequires-python = \">=3.12\"\n")
            .expect_err("reject unsupported uv lock version");
        assert!(matches!(error, Error::InvalidLock(_)));
    }
}
