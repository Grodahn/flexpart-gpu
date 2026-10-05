//! Temporary file support for configuration domain tests only.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
pub(super) fn temp_dir(prefix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    path.push(format!(
        "flexpart_gpu_{prefix}_{nanos}_{}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("create temp directory");
    path
}
