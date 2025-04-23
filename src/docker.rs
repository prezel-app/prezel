// TODO: maybe this should be as well on the container module

use anyhow::anyhow;
use bollard::models::BuildInfoAux;
use bollard::{
    container::{
        Config, CreateContainerOptions, ListContainersOptions, LogOutput, LogsOptions,
        NetworkingConfig, StartContainerOptions,
    },
    image::{BuildImageOptions, CreateImageOptions},
    secret::{BuildInfo, HostConfig, ImageInspect},
    Docker,
};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use hyper::body::Bytes;
use nanoid::nanoid;
use serde::Serialize;
use std::{
    error::Error,
    future::{self, Future},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    pin::Pin,
};
use utoipa::ToSchema;

use crate::{env::EnvVars, utils::LOWERCASE_PLUS_NUMBERS};

#[tracing::instrument]
pub(crate) fn docker_client() -> Docker {
    Docker::connect_with_unix_defaults().unwrap()
}

const NETWORK_NAME: &'static str = "prezel";
const CONTAINER_PREFIX: &'static str = "prezel-";

// TODO: instead of this returna DockerContainerHandle that you can call create and start against
pub(crate) fn generate_managed_container_name() -> String {
    let suffix = nanoid!(21, &LOWERCASE_PLUS_NUMBERS);
    format!("{CONTAINER_PREFIX}{suffix}",)
}

pub(crate) fn generate_unmanaged_container_name() -> String {
    nanoid!(21, &LOWERCASE_PLUS_NUMBERS)
}

// TPODO: move all of this into a different folder to enforce usage of the function to return the real name
#[derive(Debug, Clone)]
pub(crate) struct ImageName(String);
impl ImageName {
    fn to_docker_name(&self) -> String {
        format!("{CONTAINER_PREFIX}{}", self.0)
    }
}
impl From<String> for ImageName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[tracing::instrument]
pub(crate) async fn get_bollard_container_ipv4(container_id: &str) -> Option<Ipv4Addr> {
    let docker = docker_client();
    let response = docker.inspect_container(container_id, None).await.ok()?;
    let networks = response.network_settings?.networks?;
    let ip = networks.get(NETWORK_NAME)?.ip_address.as_ref()?;
    ip.parse::<Ipv4Addr>().ok()
}

// TODO: move this to common place
#[derive(Serialize, Debug, Clone, ToSchema)]
pub(crate) struct DockerLog {
    pub(crate) time: i64,
    pub(crate) message: String,
    pub(crate) log_type: LogType,
}

#[derive(Serialize, Debug, Clone, ToSchema, PartialEq, Eq)]
pub(crate) enum LogType {
    Out,
    Err,
}

#[tracing::instrument]
pub(crate) async fn get_container_execution_logs(id: &str) -> impl Iterator<Item = DockerLog> {
    let docker = docker_client();
    let logs = docker
        .logs(
            id,
            Some(LogsOptions {
                stderr: true,
                stdout: true,
                since: 0,
                until: 100_000_000_000, // far into the future
                timestamps: true,
                tail: "all",
                ..Default::default()
            }),
        )
        .collect::<Vec<_>>()
        .await;

    logs.into_iter().filter_map(|chunk| match chunk {
        Ok(LogOutput::StdOut { message }) => {
            parse_message(message).map(|(time, content)| DockerLog {
                time,
                message: content,
                log_type: LogType::Out,
            })
        }
        Ok(LogOutput::StdErr { message }) => {
            parse_message(message).map(|(time, content)| DockerLog {
                time,
                message: content,
                log_type: LogType::Err,
            })
        } // FIXME: unwrap?
        _ => None,
    })
}

fn parse_message(message: Bytes) -> Option<(i64, String)> {
    let utf8 = String::from_utf8(message.into()).ok()?;
    let (timestamp, content) = utf8.split_once(" ")?;

    let datetime: DateTime<Utc> = timestamp.parse().expect("Failed to parse timestamp");
    let millis = datetime.timestamp_millis();

    Some((millis, content.to_owned()))
}

pub(crate) async fn get_managed_image_id(name: &ImageName) -> Option<String> {
    get_image(&name.to_docker_name()).await?.id
}

pub(crate) async fn get_image(name: &str) -> Option<ImageInspect> {
    let docker = docker_client();
    let image = docker.inspect_image(name).await;
    image.ok()
}

pub(crate) async fn get_prezel_image_version() -> Option<String> {
    let docker = docker_client();
    let container = docker.inspect_container("prezel", None).await.ok()?;
    Some(container.config?.image?.replace("prezel/prezel:", ""))
}

pub(crate) async fn pull_image(image: &str) {
    let docker = docker_client();
    docker
        .create_image(
            Some(CreateImageOptions {
                from_image: image,
                ..Default::default()
            }),
            None,
            None,
        )
        .count() // is this really the most appropriate option?
        .await;
}

pub(crate) async fn create_container<'a, I: Iterator<Item = &'a PathBuf>>(
    name: String,
    image: String,
    env: EnvVars,
    host_folders: I,
    command: Option<String>,
) -> anyhow::Result<String> {
    let binds = host_folders
        .map(|folder| {
            let path = folder.display().to_string();
            format!("{path}:{path}")
        })
        .collect();
    create_container_with_explicit_binds(name, image, env, binds, command).await
}

