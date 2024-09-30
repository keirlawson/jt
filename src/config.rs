use std::fs;

use anyhow::Result;
use reqwest::Url;
use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
pub struct Config {
    #[serde(deserialize_with = "deserialize_url")]
    pub api_endpoint: Url,
    pub worker: String,
    pub reviewer: Option<String>,
    pub default_time_spent_seconds: Option<u64>,
    #[serde(default)]
    pub static_tasks: Vec<String>,
    #[serde(default)]
    pub static_attributes: Vec<WorkAttribute>,
    #[serde(default)]
    pub dynamic_attributes: Vec<WorkAttribute>,
}

#[derive(Deserialize, Clone)]
pub struct WorkAttribute {
    pub key: String,
    pub name: String,
    pub work_attribute_id: u64,
    pub value: String,
}

const CONFIG_FILE_NAME: &str = "jt.toml";

fn deserialize_url<'de, D>(deserializer: D) -> Result<Url, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = String::deserialize(deserializer)?;
    Url::parse(&buf).map_err(serde::de::Error::custom)
}

pub fn load_config() -> Result<Config> {
    let dir = dirs::config_dir().expect("Unable to determine configuration directory");
    let path = dir.join(CONFIG_FILE_NAME);
    let content = fs::read_to_string(path)?;
    toml::from_str(&content).map_err(|e| e.into())
}
