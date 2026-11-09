use serde::Deserialize;
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
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "snake_case")]
struct RawConfig {
    provider: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    default_model: Option<String>,
    team_root: Option<String>,
    code_root: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
}

fn defaults() -> Config {
    Config {
        provider: "openai".into(),
        api_key: String::new(),
        base_url: "https://api.openai.com/v1".into(),
        default_model: "gpt-4o".into(),
        team_root: ".".into(),
        code_root: None,
        max_tokens: 8192,
        temperature: 0.7,
    }
}

fn load_json(file: &Path) -> RawConfig {
    match std::fs::read_to_string(file) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => RawConfig::default(),
    }
}

fn merge_into(base: &mut Config, raw: RawConfig) {
    if let Some(v) = raw.provider { base.provider = v; }
    if let Some(v) = raw.api_key { base.api_key = v; }
    if let Some(v) = raw.base_url { base.base_url = v; }
    if let Some(v) = raw.default_model { base.default_model = v; }
    if let Some(v) = raw.team_root { base.team_root = v; }
    if let Some(v) = raw.code_root {
        base.code_root = if v.is_empty() { None } else { Some(v) };
    }
    if let Some(v) = raw.max_tokens { base.max_tokens = v; }
    if let Some(v) = raw.temperature { base.temperature = v; }
}

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("~"))
}

pub fn load_config() -> Config {
    let mut cfg = defaults();

    let global = home().join(".opc").join("opc.json");
    merge_into(&mut cfg, load_json(&global));

    let hidden = std::env::current_dir()
        .unwrap_or_default()
        .join(".opc_agent")
        .join("opc.json");
    merge_into(&mut cfg, load_json(&hidden));

    let local = std::env::current_dir()
        .unwrap_or_default()
        .join("opc.json");
    merge_into(&mut cfg, load_json(&local));

    // Environment overrides (highest priority)
    let env_map: [(&str, &str); 8] = [
        ("OPC_API_KEY", "api_key"),
        ("OPC_BASE_URL", "base_url"),
        ("OPC_MODEL", "default_model"),
        ("OPC_PROVIDER", "provider"),
        ("OPC_TEAM_ROOT", "team_root"),
        ("OPC_CODE_ROOT", "code_root"),
        ("OPC_MAX_TOKENS", "max_tokens"),
        ("OPC_TEMPERATURE", "temperature"),
    ];
    for (k, field) in env_map {
        if let Ok(v) = std::env::var(k) {
            if v.is_empty() { continue; }
            match field {
                "api_key" => cfg.api_key = v,
                "base_url" => cfg.base_url = v,
                "default_model" => cfg.default_model = v,
                "provider" => cfg.provider = v,
                "team_root" => cfg.team_root = v,
                "code_root" => cfg.code_root = Some(v),
                "max_tokens" => cfg.max_tokens = v.parse().unwrap_or(cfg.max_tokens),
                "temperature" => cfg.temperature = v.parse().unwrap_or(cfg.temperature),
                _ => {}
            }
        }
    }
    cfg
}

pub fn team_root_path(cfg: &Config) -> PathBuf {
    let p = PathBuf::from(&cfg.team_root);
    if p.is_absolute() { p } else { std::env::current_dir().unwrap_or_default().join(p) }
}

pub fn validate_config(cfg: &Config) -> Vec<String> {
    let mut errors = Vec::new();
    if cfg.api_key.is_empty() {
        errors.push("api_key is required".into());
    }
    if cfg.provider != "openai" && cfg.provider != "anthropic" {
        errors.push(format!("unknown provider: {}", cfg.provider));
    }
    if !team_root_path(cfg).exists() {
        errors.push(format!("team_root not found: {}", cfg.team_root));
    }
    errors
}
