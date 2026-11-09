use serde::Deserialize;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Config {
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub default_model: String,
    pub team_root: String,
    #[serde(default)]
    pub code_root: Option<String>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub llm_connect_timeout_seconds: u64,
    pub llm_request_timeout_seconds: u64,
    pub llm_max_attempts: u32,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct RawConfig {
    provider: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    default_model: Option<String>,
    team_root: Option<String>,
    code_root: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    llm_connect_timeout_seconds: Option<u64>,
    llm_request_timeout_seconds: Option<u64>,
    llm_max_attempts: Option<u32>,
}

fn defaults() -> Config {
    Config {
        provider: "openai".into(),
        api_key: String::new(),
        base_url: "https://api.openai.com/v1".into(),
        default_model: "gpt-4o".into(),
        team_root: ".myopc_agent".into(),
        code_root: Some(".".into()),
        max_tokens: 8192,
        temperature: 0.7,
        llm_connect_timeout_seconds: 10,
        llm_request_timeout_seconds: 180,
        llm_max_attempts: 3,
    }
}

fn load_json(file: &Path) -> Result<Option<RawConfig>, String> {
    let source = match std::fs::read_to_string(file) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot read config {}: {error}", file.display())),
    };
    let raw: RawConfig = serde_json::from_str(&source)
        .map_err(|error| format!("Invalid config {}: {error}", file.display()))?;
    if raw.api_key.as_deref().is_some_and(|key| !key.is_empty()) {
        let file_mode = std::fs::metadata(file)
            .map_err(|error| format!("Cannot inspect config {}: {error}", file.display()))?
            .permissions()
            .mode()
            & 0o777;
        if file_mode != 0o600 {
            return Err(format!(
                "Config {} contains an API key and must have mode 0600 (current {file_mode:04o})",
                file.display()
            ));
        }
        let parent = file.parent().unwrap_or_else(|| Path::new("."));
        let directory_mode = std::fs::metadata(parent)
            .map_err(|error| {
                format!(
                    "Cannot inspect config directory {}: {error}",
                    parent.display()
                )
            })?
            .permissions()
            .mode()
            & 0o777;
        if directory_mode != 0o700 {
            return Err(format!(
                "Config directory {} contains an API key file and must have mode 0700 (current {directory_mode:04o})",
                parent.display()
            ));
        }
    }
    Ok(Some(raw))
}

fn merge_into(base: &mut Config, raw: RawConfig) {
    if let Some(value) = raw.provider {
        base.provider = value;
    }
    if let Some(value) = raw.api_key {
        base.api_key = value;
    }
    if let Some(value) = raw.base_url {
        base.base_url = value;
    }
    if let Some(value) = raw.default_model {
        base.default_model = value;
    }
    if let Some(value) = raw.team_root {
        base.team_root = value;
    }
    if let Some(value) = raw.code_root {
        base.code_root = Some(value);
    }
    if let Some(value) = raw.max_tokens {
        base.max_tokens = value;
    }
    if let Some(value) = raw.temperature {
        base.temperature = value;
    }
    if let Some(value) = raw.llm_connect_timeout_seconds {
        base.llm_connect_timeout_seconds = value;
    }
    if let Some(value) = raw.llm_request_timeout_seconds {
        base.llm_request_timeout_seconds = value;
    }
    if let Some(value) = raw.llm_max_attempts {
        base.llm_max_attempts = value;
    }
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("~"))
}

fn apply_env_override(config: &mut Config, name: &str, value: String) -> Result<(), String> {
    match name {
        "MYOPC_API_KEY" => config.api_key = value,
        "MYOPC_BASE_URL" => config.base_url = value,
        "MYOPC_MODEL" => config.default_model = value,
        "MYOPC_PROVIDER" => config.provider = value,
        "MYOPC_TEAM_ROOT" => config.team_root = value,
        "MYOPC_CODE_ROOT" => config.code_root = Some(value),
        "MYOPC_MAX_TOKENS" => {
            config.max_tokens = value
                .parse()
                .map_err(|_| format!("{name} must be a positive integer, got {value:?}"))?;
        }
        "MYOPC_TEMPERATURE" => {
            config.temperature = value
                .parse()
                .map_err(|_| format!("{name} must be a number from 0 to 2, got {value:?}"))?;
        }
        "MYOPC_LLM_CONNECT_TIMEOUT_SECONDS" => {
            config.llm_connect_timeout_seconds = value
                .parse()
                .map_err(|_| format!("{name} must be a positive integer, got {value:?}"))?;
        }
        "MYOPC_LLM_REQUEST_TIMEOUT_SECONDS" => {
            config.llm_request_timeout_seconds = value
                .parse()
                .map_err(|_| format!("{name} must be a positive integer, got {value:?}"))?;
        }
        "MYOPC_LLM_MAX_ATTEMPTS" => {
            config.llm_max_attempts = value
                .parse()
                .map_err(|_| format!("{name} must be an integer from 1 to 10, got {value:?}"))?;
        }
        _ => return Ok(()),
    }
    Ok(())
}

