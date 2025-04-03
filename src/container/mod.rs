use anyhow::{anyhow, bail};
use async_trait::async_trait;
use reqwest::Client;
use std::{
    fmt,
    future::Future,
    net::SocketAddrV4,
    ops::Deref,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::RwLock, time::sleep};
use tracing::error;

use crate::{
    api::Status,
    db::{nano_id::NanoId, BuildResult},
    deployments::worker::WorkerHandle,
    docker::{
        build_dockerfile, create_container, get_bollard_container_ipv4,
        get_container_execution_logs, pull_image, run_container, DockerLog,
    },
    env::EnvVars,
    hooks::DeploymentHooks,
    listener::{Access, Listener},
    sqlite_db::SqliteDbSetup,
    utils::now,
};

pub(crate) mod commit;
pub(crate) mod sqld;

#[derive(Debug)]
pub(crate) struct ContainerConfig {
    pub(crate) env: EnvVars,
    pub(crate) pull: bool,
    pub(crate) host_folders: Vec<PathBuf>,
    pub(crate) command: Option<String>, // TODO: review if I am using this
    pub(crate) initial_status: ContainerStatus,
    pub(crate) result: Option<BuildResult>,
}

// pub(crate) type ContextBuilderOutput =
//     Pin<Box<dyn Future<Output = anyhow::Result<PathBuf>> + Send>>;
// pub(crate) type FileSystemOutput = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

pub(crate) trait ContainerSetup: 'static + Send + Sync + fmt::Debug {
    fn setup_db<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<SqliteDbSetup>>> + Send + 'a>>;
    fn build<'a>(
        &'a self,
        hooks: &'a Box<dyn DeploymentHooks>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + 'a>>;
}

#[derive(Debug, Clone)]
pub(crate) enum ContainerStatus {
    // FIXME ideally I would save the image in the db as well, since anyways there is no guarantee
    // the image pointed by StandBy {image} exists
    // I could store the image id in case the build was successful
    // if there is build_start and build_end but not image id, I can assume the build failed
    // To be fair, if I detect an image has been removed, I should change the status to be None...
    /// this means the container was previously built successfully but the image is not known anymore
    Built,
    Queued {
        trigger_access: Option<Instant>,
    },
    Building {
        db_setup: Option<SqliteDbSetup>,
    },
    StandBy {
        image: String,
        db_setup: Option<SqliteDbSetup>,
    },
    Starting {
        image: String,
        db_setup: Option<SqliteDbSetup>,
    },
    Ready {
        image: String,
        db_setup: Option<SqliteDbSetup>,
        container: String,
        socket: SocketAddrV4,
        last_access: Arc<RwLock<Instant>>,
    },
    Failed,
}

impl ContainerStatus {
    #[tracing::instrument]
    fn get_container_id(&self) -> Option<String> {
        if let Self::Ready { container, .. } = self {
            Some(container.clone())
        } else {
            None
        }
    }

    #[tracing::instrument]
    pub(crate) fn to_status(&self) -> Status {
        match self {
            Self::Built => Status::Built,
            Self::StandBy { .. } | Self::Starting { .. } => Status::StandBy, // not relevant for the user?
            Self::Building { .. } => Status::Building,
            Self::Queued { .. } => Status::Queued,
            Self::Ready { .. } => Status::Ready,
            Self::Failed => Status::Failed,
        }
    }

    // TODO: create get_db_container alternative and use it in all the appropriate places
    // or maybe simply implement to_container for Option<SqliteDbSetup>
    #[tracing::instrument]
    pub(crate) fn get_db_setup(&self) -> Option<SqliteDbSetup> {
        match self {
            Self::Building { db_setup }
            | Self::StandBy { db_setup, .. }
            | Self::Ready { db_setup, .. }
            | Self::Starting { db_setup, .. } => db_setup.clone(),
            Self::Queued { .. } | Self::Built | Self::Failed => None,
        }
    }
}

// Potential problems to be aware of
// - Two builds should not be started at the same time for the same container
// - Two docker containers should not be created at the same time for the same container
// - A container that is waiting to be created should not be removed imediately if there are clients actively requesting it

