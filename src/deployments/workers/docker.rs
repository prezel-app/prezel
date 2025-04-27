use std::sync::Arc;

use futures::StreamExt;

use crate::{
    deployments::{manager::InstrumentedRwLock, map::DeploymentMap, worker::Worker},
    docker::{delete_container, list_managed_container_names, stop_container},
    utils::LogError,
};

#[derive(Debug)]
pub(crate) struct DockerWorker {
    pub(crate) map: Arc<InstrumentedRwLock<DeploymentMap>>,
}

impl Worker for DockerWorker {
    fn work(&self) -> impl std::future::Future<Output = ()> + Send {
        async {
            for container in list_managed_container_names().await.unwrap() {
                if !self.is_container_in_use(&container).await {
                    stop_container(&container).await.ignore_logging();
                    delete_container(&container).await.ignore_logging();
                }
            }
            // TODO: remove all the images that are not in use.
            // Careful don't remove an image that was just built but not wrote yet into an StandBy status
            // I can probably aquire the lock for the docker builder
        }
    }
}

impl DockerWorker {
    // TODO: make this O(N) instead of O(N²)
    #[tracing::instrument]
    async fn is_container_in_use(&self, name: &String) -> bool {
        let map = self.map.read().await;
        let mut containers = map.iter_containers();
        while let Some(container) = containers.next().await {
            if container.get_container_name().await.as_ref() == Some(name) {
                return true;
            }
        }
        false
    }
}
