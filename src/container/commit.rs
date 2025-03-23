use anyhow::ensure;
use nixpacks::{
    create_docker_image,
    nixpacks::{builder::docker::DockerBuilderOptions, plan::generator::GeneratePlanOptions},
};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
};
use tempfile::TempDir;
use tokio::fs;

use crate::{
    db::nano_id::NanoId,
    docker::{get_managed_image_id, ImageName},
    env::EnvVars,
    github::Github,
    hooks::StatusHooks,
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
    // main_db_file: HostFile,
    branch_db: Option<BranchSqliteDb>,
    pub(crate) repo_id: i64,
    pub(crate) sha: String,
    env: EnvVars,
    root: String,
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
            let path = tempdir.as_ref();
            let path = self.build_context(path).await?;
            let image = build_dockerfile(name, &path, self.env.clone(), &mut |chunk| async {
                if let Some(stream) = chunk.stream {
                    hooks.on_build_log(&stream, false).await
                } else if let Some(error) = chunk.error {
                    hooks.on_build_log(&error, true).await
                }
            })
            .await?;
            Ok(image)
        }
    }

    #[tracing::instrument]
    async fn build_context(&self, path: &Path) -> anyhow::Result<PathBuf> {
        self.github
            .download_commit(self.repo_id, &self.sha, &path)
            .await?;
        ensure!(path.exists());

        let inner_path = path.join(&self.root);

        if !inner_path.join("Dockerfile").exists() {
            self.create_dockerfile_with_nixpacks(&inner_path).await?;
        }

        Ok(inner_path)
    }

    #[tracing::instrument]
    async fn create_dockerfile_with_nixpacks(&self, inner_path: &Path) -> anyhow::Result<()> {
        let env_vec: Vec<String> = self.env.clone().into();
        create_docker_image(
            inner_path.to_str().unwrap(),
            env_vec.iter().map(String::as_str).collect(),
            &GeneratePlanOptions::default(),
            &DockerBuilderOptions {
                out_dir: Some(inner_path.display().to_string()), // TODO: test what happens if I omit this ?
                // quiet: true,
                // verbose: false,
                // name: Some(name.clone()),
                // print_dockerfile: false,
                // cache_key: None,
                // no_cache: true,
                // inline_cache: false,
                // platform: vec![],
                // current_dir: true,
                // no_error_without_start: true,
                // docker_host: Some("unix:///var/run/docker.sock".to_owned()),
                ..Default::default()
            },
        )
        .await?;

        fs::rename(
            inner_path.join(".nixpacks").join("Dockerfile"),
            inner_path.join("Dockerfile"),
        )
        .await?;

        Ok(())
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
