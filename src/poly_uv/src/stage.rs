use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use fs_err::File;
use uv_distribution_filename::WheelFilename;
use uv_extract::hash::{HashReader, Hasher};
use uv_install_wheel::{InstallState, Layout, LinkMode};
use uv_preview::Preview;
use uv_pypi_types::{HashAlgorithm, HashDigest, Scheme};

use crate::lock::is_generic_python3_wheel;
use crate::{Error, LockedPackage};

/// Verify a wheel against its uv lock entry, then install it into Poly's
/// Python staging directory.
pub fn stage_locked_wheel(
    package: &LockedPackage,
    archive: &Path,
    site_packages: &Path,
    sys_executable: &Path,
    python_version: (u8, u8),
) -> Result<PathBuf, Error> {
    let actual_filename = archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::InvalidWheel {
            filename: archive.display().to_string(),
            message: "wheel path has no UTF-8 filename".to_owned(),
        })?;
    if actual_filename != package.filename {
        return Err(Error::WheelFilenameMismatch {
            expected: package.filename.clone(),
            actual: actual_filename.to_owned(),
        });
    }

    let actual_hash = wheel_sha256(archive)?;
    if !actual_hash.eq_ignore_ascii_case(&package.sha256) {
        return Err(Error::HashMismatch {
            filename: package.filename.clone(),
            expected: package.sha256.clone(),
            actual: actual_hash,
        });
    }

    stage_wheel_archive(archive, site_packages, sys_executable, python_version)
}

/// Extract and install a pure wheel into Poly's Python staging directory.
///
/// `sys_executable` is the Poly executable used for generated console scripts;
/// no Python interpreter or virtual environment is discovered or launched.
pub fn stage_wheel_archive(
    archive: &Path,
    site_packages: &Path,
    sys_executable: &Path,
    python_version: (u8, u8),
) -> Result<PathBuf, Error> {
    let archive_name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::InvalidWheel {
            filename: archive.display().to_string(),
            message: "wheel path has no UTF-8 filename".to_owned(),
        })?;
    let filename = WheelFilename::from_str(archive_name).map_err(|error| Error::InvalidWheel {
        filename: archive_name.to_owned(),
        message: error.to_string(),
    })?;
    if !is_generic_python3_wheel(&filename) {
        return Err(Error::IncompatibleWheel(filename.to_string()));
    }

    let staging_root = site_packages.parent().unwrap_or(site_packages);
    let layout = Layout {
        sys_executable: sys_executable.to_path_buf(),
        python_version,
        os_name: if cfg!(windows) { "nt" } else { "posix" }.to_owned(),
        scheme: Scheme {
            purelib: site_packages.to_path_buf(),
            platlib: site_packages.to_path_buf(),
            scripts: staging_root.join("bin"),
            data: staging_root.join("data"),
            include: staging_root.join("include"),
        },
    };
    fs::create_dir_all(site_packages)?;

    let unpacked = tempfile::tempdir()?;
    uv_extract::unzip(File::open(archive)?, unpacked.path())?;
    let dist_info = uv_install_wheel::installed_dist_info_path(&layout, unpacked.path())?;
    let state = InstallState::new(Preview::default());
    uv_install_wheel::install_wheel::<(), ()>(
        &layout,
        true,
        unpacked.path(),
        &filename,
        None,
        None,
        None,
        Some("poly"),
        true,
        LinkMode::Copy,
        &state,
    )?;
    Ok(dist_info)
}

fn wheel_sha256(path: &Path) -> Result<String, Error> {
    let executor = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(Error::Executor)?;
    executor.block_on(async {
        let file = tokio::fs::File::open(path).await?;
        let mut hashers = [Hasher::from(HashAlgorithm::Sha256)];
        {
            let mut reader = HashReader::new(file, &mut hashers);
            reader.finish().await?;
        }
        let [hasher] = hashers;
        Ok(HashDigest::from(hasher).digest.to_string())
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;

    #[test]
    fn stages_pure_wheel_without_python_environment() {
        let directory = tempfile::tempdir().expect("create wheel fixture directory");
        let archive = directory.path().join("demo_pkg-1.0.0-py3-none-any.whl");
        let file = std::fs::File::create(&archive).expect("create wheel archive");
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        for (path, contents) in [
            ("demo_pkg/__init__.py", "VALUE = 42\n"),
            (
                "demo_pkg-1.0.0.dist-info/WHEEL",
                "Wheel-Version: 1.0\nGenerator: poly-test\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
            ),
            (
                "demo_pkg-1.0.0.dist-info/METADATA",
                "Metadata-Version: 2.1\nName: demo-pkg\nVersion: 1.0.0\n",
            ),
            (
                "demo_pkg-1.0.0.dist-info/RECORD",
                "demo_pkg/__init__.py,,\ndemo_pkg-1.0.0.dist-info/WHEEL,,\ndemo_pkg-1.0.0.dist-info/METADATA,,\ndemo_pkg-1.0.0.dist-info/RECORD,,\n",
            ),
        ] {
            writer
                .start_file(path, options)
                .expect("start wheel member");
            writer
                .write_all(contents.as_bytes())
                .expect("write wheel member");
        }
        writer.finish().expect("finish wheel archive");

        let package = LockedPackage {
            name: "demo-pkg".to_owned(),
            version: "1.0.0".to_owned(),
            filename: "demo_pkg-1.0.0-py3-none-any.whl".to_owned(),
            url: "https://example.invalid/demo_pkg-1.0.0-py3-none-any.whl".to_owned(),
            sha256: wheel_sha256(&archive).expect("hash wheel fixture"),
        };
        let site_packages = directory.path().join("staging/python");
        let executable = std::env::current_exe().expect("test executable path");
        let dist_info =
            stage_locked_wheel(&package, &archive, &site_packages, &executable, (3, 14))
                .expect("stage pure wheel");

        assert_eq!(
            fs::read_to_string(site_packages.join("demo_pkg/__init__.py"))
                .expect("read installed module"),
            "VALUE = 42\n"
        );
        assert_eq!(
            fs::read_to_string(dist_info.join("INSTALLER")).expect("read installer metadata"),
            "poly"
        );
    }

    #[test]
    fn rejects_wheel_that_does_not_match_lock_hash() {
        let directory = tempfile::tempdir().expect("create wheel fixture directory");
        let archive = directory.path().join("demo_pkg-1.0.0-py3-none-any.whl");
        fs::write(&archive, b"not the locked wheel").expect("write wheel fixture");
        let package = LockedPackage {
            name: "demo-pkg".to_owned(),
            version: "1.0.0".to_owned(),
            filename: "demo_pkg-1.0.0-py3-none-any.whl".to_owned(),
            url: "https://example.invalid/demo_pkg-1.0.0-py3-none-any.whl".to_owned(),
            sha256: "00".repeat(32),
        };

        let error = stage_locked_wheel(
            &package,
            &archive,
            &directory.path().join("staging/python"),
            &std::env::current_exe().expect("test executable path"),
            (3, 14),
        )
        .expect_err("reject hash mismatch");
        assert!(matches!(error, Error::HashMismatch { .. }));
    }
}
