use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::{stream, Stream, StreamExt};

use crate::container::commit::CommitContainer;
use crate::container::ContainerStatus;
use crate::db::{BuildResult, Deployment as DbDeployment};
use crate::deployment_hooks::StatusHooks;
use crate::label::Label;
use crate::sqlite_db::ProdSqliteDb;
use crate::{
    container::Container,
    db::{Db, DeploymentWithProject},
    github::Github,
};

use super::worker::WorkerHandle;

#[derive(Debug)]
pub(crate) struct Deployment {
    pub(crate) branch: String,
    pub(crate) default_branch: bool,
    pub(crate) sha: String,
    pub(crate) id: i64,
    pub(crate) project: i64,
    pub(crate) url_id: String,
    pub(crate) timestamp: i64,
    pub(crate) created: i64,
    pub(crate) forced_prod: bool, // TODO: review if im using this
    pub(crate) app_container: Arc<Container>, // FIXME: try to remove Arc, only needed to make access to socket/public generic
                                              // pub(crate) prisma_container: Arc<Container>,
}

impl Deployment {
    // async fn get_all_containers(&self) -> impl Iterator<Item = &Container> {

    //     self.app_container.status.read().await.get_db_container();

    //     [self.app_container.as_ref(), self.prisma_container.as_ref()].into_iter()
    // }

    // TODO:  try to merge this with the one above?
    pub(crate) fn iter_arc_containers(&self) -> impl Stream<Item = Arc<Container>> + Send + '_ {
        // let db_container = self.app_container.status.read().await.get_db_container();
        // [Some(self.app_container.clone()), db_container]
        //     .into_iter()
        //     .filter_map(|container| container)

        let containers: [Pin<Box<dyn Future<Output = Option<Arc<Container>>> + Send>>; 2] = [
            Box::pin(async { Some(self.app_container.clone()) }),
            Box::pin(async {
                self.app_container
                    .status
                    .read()
                    .await
                    .get_db_setup()
                    .map(|setup| setup.container.clone())
            }),
        ];
        stream::iter(containers).filter_map(|container| container)
    }

    pub(crate) fn new(
        deployment: DeploymentWithProject,
        build_queue: WorkerHandle,
        github: Github,
        db: Db,
        project_db: &ProdSqliteDb,
    ) -> Self {
        let DeploymentWithProject {
            deployment,
            project,
        } = deployment;
        let default_branch = deployment.is_default_branch();
        let DbDeployment {
            sha,
            env,
            branch,
            id,
            url_id,
            timestamp,
            created,
            ..
        } = deployment;

        let env = env.into();
        let hooks = StatusHooks::new(db, id);

        let (inistial_status, build_result) = match deployment.result {
            Some(BuildResult::Failed) => (ContainerStatus::Failed, Some(BuildResult::Failed)),
            Some(BuildResult::Built) => (ContainerStatus::Built, Some(BuildResult::Built)),
            _ => (
                ContainerStatus::Queued {
                    trigger_access: None,
                },
                None,
            ),
        };

        let is_branch_deployment = !default_branch;
        let is_public = default_branch;
        let commit_container = CommitContainer::new(
            build_queue.clone(),
            hooks,
            github,
            project.repo_id.clone(),
            sha.clone(),
            id,
            env,
            project.root.clone(),
            is_branch_deployment,
            is_public,
            project_db,
            inistial_status,
            build_result,
        );
        // let prisma_container = PrismaContainer::new(db_file, build_queue);

        Self {
            branch,
            default_branch,
            sha,
            id,
            project: project.id,
            url_id,
            timestamp,
            created,
            forced_prod: project.prod_id.is_some_and(|prod_id| id == prod_id),
            app_container: commit_container.into(),
            // prisma_container: prisma_container.into(),
        }
    }

    pub(crate) fn get_app_hostname(&self, box_domain: &str, project_name: &str) -> String {
        Label::Deployment {
            project: project_name.to_string(),
            deployment: self.url_id.to_string(),
        }
        .format_hostname(box_domain)
    }

    pub(crate) fn get_prod_hostname(&self, box_domain: &str, project_name: &str) -> String {
        Label::Prod {
            project: project_name.to_string(),
        }
        .format_hostname(box_domain)
    }

    pub(crate) fn get_db_hostname(&self, box_domain: &str, project_name: &str) -> String {
        if self.default_branch {
            Label::ProdDb {
                project: project_name.to_string(),
            }
        } else {
            Label::BranchDb {
                project: project_name.to_string(),
                deployment: self.url_id.to_string(),
            }
        }
        .format_hostname(box_domain)
    }
}
