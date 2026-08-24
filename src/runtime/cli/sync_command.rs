use std::path::PathBuf;

use bstr::BStr;
use bun_core::{self as bun, Global, Output};

use crate::cli::command::{Tag, tag_print_help};

pub(crate) struct SyncCommand;

#[derive(Debug, PartialEq, Eq)]
struct Options {
    frozen: bool,
    check: bool,
    project: Option<PathBuf>,
    help: bool,
}

impl SyncCommand {
    pub(crate) fn exec() -> Result<(), crate::Error> {
        let argv = bun::argv();
        let args = argv.iter().skip(2).collect::<Vec<_>>();
        let options = parse_args(&args).unwrap_or_else(|message| {
            bun_core::pretty_errorln!("<r><red>error<r><d>:<r> {}", message);
            bun_core::pretty_errorln!("Run <b>poly sync --help<r> for usage.");
            Output::flush();
            Global::exit(1);
        });
        if options.help {
            tag_print_help(Tag::SyncCommand, true);
            Global::exit(0);
        }
        let frozen = options.frozen;
        let check = options.check;
        let project = match options.project {
            Some(path) => path,
            None => std::env::current_dir().unwrap_or_else(|error| {
                bun_core::pretty_errorln!(
                    "<r><red>error<r><d>:<r> cannot read the current directory: {}",
                    error
                );
                Output::flush();
                Global::exit(1);
            }),
        };
        let executable = std::env::current_exe().unwrap_or_else(|error| {
            bun_core::pretty_errorln!(
                "<r><red>error<r><d>:<r> cannot locate the Poly executable: {}",
                error
            );
            Output::flush();
            Global::exit(1);
        });
        let mut options = poly_uv::SyncOptions::new(project, executable);
        options.smoke_check = check;
        let report = if frozen {
            poly_uv::sync_frozen(&options)
        } else {
            poly_uv::sync(&options)
        };
        let report = report.unwrap_or_else(|error| {
            bun_core::pretty_errorln!("<r><red>error<r><d>:<r> {}", error);
            Output::flush();
            Global::exit(1);
        });

        bun_core::prettyln!(
            "<green>Synced<r> {} Python package{} to <b>{}<r> <d>({} downloaded, {} cached)<r>",
            report.packages.len(),
            if report.packages.len() == 1 { "" } else { "s" },
            report.target.display(),
            report.downloaded,
            report.cache_hits,
        );
        if report.compatibility.smoke_checked {
            let import_count = report.compatibility.import_count();
            let compatible = report
                .compatibility
                .count(poly_uv::CompatibilityStatus::SmokeCompatible);
            let unverified = report
                .compatibility
                .count(poly_uv::CompatibilityStatus::Unverified);
            bun_core::prettyln!(
                "<green>Checked<r> {} Python import{} across {} package{} <d>({} compatible, {} unverified)<r>",
                import_count,
                if import_count == 1 { "" } else { "s" },
                report.compatibility.packages.len(),
                if report.compatibility.packages.len() == 1 {
                    ""
                } else {
                    "s"
                },
                compatible,
                unverified,
            );
        } else {
            let compatible = report
                .compatibility
                .count(poly_uv::CompatibilityStatus::StaticCompatible);
            let unverified = report
                .compatibility
                .count(poly_uv::CompatibilityStatus::Unverified);
            bun_core::prettyln!(
                "<green>Scanned<r> {} Python package{} for RustPython compatibility <d>({} compatible, {} unverified)<r>",
                report.compatibility.packages.len(),
                if report.compatibility.packages.len() == 1 {
                    ""
                } else {
                    "s"
                },
                compatible,
                unverified,
            );
        }
        for package in &report.compatibility.packages {
            if package.status != poly_uv::CompatibilityStatus::Unverified {
                continue;
            }
            for diagnostic in &package.diagnostics {
                if diagnostic.kind == poly_uv::CompatibilityDiagnosticKind::NoImportTargets {
                    bun_core::pretty_errorln!(
                        "<yellow>warn<r><d>:<r> {}=={}: {}",
                        package.name,
                        package.version,
                        diagnostic.message,
                    );
                }
            }
        }
        for warning in report.warnings {
            bun_core::pretty_errorln!("<yellow>warn<r><d>:<r> {}", warning);
        }
        Output::flush();
        Ok(())
    }
}

fn parse_args(args: &[&[u8]]) -> Result<Options, String> {
    let mut options = Options {
        frozen: false,
        check: false,
        project: None,
        help: false,
    };
    let mut index = 0;
    while index < args.len() {
        let argument = args[index];
        match argument {
            b"--frozen" => options.frozen = true,
            b"--check" => options.check = true,
            b"-h" | b"--help" => options.help = true,
            b"--project" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--project requires a path".to_owned())?;
                options.project = Some(path_from_bytes(value)?);
            }
            _ if argument.starts_with(b"--project=") => {
                options.project = Some(path_from_bytes(&argument[b"--project=".len()..])?);
            }
            _ => {
                return Err(format!("unexpected argument `{}`", BStr::new(argument)));
            }
        }
        index += 1;
    }
    Ok(options)
}

fn path_from_bytes(value: &[u8]) -> Result<PathBuf, String> {
    if value.is_empty() {
        return Err("--project requires a non-empty path".to_owned());
    }
    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        return Ok(PathBuf::from(OsString::from_vec(value.to_vec())));
    }
    #[cfg(not(unix))]
    {
        let value = std::str::from_utf8(value)
            .map_err(|_| "--project path must be valid UTF-8".to_owned())?;
        Ok(PathBuf::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frozen_checked_project() {
        assert_eq!(
            parse_args(&[
                b"--frozen".as_slice(),
                b"--check".as_slice(),
                b"--project".as_slice(),
                b"example".as_slice(),
            ]),
            Ok(Options {
                frozen: true,
                check: true,
                project: Some(PathBuf::from("example")),
                help: false,
            })
        );
    }

    #[test]
    fn rejects_unknown_arguments() {
        assert!(parse_args(&[b"--offline".as_slice()]).is_err());
    }
}
