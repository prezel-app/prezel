// TODO: maybe this should be as well on the container module

use anyhow::anyhow;
use bollard::{
    container::{
        Config, CreateContainerOptions, ListContainersOptions, LogOutput, LogsOptions,
        NetworkingConfig, StartContainerOptions,
    },
    errors::Error as DockerError,
    image::{BuildImageOptions, CreateImageOptions},
    secret::{BuildInfo, HostConfig},
    Docker as BollardDoker,
};
use chrono::{DateTime, Utc};
// use docker_api::{
//     // models::{EndpointSettings, NetworkingConfig, Volume},
//     opts::{ContainerCreateOpts, NetworkCreateOpts, VolumeCreateOpts},
//     Container,
//     Docker,
//     Image,
//     Network,
// };
use futures::StreamExt;
use hyper::body::Bytes;
use nanoid::nanoid;
use serde::Serialize;
use std::{
    error::Error,
    future::{self, Future},
    net::Ipv4Addr,
    path::Path,
    pin::Pin,
};
use utoipa::ToSchema;

use crate::{alphabet, env::EnvVars, paths::HostFile};

// pub(crate) fn legacy_docker_client() -> Docker {
//     Docker::unix("/var/run/docker.sock")
// }

#[tracing::instrument]
pub(crate) fn docker_client() -> BollardDoker {
    BollardDoker::connect_with_unix_defaults().unwrap()
}

const NETWORK_NAME: &'static str = "prezel";
const CONTAINER_PREFIX: &'static str = "prezel-";

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

