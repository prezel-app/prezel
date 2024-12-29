use futures::{stream, StreamExt};

use crate::db::{Db, InsertDeployment, Project};

use super::{ApiDeployment, AppState};

#[tracing::instrument]
pub(super) async fn get_prod_deployment_id(db: &Db, project: &Project) -> Option<i64> {
    let latest_deployment = db
        .get_latest_successful_prod_deployment_for_project(project.id)
        .await;
    project.prod_id.or_else(|| Some(latest_deployment?.id))
}

#[tracing::instrument]
pub(super) async fn get_prod_deployment(
    AppState { db, manager, .. }: &AppState,
    project: i64,
) -> Option<ApiDeployment> {
    let box_domain = &manager.box_domain;
    let deployment = manager.get_prod_deployment(project).await?;
    let db_deployment = db.get_deployment_with_project(deployment.id).await?;
    let is_prod = true;
    Some(
        ApiDeployment::from(
            Some(deployment).as_deref(),
            &db_deployment,
            is_prod,
            box_domain,
            &manager,
        )
        .await,
    )
}

#[tracing::instrument]
pub(super) async fn get_all_deployments(
    AppState { db, manager, .. }: &AppState,
    project: i64,
) -> Vec<ApiDeployment> {
    let box_domain = &manager.box_domain;

    let db_deployments = db.get_deployments_with_project().await;
    let mut deployments: Vec<_> =
        stream::iter(db_deployments.filter(|deployment| deployment.deployment.project == project))
            .then(|db_deployment| async move {
                let deployment = manager.get_deployment(db_deployment.deployment.id).await;
                let is_prod = if let Some(deployment) = deployment.as_deref() {
                    let prod_url_id = manager.get_prod_url_id(project).await; // TODO: move this outside
                    Some(&deployment.url_id) == prod_url_id.as_ref()
                } else {
                    false
                };
                ApiDeployment::from(
                    deployment.as_deref(),
                    &db_deployment,
                    is_prod,
                    box_domain,
                    &manager,
                )
                .await
            })
            .collect()
            .await;
    deployments.sort_by_key(|deployment| -deployment.created);
    deployments
}

pub(crate) async fn clone_deployment(db: &Db, deployment_id: i64) -> Option<()> {
    let deployment = db.get_deployment(deployment_id).await?;
    let project = db.get_project(deployment.project).await?;

    let insert = InsertDeployment {
        env: project.env.clone(),
        sha: deployment.sha.clone(),
        branch: deployment.branch.clone(),
        default_branch: deployment.default_branch,
        timestamp: deployment.timestamp,
        project: deployment.project,
    };
    db.insert_deployment(insert).await;
    Some(())
}
