//! Pinned FLEXPART oracle reference (RISK-03.3G-01).
//!
//! This module is the single source of truth for the normative FLEXPART
//! revision used in all validation comparisons. The pinned revision lives in
//! `reference/flexpart-11.1.json` and is embedded into the library, so every
//! consumer (binaries, scripts, tests) reads the same commit hash instead of
//! hard-coding its own copy.
//!
//! Verification is fail-closed: a reference checkout must be an unmodified
//! upstream tree at exactly the pinned commit. A different commit, a dirty
//! work tree, or a missing `git` binary all produce an error, never a
//! silent fallback.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Relative path of the bundled oracle manifest from the crate root.
pub const BUNDLED_MANIFEST_RELATIVE_PATH: &str = "reference/flexpart-11.1.json";

/// Machine-readable description of the normative FLEXPART oracle revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceManifest {
    /// Model name (expected: `FLEXPART`).
    pub name: String,
    /// Model version (expected: `11.1`).
    pub version: String,
    /// Human-readable role of the oracle.
    pub role: String,
    /// Upstream repository URL (read-only; never modified by this project).
    pub upstream_repository: String,
    /// Pinned upstream tag.
    pub pinned_tag: String,
    /// Pinned upstream commit hash (full 40-hex SHA-1).
    pub pinned_commit: String,
    /// License identifier of the upstream sources.
    pub license: String,
    /// URL of the upstream license text.
    pub license_url: String,
    /// Requirement statement for reference checkouts.
    pub source_requirement: String,
    /// Additional notes (banner quirks, usage rules).
    #[serde(default)]
    pub notes: Vec<String>,
}

impl ReferenceManifest {
    /// Parse a manifest from JSON text.
    ///
    /// # Errors
    ///
    /// Returns [`ReferenceError::ParseManifest`] for invalid JSON and
    /// [`ReferenceError::InvalidManifest`] when required fields are empty or
    /// the commit hash is malformed.
    pub fn parse(json: &str) -> Result<Self, ReferenceError> {
        let manifest: Self = serde_json::from_str(json)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Load a manifest from a JSON file.
    ///
    /// # Errors
    ///
    /// Returns [`ReferenceError::ReadManifest`] when the file cannot be read,
    /// plus the errors of [`ReferenceManifest::parse`].
    pub fn load(path: &Path) -> Result<Self, ReferenceError> {
        let content =
            std::fs::read_to_string(path).map_err(|source| ReferenceError::ReadManifest {
                path: path.to_path_buf(),
                reason: source.to_string(),
            })?;
        Self::parse(&content)
    }

    /// Load the manifest bundled with this crate (`reference/flexpart-11.1.json`).
    ///
    /// # Errors
    ///
    /// Returns [`ReferenceError`] when the bundled manifest is missing or invalid.
    /// A failure here indicates a broken checkout of this repository itself.
    pub fn bundled() -> Result<Self, ReferenceError> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BUNDLED_MANIFEST_RELATIVE_PATH);
        Self::load(&path)
    }

    /// Check structural validity of manifest fields.
    ///
    /// # Errors
    ///
    /// Returns [`ReferenceError::InvalidManifest`] when a required field is
    /// empty or the pinned commit is not a 40-character hex string.
    pub fn validate(&self) -> Result<(), ReferenceError> {
        for (field, value) in [
            ("name", self.name.as_str()),
            ("version", self.version.as_str()),
            ("upstream_repository", self.upstream_repository.as_str()),
            ("pinned_tag", self.pinned_tag.as_str()),
            ("pinned_commit", self.pinned_commit.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(ReferenceError::InvalidManifest {
                    message: format!("manifest field `{field}` must not be empty"),
                });
            }
        }
        let commit = self.pinned_commit.trim();
        if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ReferenceError::InvalidManifest {
                message: "pinned_commit must be a full 40-character hex SHA-1".to_string(),
            });
        }
        Ok(())
    }
}

/// Errors for oracle manifest handling and checkout verification.
#[derive(Debug, Error)]
pub enum ReferenceError {
    #[error("failed to read manifest `{path}`: {reason}")]
    ReadManifest { path: PathBuf, reason: String },
    #[error("failed to parse manifest JSON: {0}")]
    ParseManifest(#[from] serde_json::Error),
    #[error("invalid manifest: {message}")]
    InvalidManifest { message: String },
    #[error("reference checkout `{path}` is not a git work tree: {reason}")]
    NotAGitCheckout { path: PathBuf, reason: String },
    #[error("reference checkout `{path}` is at commit {actual}, expected pinned {expected}")]
    CommitMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("reference checkout `{path}` has uncommitted changes and is not an unmodified oracle:\n{status}")]
    DirtyWorkTree { path: PathBuf, status: String },
    #[error("failed to run `git` for checkout `{path}`: {reason}")]
    GitFailed { path: PathBuf, reason: String },
}

/// Outcome of a successful checkout verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCheckout {
    /// Verified checkout directory.
    pub path: PathBuf,
    /// Pinned commit the checkout was verified against.
    pub commit: String,
}

/// Compare an observed checkout commit against the pinned commit (pure helper).
///
/// Comparison is exact and case-insensitive on surrounding whitespace only;
/// abbreviated hashes never match a full pinned hash.
///
/// # Errors
///
/// Returns [`ReferenceError::CommitMismatch`] when the hashes differ.
pub fn check_pinned_commit(
    checkout: &Path,
    expected: &str,
    actual: &str,
) -> Result<(), ReferenceError> {
    if actual.trim() == expected.trim() {
        Ok(())
    } else {
        Err(ReferenceError::CommitMismatch {
            path: checkout.to_path_buf(),
            expected: expected.to_string(),
            actual: actual.to_string(),
        })
    }
}

