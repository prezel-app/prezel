use std::path::{Component, PathBuf};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};

use crate::Github;

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Visibility {
    Standard,
    Public,
    Private,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
enum BuildBackend {
    Dockerfile,
    Nixpacks,
}

// TODO: move this to the db mod maybe???

#[derive(Deserialize, Serialize, Debug, PartialEq, Clone)]
#[serde(tag = "backend", content = "config", rename_all = "lowercase")]
pub(crate) enum Build {
    Dockerfile { path: Option<String> },
    Nixpacks { provider: Option<String> },
}

// TODO: move this somwhere else
#[derive(Deserialize, Serialize, Default, Debug, PartialEq, Clone)]
pub(crate) struct DeploymentConfig {
    pub(crate) visibility: Option<Visibility>,
    pub(crate) build: Option<Build>,
}

#[derive(Debug, Clone)]
pub(crate) struct FlatDeploymentConfig {
    pub(crate) visibility: Option<String>,
    pub(crate) backend: Option<String>,
    pub(crate) dockerfile_path: Option<String>,
}

impl From<DeploymentConfig> for FlatDeploymentConfig {
    fn from(value: DeploymentConfig) -> Self {
        let (backend, dockerfile_path) = match value.build {
            Some(Build::Dockerfile { path }) => (Some(BuildBackend::Dockerfile), path),
            Some(Build::Nixpacks { .. }) => (Some(BuildBackend::Dockerfile), None), // TODO: provider !!!!!!!!!!!!!!!
            None => (None, None),
        };
        Self {
            visibility: into_opt_str(value.visibility),
            backend: into_opt_str(backend),
            dockerfile_path,
        }
    }
}

impl TryFrom<FlatDeploymentConfig> for DeploymentConfig {
    type Error = anyhow::Error;
    fn try_from(value: FlatDeploymentConfig) -> Result<Self, Self::Error> {
        let backend: Option<BuildBackend> = from_opt_str(value.backend)?;
        let build = if backend == Some(BuildBackend::Dockerfile) {
            Some(Build::Dockerfile {
                path: value.dockerfile_path,
            })
        } else if backend == Some(BuildBackend::Nixpacks) {
            Some(Build::Nixpacks {
                provider: None, // TODO: provider !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
            })
        } else {
            None
        };
        Ok(Self {
            visibility: from_opt_str(value.visibility)?,
            build,
        })
    }
}

// TODO: move this to somewhere that makes sense
pub(crate) fn from_opt_str<T>(input: Option<String>) -> anyhow::Result<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    match input {
        Some(input) => {
            let value = serde_json::Value::String(input);
            Ok(Some(serde_json::from_value(value)?))
        }
        None => Ok(None),
    }
}

fn into_opt_str<T: Serialize>(input: Option<T>) -> Option<String> {
    if let serde_json::Value::String(value) = serde_json::to_value(input?).unwrap() {
        Some(value)
    } else {
        panic!("unexpected json type")
    }
}

impl DeploymentConfig {
    pub(crate) fn get_visibility(&self) -> Visibility {
        self.visibility.clone().unwrap_or(Visibility::Standard)
    }

    pub(crate) fn get_forced_dockerfile(&self) -> Option<&str> {
        if let Some(Build::Dockerfile { path }) = &self.build {
            Some(path.as_deref().unwrap_or("Dockerfile"))
        } else {
            None
        }
    }

    pub(crate) fn is_forced_nixpacks(&self) -> bool {
        if let Some(Build::Nixpacks { provider }) = &self.build {
            true
        } else {
            false
        }
    }

    pub(crate) async fn fetch_from_repo(
        github: &Github,
        repo_id: i64,
        sha: &str,
        root: &str,
        app_name: &str,
    ) -> anyhow::Result<Option<Self>> {
        let custom_path = format!("{app_name}.prezel.json");
        let app_config = fetch_from_path(github, repo_id, sha, root, &custom_path).await?;
        if let Some(config) = app_config {
            Ok(Some(config))
        } else {
            let config = fetch_from_path(github, repo_id, sha, root, "prezel.json").await?;
            Ok(config)
        }
    }
}

async fn fetch_from_path(
    github: &Github,
    repo_id: i64,
    sha: &str,
    root: &str,
    config_file_name: &str,
) -> anyhow::Result<Option<DeploymentConfig>> {
    let conf_path = PathBuf::from(root).join(config_file_name);
    let valid_components = conf_path
        .components()
        .filter(|comp| !matches!(comp, Component::CurDir));
    let valid_path: PathBuf = valid_components.collect();
    let err_msg = "Could not construct a valid path for prezel.json";
    let path_str = valid_path.to_str().ok_or(anyhow!(err_msg))?;
    let content = github.download_file(repo_id, &sha, path_str).await?;
    Ok(content.map(|c| serde_json::from_str(&c)).transpose()?)
}

#[cfg(test)]
mod config_tests {

    use crate::deployments::config::Visibility;

    use super::{Build, DeploymentConfig, FlatDeploymentConfig};

    // TODO: add a test with an unknown field and double check it fails

    #[test]
    fn test_configs() {
        let content = r#"{
            "build": {
                "backend": "dockerfile",
                "config": {
                    "path": "some/path"
                }
            }
        }"#;

        let config: DeploymentConfig = serde_json::from_str(&content).unwrap();

        assert_eq!(
            config.build.unwrap(),
            Build::Dockerfile {
                path: Some("some/path".to_owned()),
            }
        )
    }

    #[test]
    fn test_two_way_conversion() {
        let config = DeploymentConfig {
            visibility: Some(Visibility::Public),
            build: Some(Build::Dockerfile {
                path: Some("some/path".to_owned()),
            }),
        };
        let flat: FlatDeploymentConfig = config.clone().into();
        let back: DeploymentConfig = flat.try_into().unwrap();
        assert_eq!(config, back);
    }
}
