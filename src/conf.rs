use serde::Deserialize;
use std::{fs, io};

use crate::paths::get_config_path;

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Conf {
    pub(crate) secret: String,
    pub(crate) hostname: String,
    pub(crate) provider: String,
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
        serde_json::from_str(&data).expect("Invalid content for config.json")
    }

    pub(crate) fn api_hostname(&self) -> String {
        // TODO: compute this in read() and add it as an additional field
        format!("--api--.{}", self.hostname)
    }

    pub(crate) fn wildcard_domain(&self) -> String {
        format!("*.{}", self.hostname)
    }
}