pub(crate) async fn create_container_with_explicit_binds(
    name: String,
    image: String,
    env: EnvVars,
    binds: Vec<String>,
    command: Option<String>,
) -> anyhow::Result<String> {
    let entrypoint = command
        .is_some()
        .then(|| vec!["sh".to_owned(), "-c".to_owned()]);
    let cmd = command.map(|command| vec![command]);
    let docker = docker_client();

    let response = docker
        .create_container::<String, _>(
            Some(CreateContainerOptions {
                name,
                platform: None,
            }),
            Config {
                image: Some(image),
                cmd,
                entrypoint,
                env: Some(env.into()),
                host_config: Some(HostConfig {
                    binds: Some(binds),
                    ..Default::default()
                }),
                networking_config: Some(NetworkingConfig {
                    endpoints_config: [(NETWORK_NAME.to_owned(), Default::default())].into(),
                }),
                ..Default::default()
            },
        )
        .await?;
    Ok(response.id)
}

#[tracing::instrument]
pub(crate) async fn run_container(id: &str) -> Result<(), impl Error> {
    let docker = docker_client();
    docker
        .start_container(id, None::<StartContainerOptions<String>>)
        .await
}

// #[tracing::instrument]
pub(crate) async fn build_dockerfile<
    O: Future<Output = ()>,
    F: FnMut(bollard::moby::buildkit::v1::StatusResponse) -> O,
>(
    name: ImageName,
    path: &Path,
    buildargs: EnvVars,
    process_chunk: &mut F,
) -> anyhow::Result<String> {
    // let image_name = nanoid!(21, &alphabet::LOWERCASE_PLUS_NUMBERS);
    let name = name.to_docker_name();

    let mut archive_builder = tar::Builder::new(Vec::new());
    archive_builder.append_dir_all(".", path).unwrap();
    let tar_content = archive_builder.into_inner().unwrap();

    let docker = docker_client();

    let mut build_stream = docker.build_image(
        BuildImageOptions {
            t: name.clone(),
            buildargs: buildargs.into(),
            rm: true,
            forcerm: true, // rm intermediate containers even if the build fails
            version: bollard::image::BuilderVersion::BuilderBuildKit,
            session: Some(name.clone()), // the idea of using the name as session id comes from some bollard example
            ..Default::default()
        },
        None,
        Some(tar_content.into()),
    );
    while let Some(Ok(BuildInfo { aux, .. })) = build_stream.next().await {
        if let Some(BuildInfoAux::BuildKit(log)) = aux {
            process_chunk(log).await;
        }
    }
    // TODO: return from here a stream of logs and a final status at the end, which is a Result (the thing returned below)
    let image = docker.inspect_image(&name).await?;
    image.id.ok_or(anyhow!("Image not found"))
}

#[tracing::instrument]
pub(crate) async fn stop_container(name: &str) -> anyhow::Result<()> {
    let docker = docker_client();
    docker.stop_container(name, None).await?;
    Ok(())
}

#[tracing::instrument]
pub(crate) async fn delete_container(name: &str) -> anyhow::Result<()> {
    let docker = docker_client();
    docker.remove_container(name, None).await?;
    Ok(())
}

#[tracing::instrument]
pub(crate) async fn delete_image(name: &str) -> anyhow::Result<()> {
    let docker = docker_client();
    docker.remove_image(name, None, None).await?;
    Ok(())
}

#[tracing::instrument]
pub(crate) async fn list_managed_container_names() -> anyhow::Result<impl Iterator<Item = String>> {
    let prefix = format!("/{CONTAINER_PREFIX}");
    let docker = docker_client();
    let opts: ListContainersOptions<String> = ListContainersOptions {
        all: true,
        ..Default::default()
    };
    let containers = docker.list_containers(Some(opts)).await?;
    Ok(containers
        .into_iter()
        .filter_map(|summary| summary.names)
        .filter_map(move |names| names.get(0).cloned())
        .filter_map(move |name| {
            if name.starts_with(&prefix) {
                Some(name.replace("/", ""))
            } else {
                None
            }
        }))
}

#[cfg(test)]
mod docker_tests {
    use crate::docker::{create_container, get_bollard_container_ipv4, run_container};

    // #[tokio::test]
    // async fn test_list_containers() {
    //     let ids = list_container_ids().await.unwrap();
    // }

    #[tokio::test]
    async fn test_creating_and_running_container() {
        // let image = build_dockerfile(path, self.config.args.clone(), &mut |chunk| {
        //     println!("prisma dockerfile chunk: {chunk:?}")
        // }) // TODO: make this more readable
        // .await?;
        // let image = image.inspect().await?;
        // let image_id = image.id.ok_or(anyhow!("Image not found"));

        // let id = create_container(
        //     "busybox".to_owned(),
        //     Default::default(),
        //     [].into_iter(),
        //     None,
        // )
        // .await
        // .unwrap();
        // run_container(&id).await.unwrap();
        // let _ip = get_bollard_container_ipv4(&id).await.unwrap();

        // run_container("zen_wright").await.unwrap();

        // let container = docker_client()
        //     .containers()
        //     .create(
        //         &ContainerCreateOpts::builder()
        //             .image("prisma")
        //             .expose(PublishPort::tcp(80), 80)
        //             // .volumes(volumes)
        //             .build(),
        //     )
        //     .await
        //     .unwrap();
    }

    // #[tokio::test]
    // async fn test_nixpacks() {
    //     let path = Path::new("./examples/astro-drizzle");
    //     let name = "nixpacks-test".to_owned();
    //     let env = [
    //         "DATABASE_URL=/opt/prezel/sqlite/main.db",
    //         "HOST=0.0.0.0",
    //         "PORT=80",
    //     ]
    //     .into_iter()
    //     .map(|env| env.to_owned())
    //     .collect::<Vec<_>>();
    //     create_docker_image_for_path(path, name, env, &mut |chunk| match chunk {
    //         ImageBuildChunk::Update { stream } => println!("{}", stream),
    //         ImageBuildChunk::Error { error, .. } => println!("{}", error),
    //         _ => {}
    //     })
    //     .await
    //     .unwrap();
    // }
}