/// This is a lazy container, it might be that it is not running, or even that the container is deleted, or even that
/// the image itself to create this container was deleted, and it will make sure, upon access, to rebuild the image / start the container
/// before responding with the socket address
#[derive(Debug)]
pub(crate) struct Container {
    pub(crate) status: RwLock<ContainerStatus>,
    pub(crate) result: RwLock<Option<BuildResult>>,
    setup: Box<dyn ContainerSetup>,
    config: ContainerConfig,
    hooks: Box<dyn DeploymentHooks>,
    pub(crate) logging_deployment_id: Option<NanoId>,
    pub(crate) public: bool,
    build_queue: WorkerHandle,
}

impl Container {
    // TODO: remove this function and just make the private fields public within the module (they are already no?)
    #[tracing::instrument]
    pub(crate) fn new(
        setup: impl ContainerSetup,
        config: ContainerConfig,
        build_queue: WorkerHandle,
        logging_deployment_id: Option<NanoId>,
        public: bool,
        hooks: impl DeploymentHooks,
    ) -> Self {
        Self {
            status: config.initial_status.clone().into(),
            result: RwLock::new(config.result),
            setup: Box::new(setup),
            config,
            hooks: Box::new(hooks),
            logging_deployment_id,
            public,
            build_queue,
        }
    }

    // TODO: review, do we really need to expose the container id in the api?
    #[tracing::instrument]
    pub(crate) async fn get_container_id(&self) -> Option<String> {
        self.status.read().await.get_container_id()
    }

    #[tracing::instrument]
    pub(crate) async fn get_logs(&self) -> Box<dyn Iterator<Item = DockerLog>> {
        if let Some(container) = self.get_container_id().await {
            Box::new(get_container_execution_logs(&container).await)
        } else {
            Box::new(std::iter::empty())
        }
    }

    /// this function runs no sanity checks on the current status before setting the new one
    #[tracing::instrument]
    pub(crate) async fn enqueue(&self) {
        *self.status.write().await = ContainerStatus::Queued {
            trigger_access: None,
        };
    }

