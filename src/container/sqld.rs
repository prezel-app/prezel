use std::path::PathBuf;

use crate::{
    deployment_hooks::NoopHooks, deployments::worker::WorkerHandle, env::EnvVars, paths::HostFile,
};

use super::{BuildResult, Container, ContainerConfig, ContainerSetup, ContainerStatus};

const VERSION: &str = "0.24.28";

#[derive(Clone, Debug)]
pub(crate) struct SqldContainer;

impl SqldContainer {
    pub(crate) fn new(db_file: HostFile, key: &str, build_queue: WorkerHandle) -> Container {
        let builder = Self {};

        let db_path = db_file.get_container_file().display().to_string();
        let command = format!("mkdir -p /tmp/db/dbs && printf {VERSION} > /tmp/db/.version && ln -s {db_path} /tmp/db/data && ln -s /tmp/db /tmp/db/dbs/default && /usr/local/bin/docker-wrapper.sh /bin/sqld");

        Container::new(
            builder,
            ContainerConfig {
                args: EnvVars::empty(),
                host_files: vec![db_file.clone()],
                env: [
                    ("SQLD_HTTP_LISTEN_ADDR", "0.0.0.0:80"),
                    ("SQLD_DB_PATH", "/tmp/db"),
                    ("SQLD_AUTH_JWT_KEY", key),
                ]
                .as_ref() // FIXME: should not need this
                .into(),
                initial_status: ContainerStatus::StandBy {
                    image: format!("ghcr.io/tursodatabase/libsql-server:v{VERSION}"),
                    db_setup: None,
                },
                command: Some(command),
                result: Some(BuildResult::Built),
            },
            build_queue,
            None,
            true, // FIXME: make sure I handle auth at the
            NoopHooks,
        )
    }
}

// FIXME: this being empty clearly means the abstraction is pointless
impl ContainerSetup for SqldContainer {
    fn build<'a>(
        &'a self,
        hooks: &'a Box<dyn super::DeploymentHooks>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<super::BuildOutput>> + Send + 'a>,
    > {
        todo!()
    }
    // fn setup_build_context(&self, path: PathBuf) -> ContextBuilderOutput {
    //     Box::pin(async { Ok(path) }) // FIXME: this is just a placeholder, calling this should not be a possibility
    // }

    // fn setup_filesystem(&self) -> FileSystemOutput {
    //     Box::pin(async { Ok(()) })
    // }
}
