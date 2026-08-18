use anyhow::{Context, Result, bail};
use flowwatch_core::ClashConfig;
use std::path::Path;

pub fn read_clash_config(path: &Path) -> Result<ClashConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read Clash config {}", path.display()))?;
    let mut controller = String::new();
    let mut secret = String::new();
    for line in raw.lines() {
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key {
            "external-controller" => controller = parse_yaml_scalar(value),
            "secret" => secret = parse_yaml_scalar(value),
            _ => {}
        }
    }
    if controller.is_empty() {
        bail!("external-controller is missing from {}", path.display());
    }
    Ok(ClashConfig {
        enabled: true,
        controller,
        secret,
    })
}

fn parse_yaml_scalar(raw: &str) -> String {
    let value = strip_yaml_comment(raw.trim());
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return serde_json::from_str::<String>(value)
            .unwrap_or_else(|_| value[1..value.len() - 1].to_string());
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("''", "'");
    }
    value.to_string()
}

fn strip_yaml_comment(value: &str) -> &str {
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;
    let mut previous_whitespace = true;
    for (index, character) in value.char_indices() {
        if double_quoted && character == '\\' && !escaped {
            escaped = true;
            previous_whitespace = false;
            continue;
        }
        if !escaped {
            if character == '\'' && !double_quoted {
                single_quoted = !single_quoted;
            } else if character == '"' && !single_quoted {
                double_quoted = !double_quoted;
            } else if character == '#' && !single_quoted && !double_quoted && previous_whitespace {
                return value[..index].trim_end();
            }
        }
        previous_whitespace = character.is_whitespace();
        escaped = false;
    }
    value.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_scalar_preserves_numeric_looking_secret() {
        assert_eq!(parse_yaml_scalar(" 0007"), "0007");
        assert_eq!(parse_yaml_scalar(" '0007'"), "0007");
        assert_eq!(parse_yaml_scalar(" '0007' # comment"), "0007");
        assert_eq!(parse_yaml_scalar(" value # comment"), "value");
        assert_eq!(parse_yaml_scalar(" 'value # literal'"), "value # literal");
    }
}
