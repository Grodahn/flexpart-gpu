//! COMMAND run timing, file aliases and interval validation.

use serde::{Deserialize, Serialize};
use std::path::Path;

use super::error::ConfigError;
use super::parsing::{
    extract_namelist_sections, extract_timestamp, parse_assignments, parse_optional_u64,
    read_text_file, ConfigMap,
};

/// Run chronology and optional output/synchronization intervals in seconds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandConfig {
    pub start_time: String,
    pub end_time: String,
    pub output_interval_seconds: Option<u64>,
    pub sync_interval_seconds: Option<u64>,
    pub raw: ConfigMap,
}

impl CommandConfig {
    /// Read COMMAND timing from a required input file.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = read_text_file(path)?;
        Self::parse(&content, path)
    }

    /// Parse the supported COMMAND namelist without supplying interval defaults.
    pub fn parse(input: &str, source: &Path) -> Result<Self, ConfigError> {
        let context = format!("COMMAND ({})", source.display());
        let sections =
            extract_namelist_sections(input, "COMMAND").map_err(|message| ConfigError::Parse {
                context: context.clone(),
                message,
            })?;
        let section = sections.first().ok_or_else(|| ConfigError::Parse {
            context: context.clone(),
            message: "expected `&COMMAND ... /` section".to_string(),
        })?;
        let raw = parse_assignments(section).map_err(|message| ConfigError::Parse {
            context: context.clone(),
            message,
        })?;
        Self::from_map(raw, &context)
    }

    fn from_map(raw: ConfigMap, context: &str) -> Result<Self, ConfigError> {
        let start_time = extract_timestamp(
            &raw,
            context,
            &["start_time", "start", "ibdatetime"],
            Some(("ibdate", "ibtime")),
        )?;
        let end_time = extract_timestamp(
            &raw,
            context,
            &["end_time", "end", "iedatetime"],
            Some(("iedate", "ietime")),
        )?;
        let output_interval_seconds = parse_optional_u64(
            &raw,
            context,
            &["loutstep", "outstep", "output_interval_seconds"],
        )?;
        let sync_interval_seconds =
            parse_optional_u64(&raw, context, &["lsynctime", "sync_interval_seconds"])?;

        let config = Self {
            start_time,
            end_time,
            output_interval_seconds,
            sync_interval_seconds,
            raw,
        };
        config.validate()?;
        Ok(config)
    }

    /// Reject reversed chronology and zero output/synchronization intervals.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.start_time > self.end_time {
            return Err(ConfigError::Validation {
                message: format!(
                    "COMMAND start_time `{}` is after end_time `{}`",
                    self.start_time, self.end_time
                ),
            });
        }
        if self.output_interval_seconds == Some(0) {
            return Err(ConfigError::Validation {
                message: "COMMAND output interval must be > 0".to_string(),
            });
        }
        if self.sync_interval_seconds == Some(0) {
            return Err(ConfigError::Validation {
                message: "COMMAND sync interval must be > 0".to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_command_namelist() {
        let input = r#"
            &COMMAND
              IBDATE = 20240101,
              IBTIME = 120000,
              IEDATE = 20240101,
              IETIME = 180000,
              LOUTSTEP = 3600,
            /
        "#;
        let command = CommandConfig::parse(input, Path::new("<inline>")).expect("parse command");
        assert_eq!(command.start_time, "20240101120000");
        assert_eq!(command.end_time, "20240101180000");
        assert_eq!(command.output_interval_seconds, Some(3600));
    }
}