fn validate_values(config: &Config) -> Vec<String> {
    let mut errors = Vec::new();
    if config.provider != "openai" && config.provider != "anthropic" {
        errors.push(format!("unknown provider: {}", config.provider));
    }
    let valid_url = reqwest::Url::parse(&config.base_url)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some());
    if !valid_url {
        errors.push("base_url must be an absolute HTTP(S) URL".into());
    }
    if config.default_model.trim().is_empty() {
        errors.push("default_model cannot be empty".into());
    }
    if config.team_root.trim().is_empty() {
        errors.push("team_root cannot be empty".into());
    }
    if config
        .code_root
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        errors.push("code_root cannot be empty".into());
    }
    if config.max_tokens == 0 {
        errors.push("max_tokens must be greater than 0".into());
    }
    if !config.temperature.is_finite() || !(0.0..=2.0).contains(&config.temperature) {
        errors.push("temperature must be from 0 to 2".into());
    }
    if config.llm_connect_timeout_seconds == 0 {
        errors.push("llm_connect_timeout_seconds must be greater than 0".into());
    }
    if config.llm_request_timeout_seconds == 0 {
        errors.push("llm_request_timeout_seconds must be greater than 0".into());
    }
    if !(1..=10).contains(&config.llm_max_attempts) {
        errors.push("llm_max_attempts must be from 1 to 10".into());
    }
    errors
}

pub fn load_config() -> Result<Config, Vec<String>> {
    let mut config = defaults();
    let mut errors = Vec::new();
    let current_dir = std::env::current_dir().unwrap_or_default();
    let files = [
        home().join(".myopc").join("myopc.json"),
        current_dir.join(".myopc_agent").join("myopc.json"),
        current_dir.join("myopc.json"),
    ];

    for file in files {
        match load_json(&file) {
            Ok(Some(raw)) => merge_into(&mut config, raw),
            Ok(None) => {}
            Err(error) => errors.push(error),
        }
    }

    const ENV_NAMES: [&str; 11] = [
        "MYOPC_API_KEY",
        "MYOPC_BASE_URL",
        "MYOPC_MODEL",
        "MYOPC_PROVIDER",
        "MYOPC_TEAM_ROOT",
        "MYOPC_CODE_ROOT",
        "MYOPC_MAX_TOKENS",
        "MYOPC_TEMPERATURE",
        "MYOPC_LLM_CONNECT_TIMEOUT_SECONDS",
        "MYOPC_LLM_REQUEST_TIMEOUT_SECONDS",
        "MYOPC_LLM_MAX_ATTEMPTS",
    ];
    for name in ENV_NAMES {
        if let Ok(value) = std::env::var(name) {
            if value.is_empty() {
                continue;
            }
            if let Err(error) = apply_env_override(&mut config, name, value) {
                errors.push(error);
            }
        }
    }

    errors.extend(validate_values(&config));
    if errors.is_empty() {
        Ok(config)
    } else {
        Err(errors)
    }
}

pub fn team_root_path(config: &Config) -> PathBuf {
    let path = PathBuf::from(&config.team_root);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

impl Config {
    pub fn redactions(&self) -> Vec<String> {
        if self.api_key.is_empty() {
            Vec::new()
        } else {
            vec![self.api_key.clone()]
        }
    }

    pub fn redact(&self, text: &str) -> String {
        crate::redaction::redact_text(text, &self.redactions())
    }
}

pub fn validate_config(config: &Config) -> Vec<String> {
    let mut errors = validate_values(config);
    if config.api_key.is_empty() {
        errors.push("api_key is required".into());
    }
    if !team_root_path(config).exists() {
        errors.push(format!("team_root not found: {}", config.team_root));
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_initialized_project_layout() {
        let config = defaults();
        assert_eq!(config.team_root, ".myopc_agent");
        assert_eq!(config.code_root.as_deref(), Some("."));
        assert_eq!(config.llm_connect_timeout_seconds, 10);
        assert_eq!(config.llm_request_timeout_seconds, 180);
        assert_eq!(config.llm_max_attempts, 3);
    }

    #[test]
    fn malformed_json_reports_the_config_path() {
        let dir = std::env::temp_dir().join(format!(
            "myopc-config-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("myopc.json");
        std::fs::write(&file, "{not-json").unwrap();

        let error = load_json(&file).err().unwrap();
        assert!(error.contains(&file.display().to_string()));
        assert!(error.contains("Invalid config"));

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn wrong_field_type_is_an_error() {
        let dir = std::env::temp_dir().join(format!(
            "myopc-config-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("myopc.json");
        std::fs::write(&file, r#"{"max_tokens":"many"}"#).unwrap();

        let error = load_json(&file).err().unwrap();
        assert!(error.contains("expected u32"), "error: {error}");

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut config = defaults();
        config.temperature = 3.0;
        config.max_tokens = 0;
        config.base_url = "not-a-url".into();
        config.llm_max_attempts = 0;

        let errors = validate_values(&config);
        assert!(errors.iter().any(|error| error.contains("temperature")));
        assert!(errors.iter().any(|error| error.contains("max_tokens")));
        assert!(errors.iter().any(|error| error.contains("base_url")));
        assert!(errors
            .iter()
            .any(|error| error.contains("llm_max_attempts")));
    }

    #[test]
    fn invalid_environment_number_is_an_error() {
        let mut config = defaults();
        let error =
            apply_env_override(&mut config, "MYOPC_MAX_TOKENS", "not-a-number".into()).unwrap_err();
        assert!(error.contains("MYOPC_MAX_TOKENS"));
    }

    #[test]
    fn config_with_api_key_requires_private_permissions() {
        let dir = std::env::temp_dir().join(format!(
            "myopc-config-permissions-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("myopc.json");
        std::fs::write(&file, r#"{"api_key":"secret"}"#).unwrap();
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o644))
            .unwrap();

        assert!(load_json(&file).err().unwrap().contains("0600"));
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        assert!(load_json(&file).err().unwrap().contains("0700"));
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        assert!(load_json(&file).is_ok());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn configured_api_key_is_redacted() {
        let mut config = defaults();
        config.api_key = "secret-value".into();
        assert_eq!(
            config.redact("before secret-value after"),
            "before *** after"
        );
    }
}
