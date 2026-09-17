use std::path::PathBuf;

use anyhow::{anyhow, Result};
use flexpart_gpu::reference::{verify_checkout, ReferenceManifest};

const USAGE: &str = "\
FLEXPART oracle reference checkout verification.

Usage:
  cargo run --bin reference-check -- verify --checkout <dir> [--manifest <file>]
  cargo run --bin reference-check -- show
  cargo run --bin reference-check -- --help

Commands:
  verify             Fail-closed check that <dir> is an unmodified upstream
                     tree at the pinned FLEXPART 11.1 commit.
  show               Print the bundled oracle manifest (JSON).

Options:
  --checkout <dir>   Reference checkout directory to verify.
  --manifest <file>  Manifest file (default: bundled reference/flexpart-11.1.json).
  -h, --help         Show this help
";

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifyOptions {
    checkout: PathBuf,
    manifest: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    Help,
    Show,
    Verify(VerifyOptions),
}

fn parse_cli_args<I>(args: I) -> Result<CliCommand>
where
    I: IntoIterator<Item = String>,
{
    let collected: Vec<String> = args.into_iter().collect();
    if collected.is_empty() {
        return Err(anyhow!("missing command (expected `verify` or `show`)"));
    }
    if collected.iter().any(|a| a == "-h" || a == "--help") {
        return Ok(CliCommand::Help);
    }
    match collected[0].as_str() {
        "show" => Ok(CliCommand::Show),
        "verify" => {
            let mut checkout: Option<PathBuf> = None;
            let mut manifest: Option<PathBuf> = None;
            let mut iter = collected.iter().skip(1);
            while let Some(argument) = iter.next() {
                match argument.as_str() {
                    "--checkout" => {
                        let value = iter
                            .next()
                            .ok_or_else(|| anyhow!("missing value after --checkout"))?;
                        checkout = Some(PathBuf::from(value));
                    }
                    "--manifest" => {
                        let value = iter
                            .next()
                            .ok_or_else(|| anyhow!("missing value after --manifest"))?;
                        manifest = Some(PathBuf::from(value));
                    }
                    _ if argument.starts_with("--checkout=") => {
                        let (_, value) = argument
                            .split_once('=')
                            .ok_or_else(|| anyhow!("invalid --checkout argument"))?;
                        checkout = Some(PathBuf::from(value));
                    }
                    _ if argument.starts_with("--manifest=") => {
                        let (_, value) = argument
                            .split_once('=')
                            .ok_or_else(|| anyhow!("invalid --manifest argument"))?;
                        manifest = Some(PathBuf::from(value));
                    }
                    _ => return Err(anyhow!("unknown argument: {argument}")),
                }
            }
            let checkout = checkout.ok_or_else(|| anyhow!("`verify` requires --checkout <dir>"))?;
            Ok(CliCommand::Verify(VerifyOptions { checkout, manifest }))
        }
        _ => Err(anyhow!("unknown command: {}", collected[0])),
    }
}

fn run() -> Result<()> {
    env_logger::init();
    match parse_cli_args(std::env::args().skip(1).collect::<Vec<_>>())? {
        CliCommand::Help => {
            println!("{USAGE}");
            Ok(())
        }
        CliCommand::Show => {
            let manifest = ReferenceManifest::bundled()?;
            println!("{}", serde_json::to_string_pretty(&manifest)?);
            Ok(())
        }
        CliCommand::Verify(options) => {
            let manifest = match &options.manifest {
                Some(path) => ReferenceManifest::load(path)?,
                None => ReferenceManifest::bundled()?,
            };
            let verified = verify_checkout(&manifest, &options.checkout)?;
            println!("reference checkout: OK");
            println!("path: {}", verified.path.display());
            println!("commit: {}", verified.commit);
            println!("tag: {}", manifest.pinned_tag);
            Ok(())
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("reference checkout: FAILED");
        eprintln!("{error:#}");
        eprintln!("{USAGE}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cli_args_requires_command() {
        assert!(parse_cli_args(Vec::<String>::new()).is_err());
    }

    #[test]
    fn test_parse_cli_args_show() {
        let parsed = parse_cli_args(vec!["show".to_string()]).expect("show should parse");
        assert_eq!(parsed, CliCommand::Show);
    }

    #[test]
    fn test_parse_cli_args_verify_with_checkout() {
        let parsed = parse_cli_args(vec![
            "verify".to_string(),
            "--checkout".to_string(),
            "../flexpart".to_string(),
        ])
        .expect("verify should parse");
        assert_eq!(
            parsed,
            CliCommand::Verify(VerifyOptions {
                checkout: PathBuf::from("../flexpart"),
                manifest: None,
            })
        );
    }

    #[test]
    fn test_parse_cli_args_verify_requires_checkout() {
        assert!(parse_cli_args(vec!["verify".to_string()]).is_err());
    }

    #[test]
    fn test_parse_cli_args_rejects_unknown_command() {
        assert!(parse_cli_args(vec!["build".to_string()]).is_err());
    }
}
