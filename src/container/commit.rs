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
    deployment_hooks::StatusHooks,
    env::EnvVars,
    github::Github,
    sqlite_db::{BranchSqliteDb, ProdSqliteDb},
};

use super::{
    build_dockerfile, BuildOutput, BuildResult, Container, ContainerConfig, ContainerSetup,
    ContainerStatus, DeploymentHooks, WorkerHandle,
};

const DB_PATH_ENV_NAME: &str = "PREZEL_DB_URL";

#[derive(Clone, Debug)]
pub(crate) struct CommitContainer {
    github: Github,
    // main_db_file: HostFile,
    branch_db: Option<BranchSqliteDb>,
    pub(crate) repo_id: String,
    pub(crate) sha: String,
    env: EnvVars,
    root: String,
}

impl CommitContainer {
    pub(crate) fn new(
        build_queue: WorkerHandle,
        hooks: StatusHooks,
        github: Github,
        repo_id: String,
        sha: String,
        deployment: i64,
        env: EnvVars, // TODO: this is duplicated in ContainerConfig...
        root: String,
        branch: bool,
        public: bool, // TODO: should not this be in ContainerConfig
        prod_db: &ProdSqliteDb,
        // cloned_db_file: Option<HostFile>,
        initial_status: ContainerStatus,
        result: Option<BuildResult>,
    ) -> Container {
        let (db_file, branch_db) = if branch {
            let branch_db = prod_db.branch(deployment);
            (branch_db.branch_file.clone(), Some(branch_db))
        } else {
            (prod_db.setup.file.clone(), None)
        };
        let db_path = db_file.get_container_file();
        let db_path_str = db_path.to_str().unwrap();
        let db_url = format!("file:{db_path_str}");
        let default_env = [
            (DB_PATH_ENV_NAME, db_url.as_str()),
            ("ASTRO_DB_REMOTE_URL", db_url.as_str()),
            ("HOST", "0.0.0.0"),
            ("PORT", "80"),
        ]
        .as_ref()
        .into();
        let extended_env = env + default_env;

        let builder = Self {
            github,
            branch_db,
            repo_id,
            sha,
            env: extended_env.clone(),
            root,
        };

        Container::new(
            builder,
            ContainerConfig {
                host_files: vec![db_file],
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

    // TODO: rename this
    #[tracing::instrument]
    async fn build(&self, hooks: &Box<dyn DeploymentHooks>) -> anyhow::Result<BuildOutput> {
        let db_setup = if let Some(branch_db) = &self.branch_db {
            Some(branch_db.setup().await?)
        } else {
            None
        };

        let tempdir = TempDir::new()?;
        let path = tempdir.as_ref();
        let path = self.build_context(path).await?;
        let image = build_dockerfile(&path, self.env.clone(), &mut |chunk| async {
            if let Some(stream) = chunk.stream {
                hooks.on_build_log(&stream, false).await
            } else if let Some(error) = chunk.error {
                hooks.on_build_log(&error, true).await
            }
        })
        .await?;

        Ok(BuildOutput { image, db_setup })
    }

    async fn build_context(&self, path: &Path) -> anyhow::Result<PathBuf> {
        self.github
            .download_commit(&self.repo_id, &self.sha, &path)
            .await
            .unwrap(); // FIXME: this should result in an Error type that causes the build tu be cancelled and retry later on
        assert!(path.exists());

        let inner_path = path.join(&self.root);

        let env_vec: Vec<String> = self.env.clone().into();
        create_docker_image(
            inner_path.to_str().unwrap(),
            env_vec.iter().map(String::as_str).collect(),
            &GeneratePlanOptions::default(),
            &DockerBuilderOptions {
                out_dir: Some(inner_path.to_str().unwrap().to_owned()), // TODO: test what happens if I omit this ?
                // name: Some(name.clone()),
                // print_dockerfile: false,
                // quiet: false,
                // cache_key: None,
                // no_cache: true,
                // inline_cache: false,
                // platform: vec![],
                // current_dir: true,
                // no_error_without_start: true,
                // verbose: true,
                // docker_host: Some("unix:///var/run/docker.sock".to_owned()),
                ..Default::default()
            },
        )
        .await?;

        // FIXME: what if there is a Dockerfile in the root folder of the project????
        // maybe I shouldnt use nixpacks at all in the first place
        fs::rename(
            inner_path.join(".nixpacks").join("Dockerfile"),
            inner_path.join("Dockerfile"),
        )
        .await?;
        Ok(inner_path)
    }
}

impl ContainerSetup for CommitContainer {
    fn build<'a>(
        &'a self,
        hooks: &'a Box<dyn DeploymentHooks>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<BuildOutput>> + Send + 'a>> {
        Box::pin(async move { self.build(hooks).await })
    }
}
