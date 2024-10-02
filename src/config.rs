use std::{fs, path::PathBuf};

use anyhow::Result;
use reqwest::Url;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Serialize, Deserialize)]
pub struct Config {
    #[serde(deserialize_with = "deserialize_url", serialize_with = "serialize_url")]
    pub api_endpoint: Url,
    pub worker: String,
    pub reviewer: Option<String>,
    pub daily_target_time_spent_seconds: Option<u64>,
    pub default_time_spent_seconds: Option<u64>,
    #[serde(default, skip_serializing)]
    pub static_tasks: Vec<String>,
    #[serde(default, skip_serializing)]
    pub static_attributes: Vec<WorkAttribute>,
    #[serde(default, skip_serializing)]
    pub dynamic_attributes: Vec<WorkAttribute>,
}

#[derive(Serialize, Deserialize, Clone)]
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

fn serialize_url<S>(url: &Url, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(url.as_str())
}

pub fn config_file_location() -> PathBuf {
    let dir = dirs::config_dir().expect("Unable to determine configuration directory");
    dir.join(CONFIG_FILE_NAME)
}

pub fn load_config() -> Result<Config> {
    let content = fs::read_to_string(config_file_location())?;
    toml::from_str(&content).map_err(|e| e.into())
}

pub fn write_config(config: Config) -> Result<()> {
    let contents = toml::to_string_pretty(&config)?;
    fs::write(config_file_location(), contents).map_err(|e| e.into())
}