// #[tracing::instrument]
pub(crate) async fn create_container<'a, I: Iterator<Item = &'a HostFile>>(
    image: String,
    env: EnvVars,
    host_files: I,
    command: Option<String>,
) -> anyhow::Result<String> {
    let entrypoint = command
        .is_some()
        .then(|| vec!["sh".to_owned(), "-c".to_owned()]);
    let cmd = command.map(|command| vec![command]);
    let docker = docker_client();
    let binds = host_files
        .map(|file| {
            let host = file.get_host_folder().to_str().unwrap().to_owned();
            let container = file.get_container_folder().to_str().unwrap().to_owned();
            format!("{host}:{container}")
        })
        .collect();
    let id = nanoid!(21, &alphabet::LOWERCASE_PLUS_NUMBERS);
    let name = format!("{CONTAINER_PREFIX}{id}",);
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
pub(crate) async fn build_dockerfile<O: Future<Output = ()> + Send, F: FnMut(BuildInfo) -> O>(
    path: &Path,
    buildargs: EnvVars,
    process_chunk: &mut F,
) -> anyhow::Result<String> {
    let image_name = nanoid!(21, &alphabet::LOWERCASE_PLUS_NUMBERS);

    let mut archive_builder = tar::Builder::new(Vec::new());
    archive_builder.append_dir_all(".", path).unwrap();
    let tar_content = archive_builder.into_inner().unwrap();

    let docker = docker_client();

    docker
        .build_image(
            BuildImageOptions {
                t: image_name.clone(),
                buildargs: buildargs.into(),
                rm: true,
                forcerm: true, // rm intermediate containers even if the build fails
                ..Default::default()
            },
            None,
            Some(tar_content.into()),
        )
        .for_each(|chunk| {
            let output: Pin<Box<dyn Future<Output = ()> + Send>> = match chunk {
                // chunk.id // TODO: use this as the image id so I don't have to generate one?
                //     // chunk.aux // or maybe this
                Ok(chunk) => Box::pin(process_chunk(chunk)),
                Err(error) => {
                    if let DockerError::DockerStreamError { error } = error {
                        Box::pin(process_chunk(BuildInfo {
                            error: Some(error),
                            ..Default::default() // TODO: this is a bit hacky, is this really equivalent
                        }))
                    } else {
                        Box::pin(future::ready(()))
                    }
                }
            };
            output
        })
        .await;

    let image = docker.inspect_image(&image_name).await?;
    image.id.ok_or(anyhow!("Image not found"))
}

// #[derive(Debug, Clone)]
// pub(crate) struct DockerBuilder {
//     docker: BollardDoker,
// }

// impl DockerBuilder {
//     pub(crate) async fn build_dockerfile<
//         O: Future<Output = ()> + Send,
//         F: FnMut(BuildInfo) -> O,
//     >(
//         &self,
//         path: &Path,
//         buildargs: EnvVars,
//         process_chunk: &mut F,
//     ) -> anyhow::Result<String> {
//         let image_name = nanoid!(21, &alphabet::LOWERCASE_PLUS_NUMBERS);

//         let mut archive_builder = tar::Builder::new(Vec::new());
//         archive_builder.append_dir_all(".", path).unwrap();
//         let tar_content = archive_builder.into_inner().unwrap();

//         self.docker
//             .build_image(
//                 BuildImageOptions {
//                     t: image_name.clone(),
//                     buildargs: buildargs.into(),
//                     rm: true,
//                     forcerm: true, // rm intermediate containers even if the build fails
//                     ..Default::default()
//                 },
//                 None,
//                 Some(tar_content.into()),
//             )
//             .for_each(|chunk| {
//                 let output: Pin<Box<dyn Future<Output = ()> + Send>> = match chunk {
//                     // chunk.id // TODO: use this as the image id so I don't have to generate one?
//                     //     // chunk.aux // or maybe this
//                     Ok(chunk) => {
//                         Box::pin(process_chunk(chunk))
//                     }
//                     Err(error) => {
//                         if let DockerError::DockerStreamError { error } = error {
//                             Box::pin(process_chunk(BuildInfo {
//                                 error: Some(error),
//                                 ..Default::default() // TODO: this is a bit hacky, is this really equivalent
//                             }))
//                         } else {
//                             Box::pin(future::ready(()))
//                         }
//                     }
//                 };
//                 output
//             })
//             .await;

//         let image = self.docker.inspect_image(&image_name).await?;
//         image.id.ok_or(anyhow!("Image not found"))
//     }
// }

// #[derive(Debug, Clone)]
// pub(crate) struct Docker {
//     builder: Arc<Mutex<DockerBuilder>>,
//     docker: BollardDoker,
// }

// impl Default for Docker {
//     fn default() -> Self {
//         let docker = docker_client();
//         Self {
//             docker: docker.clone(),
//             builder: Mutex::new(DockerBuilder { docker }).into(),
//         }
//     }
// }

// impl Docker {
//     pub(crate) async fn aquire_builder<'a>(&'a self) -> MutexGuard<'a, DockerBuilder> {
//         self.builder.lock().await
//     }
// }

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
pub(crate) async fn list_managed_container_ids() -> anyhow::Result<impl Iterator<Item = String>> {
    let docker = docker_client();
    let opts: ListContainersOptions<String> = ListContainersOptions {
        all: true,
        ..Default::default()
    };
    let containers = docker.list_containers(Some(opts)).await?;

    Ok(containers
        .into_iter()
        .filter(move |summary| match &summary.names {
            Some(names) => names
                .get(0)
                .is_some_and(|name| name.starts_with(&format!("/{CONTAINER_PREFIX}"))),
            None => true,
        })
        .filter_map(|summary| summary.id))
}

// pub(crate) async fn get_stats() -> anyhow::Result<String> {
//     let docker = docker_client();

//     let containers = docker
//         .list_containers(Some(ListContainersOptions {
//             // all: true, for the stats I guess I only want to see running containers
//             ..Default::default()
//         }))
//         .await?
//         .into_iter()
//         .filter_map(|summary| summary.id)
//         .collect::<Vec<_>>();

//     docker.stats(container_name, options);

//     // https://docs.docker.com/reference/api/engine/version/v1.47/#tag/System/operation/SystemDataUsage

//     todo!()
// }

#[cfg(test)]
mod docker_tests {
    use crate::{
        docker::{create_container, get_bollard_container_ipv4, run_container},
        paths::HostFile,
    };

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

        let container = create_container(
            "busybox".to_owned(),
            Default::default(),
            [].into_iter(),
            None,
        )
        .await
        .unwrap();
        run_container(&container).await.unwrap();
        let ip = get_bollard_container_ipv4(&container).await.unwrap();

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
