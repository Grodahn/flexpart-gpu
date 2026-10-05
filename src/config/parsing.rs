//! Shared file grammar, scalar conversion, timestamps and version policy.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::error::ConfigError;

pub(super) type ConfigMap = BTreeMap<String, String>;

pub(super) const CURRENT_CONFIG_VERSION: u32 = 1;
/// Unversioned FLEXPART and earlier project files retain their legacy parser semantics.
/// Project-owned versioned files currently support schema version 1 only.
pub(super) fn validate_config_version(
    version: Option<u32>,
    context: &str,
) -> Result<(), ConfigError> {
    match version {
        None | Some(CURRENT_CONFIG_VERSION) => Ok(()),
        Some(other) => Err(ConfigError::Validation {
            message: format!(
                "{context} has unsupported version {other}; supported version is {CURRENT_CONFIG_VERSION} (omit version only for legacy input)"
            ),
        }),
    }
}

pub(super) fn read_text_file(path: &Path) -> Result<String, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::MissingPath {
            path: path.to_path_buf(),
        });
    }
    fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.to_path_buf(),
        source,
    })
}

pub(super) fn extract_namelist_sections(
    input: &str,
    section_name: &str,
) -> Result<Vec<String>, String> {
    let lower = input.to_ascii_lowercase();
    let target = section_name.to_ascii_lowercase();
    let mut sections = Vec::new();
    let mut idx = 0;

    while let Some(found) = lower[idx..].find('&') {
        let start = idx + found;
        let mut name_end = start + 1;

        while let Some(ch) = lower.as_bytes().get(name_end).map(|byte| char::from(*byte)) {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                name_end += 1;
            } else {
                break;
            }
        }

        if name_end == start + 1 {
            idx = start + 1;
            continue;
        }

        let name = &lower[(start + 1)..name_end];
        if name != target {
            idx = name_end;
            continue;
        }

        let mut end = name_end;
        let mut in_single = false;
        let mut in_double = false;
        let mut found_terminator = false;

        while let Some(ch) = input.as_bytes().get(end).map(|byte| char::from(*byte)) {
            if ch == '\'' && !in_double {
                in_single = !in_single;
            } else if ch == '"' && !in_single {
                in_double = !in_double;
            } else if ch == '/' && !in_single && !in_double {
                sections.push(input[name_end..end].to_string());
                end += 1;
                found_terminator = true;
                break;
            }
            end += 1;
        }

        if !found_terminator {
            return Err(format!(
                "section `&{section_name}` is missing a terminating `/`"
            ));
        }

        idx = end;
    }
    Ok(sections)
}

pub(super) fn strip_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        let mut in_single = false;
        let mut in_double = false;
        for ch in line.chars() {
            if ch == '\'' && !in_double {
                in_single = !in_single;
            } else if ch == '"' && !in_single {
                in_double = !in_double;
            }
            if !in_single && !in_double && (ch == '!' || ch == '#') {
                break;
            }
            out.push(ch);
        }
        out.push('\n');
    }
    out
}

pub(super) fn parse_assignments(input: &str) -> Result<ConfigMap, String> {
    let cleaned = strip_comments(input);
    let mut map = BTreeMap::new();
    let mut chars = cleaned.chars().peekable();

    while chars.peek().is_some() {
        while let Some(ch) = chars.peek() {
            if ch.is_whitespace() || *ch == ',' {
                chars.next();
            } else {
                break;
            }
        }
        if chars.peek().is_none() {
            break;
        }

        let mut key = String::new();
        while let Some(ch) = chars.peek() {
            if *ch == '=' {
                break;
            }
            if *ch == ',' || *ch == '\n' || *ch == '\r' {
                return Err(format!("expected `=` after key `{}`", key.trim()));
            }
            key.push(*ch);
            chars.next();
        }
        if chars.next() != Some('=') {
            return Err(format!("missing `=` for key `{}`", key.trim()));
        }
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            return Err("empty key before `=`".to_string());
        }

        while let Some(ch) = chars.peek() {
            if ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }

        let mut value = String::new();
        let mut in_single = false;
        let mut in_double = false;
        while let Some(ch) = chars.peek() {
            let c = *ch;
            if c == '\'' && !in_double {
                in_single = !in_single;
                value.push(c);
                chars.next();
                continue;
            }
            if c == '"' && !in_single {
                in_double = !in_double;
                value.push(c);
                chars.next();
                continue;
            }
            if !in_single && !in_double && (c == ',' || c == '\n' || c == '\r') {
                break;
            }
            value.push(c);
            chars.next();
        }

        let value = trim_wrapping_quotes(value.trim());
        if value.is_empty() {
            return Err(format!("empty value for key `{key}`"));
        }
        map.insert(key, value.to_string());

        while let Some(ch) = chars.peek() {
            if *ch == ',' || *ch == '\n' || *ch == '\r' || ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
    }

    Ok(map)
}

fn trim_wrapping_quotes(input: &str) -> &str {
    if input.len() >= 2 {
        let starts_single = input.starts_with('\'') && input.ends_with('\'');
        let starts_double = input.starts_with('"') && input.ends_with('"');
        if starts_single || starts_double {
            return &input[1..(input.len() - 1)];
        }
    }
    input
}

pub(super) fn get_first<'a>(map: &'a ConfigMap, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| map.get(*key).map(String::as_str))
}