    // FIXME: this i pointless now, just a thin wrapper
    #[tracing::instrument]
    pub(crate) async fn setup_as_standby(&self) -> anyhow::Result<()> {
        self.build().await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn downgrade_if_unused(&self) {
        let new_status = if let ContainerStatus::Ready {
            image,
            last_access,
            db_setup,
            ..
        } = self.status.read().await.deref()
        {
            let last_access = last_access.read().await;
            let elapsed = Instant::now().checked_duration_since(*last_access);
            if elapsed.is_some_and(|elapsed| elapsed > Duration::from_secs(30)) {
                Some(ContainerStatus::StandBy {
                    image: image.clone(),
                    db_setup: db_setup.clone(),
                })
            } else {
                None
            }
        } else {
            None
        };

        if let Some(new_status) = new_status {
            *self.status.write().await = new_status;
        }
    }

    #[tracing::instrument]
    async fn build(&self) -> anyhow::Result<()> {
        // FIXME: I think there might be a race condition here where the container build is started twice
        // at the same time...
        self.hooks.on_build_started().await;

        let db_setup = self.setup.setup_db().await?;

        *self.status.write().await = ContainerStatus::Building {
            db_setup: db_setup.clone(),
        };

        match self.setup.build(&self.hooks).await {
            Ok(image) => {
                self.hooks.on_build_finished().await;
                *self.result.write().await = Some(BuildResult::Built);
                *self.status.write().await = ContainerStatus::StandBy { image, db_setup };
            }
            Err(error) => {
                error!("{}", error);
                self.hooks.on_build_log(&error.to_string(), true).await;
                self.hooks.on_build_failed().await;
                *self.status.write().await = ContainerStatus::Failed;
                *self.result.write().await = Some(BuildResult::Failed);
            }
        }
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn start(&self) -> anyhow::Result<SocketAddrV4> {
        let (owned_start, image, db_setup) = {
            dbg!();
            let mut current = self.status.write().await;
            dbg!();
            if let ContainerStatus::StandBy { image, db_setup } = current.clone() {
                *current = ContainerStatus::Starting {
                    image: image.clone(),
                    db_setup: db_setup.clone(),
                };
                (true, image, db_setup)
            } else if let ContainerStatus::Starting { image, db_setup } = current.deref() {
                (false, image.clone(), db_setup.clone())
            } else if let ContainerStatus::Ready { socket, .. } = current.deref() {
                return Ok(socket.clone());
            } else {
                bail!("Tried to start container in a state different than StandBy or Starting")
            }
        };

        if owned_start {
            dbg!();
            if self.config.pull {
                dbg!();
                pull_image(&image).await;
                dbg!();
            }
            let container = create_container(
                image.clone(),
                self.config.env.clone(),
                self.config.host_folders.iter(),
                self.config.command.clone(),
            )
            .await?;
            dbg!();
            run_container(&container).await?;
            dbg!();

            let ip = get_bollard_container_ipv4(&container)
                .await
                .ok_or(anyhow!("Could not get IP for container"))?;
            dbg!();
            let socket = SocketAddrV4::new(ip, 80);
            let timeout = now() + 60 * 1000; // 60 seconds
            while !is_online(&socket.to_string()).await {
                dbg!();
                if now() > timeout {
                    let logs: String = get_container_execution_logs(&container)
                        .await
                        .map(|log| log.message)
                        .collect();
                    bail!("Container {container} start timed out. See the logs below:\n{logs}");
                }
                dbg!();
                sleep(Duration::from_millis(200)).await;
            }

            // TODO: this is better, but doesnt compile
            // let online = stream::iter(0..(5 * 30))
            //     .then(|_| async {
            //         if is_online(&socket.to_string()).await {
            //             true
            //         } else {
            //             sleep(Duration::from_millis(200)).await;
            //             false
            //         }
            //     })
            //     .take_while(|online| future::ready(!online))
            //     .collect::<Vec<_>>()
            //     .await
            //     .last();
            // if online !== Some(true) {
            //     bail!("Container start timed out");
            // }

            dbg!();
            *self.status.write().await = ContainerStatus::Ready {
                image: image.clone(),
                db_setup,
                container,
                socket,
                last_access: RwLock::new(Instant::now()).into(),
            };
            dbg!();

            Ok(socket)
        } else {
            // FIXME: unbounded loop
            loop {
                if let ContainerStatus::Ready { socket, .. } = self.status.read().await.clone() {
                    return Ok(socket);
                }
                sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

#[async_trait]
impl Listener for Arc<Container> {
    fn is_public(&self) -> bool {
        self.public
    }

    #[tracing::instrument]
    async fn access(&self) -> anyhow::Result<Access> {
        let socket = match self.status.read().await.deref() {
            ContainerStatus::Ready {
                socket,
                last_access,
                ..
            } => {
                *last_access.write().await = Instant::now();
                Some(socket.clone())
            }
            _ => None,
        };
        dbg!(&socket);

        // all of this duplication is just to avoid holding a read lock onto the status...
        // wait but Im creating it anyways

        // FIXME: should aquire the lock at this point, so even if allow reads,
        // I know no one is going to be changing the status in the meantime.
        // If so, I should pass the WritableStatus down to container.start() / container.build()
        //
        // FIXME: instead of AtomicStatus, I don't think it is the end of the world aquiring a write lock on the status for a container that is not in prod in ready mode

        match socket {
            Some(socket) => Ok(Access::Socket(socket.clone())),
            None => {
                let status = self.status.read().await.clone();
                match status {
                    ContainerStatus::Ready {
                        socket,
                        last_access,
                        ..
                    } => {
                        // FIXME: boilerplate in here
                        *last_access.write().await = Instant::now();
                        Ok(Access::Socket(socket.clone()))
                    }
                    ContainerStatus::StandBy { .. } | ContainerStatus::Starting { .. } => {
                        dbg!();
                        let socket = self.start().await?;
                        Ok(Access::Socket(socket))
                    }
                    ContainerStatus::Built => {
                        *self.status.write().await = ContainerStatus::Queued {
                            trigger_access: Some(Instant::now()),
                        };
                        self.build_queue.trigger();
                        Ok(Access::Loading)
                    }
                    ContainerStatus::Queued { .. } | ContainerStatus::Building { .. } => {
                        Ok(Access::Loading)
                    }
                    ContainerStatus::Failed => {
                        bail!("container failed to build")
                    }
                }
            }
        }
    }
}

// FIXME: this might fail, especially for some API server with no / route
// there has to be another way
#[tracing::instrument]
async fn is_online(host: &str) -> bool {
    let url = format!("http://{host}");
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let response = client.get(url).send().await;
    dbg!(&response);
    // let response = reqwest::get(url).await;
    // TODO: review if this is actually enough or I need to do the thing below instead
    response.is_ok()
    // match response {
    //     Ok(response) => response.status() == StatusCode::OK,
    //     Err(_) => false,
    // }
}
