use std::sync::Arc;

use futures::StreamExt;

use crate::{
    deployments::{manager::InstrumentedRwLock, map::DeploymentMap, worker::Worker},
    docker::{delete_container, list_managed_container_ids, stop_container},
};

#[derive(Debug)]
pub(crate) struct DockerWorker {
    pub(crate) map: Arc<InstrumentedRwLock<DeploymentMap>>,
}

impl Worker for DockerWorker {
    fn work(&self) -> impl std::future::Future<Output = ()> + Send {
        async {
            // Careful, don't remove a container that was just started but not wrote yet into a Ready status
            for container in list_managed_container_ids().await.unwrap() {
                if !self.is_container_in_use(&container).await {
                    // stop_container(&container).await;
                    // delete_container(&container).await;
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
    async fn is_container_in_use(&self, id: &String) -> bool {
        let map = self.map.read().await;
        let mut containers = map.iter_containers();
        while let Some(container) = containers.next().await {
            if container.get_container_id().await.as_ref() == Some(id) {
                return true;
            }
        }
        false
    }
}
