use std::{future::Future, sync::Arc};

use futures::StreamExt;
use rand::seq::SliceRandom;

use crate::{
    container::{Container, ContainerStatus},
    db::Db,
    deployments::{
        manager::InstrumentedRwLock,
        map::DeploymentMap,
        worker::{Worker, WorkerHandle},
    },
    github::Github,
    utils::LogError,
};

#[derive(Clone, Debug)]
pub(crate) struct BuildWorker {
    // TODO: define a new function instead of having these public, same for other workers
    pub(crate) map: Arc<InstrumentedRwLock<DeploymentMap>>,
    pub(crate) db: Db,
    pub(crate) github: Github,
    pub(crate) build_queue: WorkerHandle,
}

impl Worker for BuildWorker {
    #[tracing::instrument]
    fn work(&self) -> impl Future<Output = ()> + Send {
        async {
            loop {
                if let Some(container) = self.get_container_to_build().await {
                    container.setup_as_standby().await.ignore_logging();
                    // we call this because the container we just built might be promoted to be the prod one
                    self.map
                        .write()
                        .await
                        .read_db_and_build_updates(&self.build_queue, &self.github, &self.db)
                        .await
                        .ignore_logging();
                    // FIXME: this might produce and infinite loop if setup_as_standby always fails
                    // at least should chose a differnet container every time, to avoid starvation
                } else {
                    break;
                }
            }
        }
    }
}

impl BuildWorker {
    #[tracing::instrument]
    async fn get_container_to_build(&self) -> Option<Arc<Container>> {
        // this block helds this read guard
        let map = self.map.read().await;
        let queued_containers = map
            .iter_containers()
            .filter_map(|container| async {
                let status = container.status.read().await.clone();
                if let ContainerStatus::Queued { trigger_access } = status {
                    Some((container, trigger_access))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .await;

        let first_container_accessed = queued_containers
            .iter()
            .filter_map(|(container, trigger_access)| Some((container, trigger_access.clone()?)))
            .min_by_key(|(_, trigger_access)| trigger_access.clone())
            .map(|(container, _)| container);

        if first_container_accessed.is_some() {
            first_container_accessed.cloned()
        } else {
            // if no container is accessed, just return a random one
            queued_containers
                .choose(&mut rand::thread_rng())
                .map(|(container, _)| container.clone())
        }
    }
}
