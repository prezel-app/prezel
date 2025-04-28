use futures::{stream, StreamExt};

use crate::{
    db::{nano_id::NanoId, Db, InsertDeployment, Project},
    sqlite_db::DbAccess,
};

use super::{ApiDeployment, AppState};

#[tracing::instrument]
pub(super) async fn get_prod_deployment_id(db: &Db, project: &Project) -> Option<NanoId> {
    let latest_deployment = db
        .get_latest_successful_prod_deployment_for_project(&project.id)
        .await
        .unwrap();
    project
        .prod_id
        .clone()
        .or_else(|| Some(latest_deployment?.id))
}

#[tracing::instrument]
pub(super) async fn get_prod_deployment(
    AppState { db, manager, .. }: &AppState,
    project: &NanoId,
    access: DbAccess,
) -> Option<ApiDeployment> {
    let box_domain = &manager.box_domain;
    let deployment = manager.get_prod_deployment(project).await?;
    let db_deployment = db
        .get_deployment_with_project(&deployment.id)
        .await
        .unwrap()?;
    let is_prod = true;
    Some(
        ApiDeployment::from(
            Some(deployment).as_ref(),
            &db_deployment,
            is_prod,
            box_domain,
            &manager,
            access,
        )
        .await,
    )
}

#[tracing::instrument]
pub(super) async fn get_all_deployments(
    AppState { db, manager, .. }: &AppState,
    project: &NanoId,
    access: DbAccess,
) -> Vec<ApiDeployment> {
    let box_domain = &manager.box_domain;

    let db_deployments = db.get_deployments_with_project().await.unwrap();
    let mut deployments: Vec<_> =
        stream::iter(db_deployments.filter(|deployment| &deployment.deployment.project == project))
            .then(|db_deployment| async move {
                let deployment = manager.get_deployment(&db_deployment.deployment.id).await;
                let is_prod = if let Some(deployment) = &deployment {
                    let prod_url_id = manager.get_prod_url_id(project).await; // TODO: move this outside
                    Some(&deployment.url_id) == prod_url_id.as_ref()
                } else {
                    false
                };
                ApiDeployment::from(
                    deployment.as_ref(),
                    &db_deployment,
                    is_prod,
                    box_domain,
                    &manager,
                    access,
                )
                .await
            })
            .collect()
            .await;
    deployments.sort_by_key(|deployment| -deployment.created);
    deployments
}

pub(super) async fn clone_deployment(db: &Db, deployment_id: &NanoId) {
    let deployment = db.get_deployment(deployment_id).await.unwrap().unwrap();
    let project = db.get_project(&deployment.project).await.unwrap().unwrap();
    let insert = InsertDeployment {
        env: project.env.clone(),
        sha: deployment.sha.clone(),
        branch: deployment.branch.clone(),
        default_branch: deployment.default_branch,
        timestamp: deployment.timestamp,
        project: deployment.project,
        result: None,
    };
    db.insert_deployment(insert, deployment.config.into())
        .await
        .unwrap();
}

pub(super) fn is_app_name_valid(name: &str) -> bool {
    name != "api" && !name.contains("--")
}