pub(super) fn parse_optional_string(map: &ConfigMap, keys: &[&str]) -> Option<String> {
    get_first(map, keys).map(str::trim).map(ToString::to_string)
}

pub(super) fn parse_required_f64(
    map: &ConfigMap,
    context: &str,
    keys: &[&str],
) -> Result<f64, ConfigError> {
    parse_optional_f64(map, context, keys)?.ok_or_else(|| ConfigError::MissingKey {
        context: context.to_string(),
        key: keys[0].to_string(),
    })
}

pub(super) fn parse_optional_f64(
    map: &ConfigMap,
    context: &str,
    keys: &[&str],
) -> Result<Option<f64>, ConfigError> {
    let Some(raw) = get_first(map, keys) else {
        return Ok(None);
    };
    raw.parse::<f64>()
        .map(Some)
        .map_err(|_| ConfigError::InvalidValue {
            context: context.to_string(),
            key: keys[0].to_string(),
            value: raw.to_string(),
            message: "expected floating-point number".to_string(),
        })
}

pub(super) fn parse_required_u32(
    map: &ConfigMap,
    context: &str,
    keys: &[&str],
) -> Result<u32, ConfigError> {
    parse_optional_u32(map, context, keys)?.ok_or_else(|| ConfigError::MissingKey {
        context: context.to_string(),
        key: keys[0].to_string(),
    })
}

pub(super) fn parse_optional_u32(
    map: &ConfigMap,
    context: &str,
    keys: &[&str],
) -> Result<Option<u32>, ConfigError> {
    let Some(raw) = get_first(map, keys) else {
        return Ok(None);
    };
    raw.parse::<u32>()
        .map(Some)
        .map_err(|_| ConfigError::InvalidValue {
            context: context.to_string(),
            key: keys[0].to_string(),
            value: raw.to_string(),
            message: "expected unsigned integer".to_string(),
        })
}

pub(super) fn parse_optional_u64(
    map: &ConfigMap,
    context: &str,
    keys: &[&str],
) -> Result<Option<u64>, ConfigError> {
    let Some(raw) = get_first(map, keys) else {
        return Ok(None);
    };
    raw.parse::<u64>()
        .map(Some)
        .map_err(|_| ConfigError::InvalidValue {
            context: context.to_string(),
            key: keys[0].to_string(),
            value: raw.to_string(),
            message: "expected unsigned integer".to_string(),
        })
}

pub(super) fn extract_timestamp(
    map: &ConfigMap,
    context: &str,
    direct_keys: &[&str],
    date_time_keys: Option<(&str, &str)>,
) -> Result<String, ConfigError> {
    if let Some(raw) = get_first(map, direct_keys) {
        return normalize_timestamp(raw).map_err(|message| ConfigError::InvalidValue {
            context: context.to_string(),
            key: direct_keys[0].to_string(),
            value: raw.to_string(),
            message,
        });
    }

    if let Some((date_key, time_key)) = date_time_keys {
        if let Some(date_raw) = map.get(date_key) {
            let time_raw = map.get(time_key).map_or("000000", String::as_str);
            let combined = format!("{date_raw}{time_raw}");
            return normalize_timestamp(&combined).map_err(|message| ConfigError::InvalidValue {
                context: context.to_string(),
                key: format!("{date_key}/{time_key}"),
                value: combined,
                message,
            });
        }
    }

    Err(ConfigError::MissingKey {
        context: context.to_string(),
        key: direct_keys[0].to_string(),
    })
}

fn normalize_timestamp(raw: &str) -> Result<String, String> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    let normalized = match digits.len() {
        8 => format!("{digits}000000"),
        10 => format!("{digits}0000"),
        12 => format!("{digits}00"),
        14 => digits,
        _ => {
            return Err(
                "expected a date/time with 8, 10, 12, or 14 digits (YYYYMMDD[HH[MM[SS]]])"
                    .to_string(),
            );
        }
    };

    let year = normalized[0..4]
        .parse::<u32>()
        .map_err(|_| "invalid year in timestamp".to_string())?;
    let month = normalized[4..6]
        .parse::<u32>()
        .map_err(|_| "invalid month in timestamp".to_string())?;
    let day = normalized[6..8]
        .parse::<u32>()
        .map_err(|_| "invalid day in timestamp".to_string())?;
    let hour = normalized[8..10]
        .parse::<u32>()
        .map_err(|_| "invalid hour in timestamp".to_string())?;
    let minute = normalized[10..12]
        .parse::<u32>()
        .map_err(|_| "invalid minute in timestamp".to_string())?;
    let second = normalized[12..14]
        .parse::<u32>()
        .map_err(|_| "invalid second in timestamp".to_string())?;

    if year == 0 {
        return Err("year must be > 0".to_string());
    }
    if !(1..=12).contains(&month) {
        return Err("month must be in [1, 12]".to_string());
    }
    if !(1..=31).contains(&day) {
        return Err("day must be in [1, 31]".to_string());
    }
    if hour > 23 {
        return Err("hour must be in [0, 23]".to_string());
    }
    if minute > 59 {
        return Err("minute must be in [0, 59]".to_string());
    }
    if second > 59 {
        return Err("second must be in [0, 59]".to_string());
    }

    Ok(normalized)
}
