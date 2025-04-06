use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use std::{fs, io};

use crate::paths::get_config_path;

#[derive(Clone, Debug)]
pub(crate) struct Conf {
    pub(crate) hostname: String,
    pub(crate) provider: String,
    pub(crate) encoded_secret: String,
    pub(crate) secret: Vec<u8>,
}

#[derive(Deserialize)]
struct StoredConf {
    pub(crate) hostname: String,
    pub(crate) provider: String,
    pub(crate) secret: String,
}

impl Conf {
    pub(crate) fn read() -> Self {
        let conf_data = fs::read_to_string(get_config_path());
        Self::from_string(conf_data)
    }

    pub(crate) async fn read_async() -> Self {
        let conf_data = tokio::fs::read_to_string(get_config_path()).await;
        Self::from_string(conf_data)
    }

    fn from_string(data: io::Result<String>) -> Self {
        let data = data.expect("Unable to find config.json");
        let stored: StoredConf =
            serde_json::from_str(&data).expect("Invalid content for config.json");
        Self {
            hostname: stored.hostname,
            provider: stored.provider,
            encoded_secret: stored.secret.clone(),
            secret: STANDARD
                .decode(stored.secret)
                .expect("invalid base64 encoding for secret"),
        }
    }

    pub(crate) fn api_hostname(&self) -> String {
        // TODO: compute this in read() and add it as an additional field
        format!("--api--.{}", self.hostname)
    }

    pub(crate) fn wildcard_domain(&self) -> String {
        format!("*.{}", self.hostname)
    }
}
