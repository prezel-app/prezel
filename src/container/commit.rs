use anyhow::ensure;
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
};
use tempfile::TempDir;

use crate::{
    db::nano_id::NanoId,
    deployments::config::DeploymentConfig,
    docker::{get_managed_image_id, ImageName},
    env::EnvVars,
    github::Github,
    hooks::StatusHooks,
    nixpacks::create_docker_image_with_nixpacks,
    sqlite_db::{BranchSqliteDb, ProdSqliteDb, SqliteDbSetup},
};

use super::{
    build_dockerfile, BuildResult, Container, ContainerConfig, ContainerSetup, ContainerStatus,
    DeploymentHooks, WorkerHandle,
};

#[derive(Clone, Debug)]
pub(crate) struct CommitContainer {
    github: Github,
    deployment: NanoId,
    branch_db: Option<BranchSqliteDb>,
    pub(crate) repo_id: i64,
    pub(crate) sha: String,
    env: EnvVars,
    root: String,
    config: DeploymentConfig,
}

impl CommitContainer {
    #[tracing::instrument]
    pub(crate) fn new(
        build_queue: WorkerHandle,
        hooks: StatusHooks,
        github: Github,
        repo_id: i64,
        sha: String,
        deployment: NanoId,
        env: EnvVars, // TODO: this is duplicated in ContainerConfig...
        root: String,
        branch: bool,
        public: bool, // TODO: should not this be in ContainerConfig
        prod_db: &ProdSqliteDb,
        db_url: &str,
        // cloned_db_file: Option<HostFile>,
        initial_status: ContainerStatus,
        result: Option<BuildResult>,
        config: DeploymentConfig,
    ) -> Container {
        let (branch_db, token) = if branch {
            let branch_db = prod_db.branch(&deployment);
            let token = branch_db.auth.get_permanent_token().to_owned();
            (Some(branch_db), token)
        } else {
            (None, prod_db.setup.auth.get_permanent_token().to_owned())
        };
        let default_env = [
            ("PREZEL_DB_URL", db_url),
            ("PREZEL_DB_AUTH_TOKEN", &token),
            ("PREZEL_LIBSQL_URL", db_url),
            ("PREZEL_LIBSQL_AUTH_TOKEN", &token),
            ("ASTRO_DB_REMOTE_URL", db_url),
            ("ASTRO_DB_APP_TOKEN", &token),
            ("HOST", "0.0.0.0"),
            ("PORT", "80"),
        ]
        .as_ref()
        .into();
        let extended_env = env + default_env;

        let builder = Self {
            github,
            branch_db,
            deployment: deployment.clone(),
            repo_id,
            sha,
            env: extended_env.clone(),
            root,
            config,
        };

        Container::new(
            builder,
            ContainerConfig {
                host_folders: vec![],
                env: extended_env,
                pull: false,
                initial_status,
                command: None,
                result,
            },
            build_queue,
            Some(deployment),
            public,
            hooks,
        )
    }

    async fn setup_db(&self) -> anyhow::Result<Option<SqliteDbSetup>> {
        let db_setup = if let Some(branch_db) = &self.branch_db {
            Some(branch_db.setup().await?)
        } else {
            None
        };
        Ok(db_setup)
    }

    #[tracing::instrument]
    async fn build(&self, hooks: &Box<dyn DeploymentHooks>) -> anyhow::Result<String> {
        let name: ImageName = self.deployment.to_string().into();
        if let Some(image) = get_managed_image_id(&name).await {
            // TODO: only do this on first run?
            // if build and docker workers do not overlap, I'm safe
            // the problem might be grabbing this id at the same time the image is being removed
            // the same happens with containers
            Ok(image)
        } else {
            let tempdir = TempDir::new()?;
            let (path, dockerfile) = self.build_context(tempdir.as_ref()).await?;
            let image = build_dockerfile(
                name,
                &path,
                dockerfile,
                self.env.clone(),
                &mut |chunk| async {
                    for log in chunk.logs {
                        hooks
                            .on_build_log(&String::from_utf8_lossy(&log.msg), false)
                            .await // FIXME: use time returned by docker in log.timestamp !!!!!!!!!! below as well!!
                    }
                    for vertex in chunk.vertexes {
                        if vertex.completed.is_some() {
                            if vertex.cached {
                                let name = vertex.name;
                                hooks.on_build_log(&format!("CACHED {name}"), false).await;
                            } else {
                                hooks.on_build_log(&vertex.name, false).await;
                            }
                        }
                        if !vertex.error.is_empty() {
                            hooks.on_build_log(&vertex.error, true).await
                        }
                    }
                },
            )
            .await?;
            Ok(image)
        }
    }

    #[tracing::instrument]
    async fn build_context(&self, path: &Path) -> anyhow::Result<(PathBuf, String)> {
        self.github
            .download_commit(self.repo_id, self.sha.clone(), &path)
            .await?;
        ensure!(path.exists());

        let inner_path = path.join(&self.root);

        let default_dockerfile = "Dockerfile".to_owned();
        let default_dockerfile_present = inner_path.join(&default_dockerfile).exists();

        if let Some(dockerfile) = self.config.get_forced_dockerfile() {
            Ok((inner_path, dockerfile.to_owned()))
        } else if self.config.is_forced_nixpacks() || !default_dockerfile_present {
            let env_vec: Vec<String> = self.env.clone().into();
            create_docker_image_with_nixpacks(
                &inner_path,
                env_vec.iter().map(String::as_str).collect(),
            )
            .await?;
            Ok((inner_path, default_dockerfile))
        } else {
            Ok((inner_path, default_dockerfile))
        }
    }
}

impl ContainerSetup for CommitContainer {
    fn setup_db<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<SqliteDbSetup>>> + Send + 'a>> {
        Box::pin(self.setup_db())
    }
    fn build<'a>(
        &'a self,
        hooks: &'a Box<dyn DeploymentHooks>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + 'a>> {
        Box::pin(async move { self.build(hooks).await })
    }
}