/// Verify that `checkout` is an unmodified upstream tree at the pinned commit.
///
/// Runs `git rev-parse HEAD` and `git status --porcelain` inside `checkout`.
/// Any deviation fails; there is no silent fallback.
///
/// # Errors
///
/// Returns [`ReferenceError`] when `git` is unavailable, the directory is not
/// a git checkout, the commit differs, or the work tree is dirty.
pub fn verify_checkout(
    manifest: &ReferenceManifest,
    checkout: &Path,
) -> Result<VerifiedCheckout, ReferenceError> {
    let head = run_git(checkout, &["rev-parse", "HEAD"])?;
    let actual = head.trim().to_string();
    if actual.is_empty() {
        return Err(ReferenceError::NotAGitCheckout {
            path: checkout.to_path_buf(),
            reason: "`git rev-parse HEAD` produced no output".to_string(),
        });
    }
    check_pinned_commit(checkout, &manifest.pinned_commit, &actual)?;

    let status = run_git(checkout, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err(ReferenceError::DirtyWorkTree {
            path: checkout.to_path_buf(),
            status: status.trim().to_string(),
        });
    }

    Ok(VerifiedCheckout {
        path: checkout.to_path_buf(),
        commit: manifest.pinned_commit.clone(),
    })
}

fn run_git(checkout: &Path, args: &[&str]) -> Result<String, ReferenceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .output()
        .map_err(|source| ReferenceError::GitFailed {
            path: checkout.to_path_buf(),
            reason: format!("cannot execute `git`: {source}"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ReferenceError::NotAGitCheckout {
            path: checkout.to_path_buf(),
            reason: if stderr.is_empty() {
                format!("git command failed: {}", args.join(" "))
            } else {
                stderr
            },
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED_COMMIT: &str = "c70586c2b7f5258850705325881c61f557ea9bd8";

    fn sample_manifest_json() -> String {
        format!(
            r#"{{
  "name": "FLEXPART",
  "version": "11.1",
  "role": "oracle",
  "upstream_repository": "https://gitlab.phaidra.org/flexpart/flexpart",
  "pinned_tag": "v11.1",
  "pinned_commit": "{PINNED_COMMIT}",
  "license": "GPL-3.0-or-later",
  "license_url": "https://example.invalid/LICENSE",
  "source_requirement": "unmodified",
  "notes": []
}}"#
        )
    }

    #[test]
    fn test_bundled_manifest_pins_flexpart_11_1_oracle() {
        let manifest = ReferenceManifest::bundled().expect("bundled manifest loads");
        assert_eq!(manifest.name, "FLEXPART");
        assert_eq!(manifest.version, "11.1");
        assert_eq!(manifest.pinned_tag, "v11.1");
        assert_eq!(manifest.pinned_commit, PINNED_COMMIT);
    }

    #[test]
    fn test_manifest_parse_accepts_valid_document() {
        let manifest = ReferenceManifest::parse(&sample_manifest_json()).expect("valid manifest");
        assert_eq!(manifest.pinned_commit, PINNED_COMMIT);
    }

    #[test]
    fn test_manifest_parse_rejects_malformed_json() {
        let error = ReferenceManifest::parse("{not json").expect_err("must reject");
        assert!(matches!(error, ReferenceError::ParseManifest(_)));
    }

    #[test]
    fn test_manifest_validate_rejects_short_commit() {
        let mut manifest =
            ReferenceManifest::parse(&sample_manifest_json()).expect("valid manifest");
        manifest.pinned_commit = "c70586c".to_string();
        let error = manifest.validate().expect_err("must reject short hash");
        assert!(matches!(error, ReferenceError::InvalidManifest { .. }));
    }

    #[test]
    fn test_manifest_validate_rejects_empty_tag() {
        let mut manifest =
            ReferenceManifest::parse(&sample_manifest_json()).expect("valid manifest");
        manifest.pinned_tag = "  ".to_string();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_check_pinned_commit_accepts_exact_match() {
        let checkout = Path::new("/tmp/oracle");
        check_pinned_commit(checkout, PINNED_COMMIT, &format!("{PINNED_COMMIT}\n"))
            .expect("exact match with trailing newline");
    }

    #[test]
    fn test_check_pinned_commit_rejects_mismatch() {
        let checkout = Path::new("/tmp/oracle");
        let error = check_pinned_commit(
            checkout,
            PINNED_COMMIT,
            "0000000000000000000000000000000000000000",
        )
        .expect_err("must reject");
        let rendered = error.to_string();
        assert!(rendered.contains(PINNED_COMMIT), "unexpected: {rendered}");
    }

    #[test]
    fn test_verify_checkout_rejects_missing_directory() {
        let manifest = ReferenceManifest::bundled().expect("bundled manifest loads");
        let missing = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("definitely-not-a-checkout-12345");
        if missing.exists() {
            return;
        }
        let error = verify_checkout(&manifest, &missing).expect_err("must reject");
        let rendered = error.to_string();
        assert!(
            rendered.contains("not a git work tree") || rendered.contains("cannot execute"),
            "unexpected: {rendered}"
        );
    }
}
