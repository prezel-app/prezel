use std::{collections::HashMap, ops::Deref, sync::Arc};

use futures::{stream, StreamExt, TryStreamExt};
use nano_id::{MaybeNanoId, NanoId};
use nanoid::nanoid;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePool, FromRow, Pool, Sqlite};
use tracing::info;
use utoipa::ToSchema;

use crate::{
    deployments::config::{from_opt_str, DeploymentConfig, FlatDeploymentConfig},
    label::Label,
    paths::get_instance_db_path,
    utils::{now, PlusHttps, LOWERCASE_PLUS_NUMBERS},
};

pub(crate) mod nano_id;

#[derive(sqlx::Type, Deserialize, PartialEq, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")] // TODO: have a string in InsertDeployment instead?
pub(crate) enum BuildResult {
    Built,
    Failed,
}

#[derive(Clone, Debug)]
struct PlainProject {
    pub(crate) id: NanoId,
    pub(crate) name: String,
    pub(crate) repo_id: i64,
    pub(crate) created: i64,
    pub(crate) root: String,
    pub(crate) prod_id: MaybeNanoId,
}

#[derive(FromRow, Debug)]
struct PlainDeployment {
    pub(crate) id: NanoId,
    pub(crate) slug: String,
    pub(crate) timestamp: i64,
    pub(crate) created: i64,
    pub(crate) sha: String,
    pub(crate) branch: String, // I might need to have here a list of prs somehow
    pub(crate) default_branch: i64,
    pub(crate) result: Option<String>,
    pub(crate) build_started: Option<i64>,
    pub(crate) build_finished: Option<i64>,
    pub(crate) project: NanoId,
    pub(crate) config_visibility: Option<String>,
    pub(crate) config_build_backend: Option<String>,
    pub(crate) config_dockerfile_path: Option<String>,
    pub(crate) deleted: Option<i64>, // ignored, only used for filtering in the SQL select
}

#[derive(Debug)]
pub(crate) struct Deployment {
    pub(crate) id: NanoId,
    pub(crate) url_id: String,
    pub(crate) timestamp: i64,
    pub(crate) created: i64,
    pub(crate) sha: String,
    pub(crate) branch: String, // I might need to have here a list of prs somehow
    pub(crate) default_branch: i64,
    pub(crate) result: Option<BuildResult>,
    pub(crate) build_started: Option<i64>,
    pub(crate) build_finished: Option<i64>,
    pub(crate) project: NanoId,
    pub(crate) config: DeploymentConfig,
    pub(crate) env: Vec<EnvVar>,
}

impl Deployment {
    pub(crate) fn is_default_branch(&self) -> bool {
        self.default_branch != 0
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, ToSchema)]
pub(crate) struct EditedEnvVar {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) edited: i64,
}

#[derive(Deserialize, Debug, ToSchema)]
pub(crate) struct EnvVar {
    pub(crate) name: String,
    pub(crate) value: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Project {
    pub(crate) id: NanoId,
    pub(crate) name: String,
    pub(crate) repo_id: i64,
    pub(crate) created: i64,
    pub(crate) env: Vec<EditedEnvVar>,
    pub(crate) root: String,
    pub(crate) prod_id: Option<NanoId>,
    pub(crate) custom_domains: Vec<String>,
}

#[derive(Deserialize, Debug, ToSchema)]
pub(crate) struct InsertProject {
    pub(crate) name: String,
    pub(crate) repo_id: i64,
    pub(crate) env: Vec<EnvVar>,
    pub(crate) root: String,
}

#[derive(Deserialize, Debug, ToSchema)]
pub(crate) struct UpdateProject {
    pub(crate) name: Option<String>,
    custom_domains: Option<Vec<String>>,
}

#[derive(FromRow)]
pub(crate) struct BuildLog {
    pub(crate) id: i64,
    pub(crate) content: String,
    pub(crate) timestamp: i64,
    pub(crate) error: i64,
    pub(crate) deployment: NanoId,
}

#[derive(Debug)]
pub(crate) struct DeploymentWithProject {
    pub(crate) deployment: Deployment,
    pub(crate) project: Arc<Project>,
}

impl DeploymentWithProject {
    pub(crate) fn get_app_base_url(&self, box_domain: &str) -> String {
        Label::Deployment {
            project: self.project.name.clone(),
            deployment: self.url_id.to_string(),
        }
        .format_hostname(box_domain)
        .plus_https()
    }

    pub(crate) fn get_prod_base_url(&self, box_domain: &str) -> String {
        Label::Prod {
            project: self.project.name.clone(),
        }
        .format_hostname(box_domain)
        .plus_https()
    }

    // FIXME: this choice between prod or branch is disconnected from similar choices in other parts
    // such as in the api where we get the token from the prod db or the branch db
    // or in commit.rs where we do the same
    pub(crate) fn get_libsql_url(&self, box_domain: &str) -> String {
        if self.default_branch == 1 {
            Label::ProdDb {
                project: self.project.id.clone(),
            }
            .format_hostname(box_domain)
            .plus_https()
        } else {
            Label::BranchDb {
                project: self.project.id.clone(),
                deployment: self.url_id.clone(),
            }
            .format_hostname(box_domain)
            .plus_https()
        }
    }
}

impl Deref for DeploymentWithProject {
    type Target = Deployment;

    fn deref(&self) -> &Self::Target {
        &self.deployment
    }
}

#[derive(Debug)]
pub(crate) struct InsertDeployment {
    pub(crate) env: Vec<EditedEnvVar>,
    pub(crate) sha: String,
    pub(crate) timestamp: i64,
    pub(crate) branch: String,
    pub(crate) default_branch: i64,
    pub(crate) project: NanoId,
    pub(crate) result: Option<BuildResult>,
}

fn create_deployment_url_id() -> String {
    nanoid!(10, &LOWERCASE_PLUS_NUMBERS)
}

#[derive(Clone, Debug)]
pub(crate) struct Db {
    conn: Pool<Sqlite>, // TODO: put this in a module with db.rs and make this provate
}

impl Db {
    #[tracing::instrument]
    pub(crate) async fn setup() -> anyhow::Result<Self> {
        let db_path = get_instance_db_path();
        let db_path_str = db_path.to_str().expect("Path to DB coud not be generated");

        if !db_path.exists() {
            std::fs::File::create_new(&db_path)?;
        }

        let conn = SqlitePool::connect(db_path_str).await?;

        sqlx::migrate!("./migrations").run(&conn).await?;

        info!("db setup at {}", db_path.canonicalize()?.display());

        Ok(Self { conn })
    }

    // TODO: try to make the manager have access only to the read methods in here
    #[tracing::instrument]
    pub(crate) async fn get_project(&self, id: &NanoId) -> anyhow::Result<Option<Project>> {
        let query = sqlx::query_as!(
            PlainProject,
            "select * from projects where projects.id = ?",
            id
        );
        let project = query.fetch_optional(&self.conn).await?;
        self.append_extra_project_info_to_opt(project).await
    }

    #[tracing::instrument]
    pub(crate) async fn get_project_by_name(&self, name: &str) -> anyhow::Result<Option<Project>> {
        let query = sqlx::query_as!(
            PlainProject,
            "select * from projects where projects.name = ?",
            name
        );
        let project = query.fetch_optional(&self.conn).await?;
        self.append_extra_project_info_to_opt(project).await
    }

    async fn append_extra_project_info_to_opt(
        &self,
        project: Option<PlainProject>,
    ) -> anyhow::Result<Option<Project>> {
        if let Some(project) = project {
            Ok(Some(self.append_extra_project_info(project).await?))
        } else {
            Ok(None)
        }
    }

    #[tracing::instrument]
    pub(crate) async fn get_projects(&self) -> anyhow::Result<Vec<Project>> {
        let query = sqlx::query_as!(PlainProject, "select * from projects");
        let projects = query.fetch_all(&self.conn).await?;
        stream::iter(projects)
            .then(|project| self.append_extra_project_info(project))
            .try_collect()
            .await
    }

    #[tracing::instrument]
    async fn append_extra_project_info(&self, project: PlainProject) -> anyhow::Result<Project> {
        let custom_domains = sqlx::query!("select * from domains where project = ?", project.id)
            .fetch_all(&self.conn)
            .await?
            .into_iter()
            .map(|record| record.domain)
            .collect();
        let query = sqlx::query_as!(
            EditedEnvVar,
            "select name, value, edited from env where project = ?",
            project.id
        );
        let env = query.fetch_all(&self.conn).await?;

        Ok(Project {
            id: project.id,
            name: project.name,
            repo_id: project.repo_id,
            created: project.created,
            env,
            root: project.root,
            prod_id: project.prod_id.0,
            custom_domains,
        })
    }

    #[tracing::instrument]
    pub(crate) async fn insert_project(
        &self,
        InsertProject {
            name,
            repo_id,
            env,
            root,
        }: InsertProject,
    ) -> anyhow::Result<()> {
        let id = NanoId::random();
        let created = now();
        let query = sqlx::query!(
            "insert into projects (id, name, repo_id, created, root) values (?, ?, ?, ?, ?)",
            id,
            name,
            repo_id,
            created,
            root
        );

        let mut tx = self.conn.begin().await?;
        query.execute(&mut *tx).await?;
        let edited = now();
        for env in env {
            let query = sqlx::query!(
                "insert into env (name, value, edited, project) values (?, ?, ?, ?)",
                env.name,
                env.value,
                edited,
                id,
            );
            query.execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn update_project(
        &self,
        id: &NanoId,
        UpdateProject {
            name,
            custom_domains,
        }: UpdateProject,
    ) -> anyhow::Result<()> {
        if let Some(name) = name {
            let query = sqlx::query!("update projects set name = ? where id = ?", name, id);
            query.execute(&self.conn).await?;
        }

        if let Some(custom_domains) = custom_domains {
            let mut tx = self.conn.begin().await?;
            let query = sqlx::query!("delete from domains WHERE project = ?", id);
            query.execute(&mut *tx).await?;
            for domain in custom_domains {
                let query = sqlx::query!(
                    "insert into domains (domain, project) values (?, ?)",
                    domain,
                    id
                );
                query.execute(&mut *tx).await?;
            }
            tx.commit().await?;
        }

        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn delete_project(&self, id: &NanoId) -> anyhow::Result<()> {
        let query = sqlx::query!("delete from projects where id = ?", id);
        query.execute(&self.conn).await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn upsert_env(
        &self,
        project: &NanoId,
        name: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        let edited = now();
        let query = sqlx::query!(
            "insert into env (project, name, value, edited) values (?, ?, ?, ?) on conflict (name, project) do update set value=?, edited=?",
            project,
            name,
            value,
            edited,
            value,
            edited,
        );
        query.execute(&self.conn).await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn delete_env(&self, project: &NanoId, name: &str) -> anyhow::Result<()> {
        let query = sqlx::query!(
            "delete from env where project = ? and name = ?",
            project,
            name
        );
        query.execute(&self.conn).await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn get_deployment(
        &self,
        deployment: &NanoId,
    ) -> anyhow::Result<Option<Deployment>> {
        let plain_deployment = sqlx::query_as!(
            PlainDeployment,
            r#"select * from deployments where id = ? and deleted is null"#,
            deployment
        )
        .fetch_optional(&self.conn)
        .await?;

        if let Some(plain_deployment) = plain_deployment {
            let deployment = self.append_extra_deployment_info(plain_deployment).await?;
            Ok(Some(deployment))
        } else {
            Ok(None)
        }
    }

    // TODO: just return stream here?
    #[tracing::instrument]
    pub(crate) async fn get_deployments(&self) -> anyhow::Result<Vec<Deployment>> {
        let deployments = sqlx::query_as!(
            PlainDeployment,
            r#"select * from deployments where deleted is null"#
        )
        .fetch_all(&self.conn)
        .await?;
        stream::iter(deployments)
            .then(|deployment| self.append_extra_deployment_info(deployment))
            .try_collect()
            .await
    }

    #[tracing::instrument]
    async fn append_extra_deployment_info(
        &self,
        deployment: PlainDeployment,
    ) -> anyhow::Result<Deployment> {
        let env = sqlx::query_as!(
            EnvVar,
            "select name, value from deployment_env where deployment = ?",
            deployment.id
        )
        .fetch_all(&self.conn)
        .await?;
        let config = FlatDeploymentConfig {
            visibility: deployment.config_visibility,
            backend: deployment.config_build_backend,
            dockerfile_path: deployment.config_dockerfile_path,
        }
        .try_into()?;
        Ok(Deployment {
            id: deployment.id,
            url_id: deployment.slug,
            timestamp: deployment.timestamp,
            created: deployment.created,
            sha: deployment.sha,
            branch: deployment.branch,
            default_branch: deployment.default_branch,
            result: from_opt_str(deployment.result)?,
            build_started: deployment.build_started,
            build_finished: deployment.build_finished,
            project: deployment.project,
            config,
            env,
        })
    }

    #[tracing::instrument]
    pub(crate) async fn delete_deployment(&self, id: &NanoId) -> anyhow::Result<()> {
        let query = sqlx::query!("update deployments set deleted = 1 where id = ?", id);
        query.execute(&self.conn).await?;
        Ok(())
    }

    // TODO: implement this using SQL
    #[tracing::instrument]
    pub(crate) async fn get_latest_successful_prod_deployment_for_project(
        &self,
        project: &NanoId,
    ) -> anyhow::Result<Option<Deployment>> {
        let mut deployments: Vec<_> = self
            .get_deployments()
            .await?
            .into_iter()
            .filter(|deployment| &deployment.project == project && deployment.is_default_branch())
            .filter(|deployment| deployment.result != Some(BuildResult::Failed))
            .collect();
        deployments.sort_by_key(|deployment| deployment.timestamp);
        Ok(deployments.pop())
    }

    #[tracing::instrument]
    pub(crate) async fn get_deployment_with_project(
        &self,
        deployment: &NanoId,
    ) -> anyhow::Result<Option<DeploymentWithProject>> {
        let deployment = self.get_deployment(deployment).await?;
        if let Some(deployment) = deployment {
            let project = self.get_project(&deployment.project).await?;
            let deployment_with_project = project.map(|project| DeploymentWithProject {
                project: project.into(),
                deployment,
            });
            Ok(deployment_with_project) // FIXME: return error if project does not exist????????????
        } else {
            Ok(None)
        }
    }

    #[tracing::instrument]
    pub(crate) async fn get_deployments_with_project(
        &self,
    ) -> anyhow::Result<impl Iterator<Item = DeploymentWithProject>> {
        let project_iter = self.get_projects().await?.into_iter();
        let projects: HashMap<_, Arc<_>> = project_iter
            .map(|project| (project.id.clone(), project.into()))
            .collect();
        Ok(self
            .get_deployments()
            .await?
            .into_iter()
            .filter_map(move |deployment| {
                Some(DeploymentWithProject {
                    project: projects.get(&deployment.project)?.clone(),
                    deployment,
                })
            }))
    }

    #[tracing::instrument]
    pub(crate) async fn insert_deployment(
        &self,
        deployment: InsertDeployment,
        config: FlatDeploymentConfig,
    ) -> anyhow::Result<NanoId> {
        let created = now();
        let id = NanoId::random();
        let url_id = create_deployment_url_id();
        let insert_query = sqlx::query!(
            "insert into deployments (id, slug, timestamp, created, sha, branch, default_branch, project, result, config_visibility, config_build_backend, config_dockerfile_path) values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            url_id,
            deployment.timestamp,
            created,
            deployment.sha,
            deployment.branch,
            deployment.default_branch,
            deployment.project,
            deployment.result,
            config.visibility,
            config.backend,
            config.dockerfile_path,
        );

        let mut tx = self.conn.begin().await?;
        insert_query.execute(&mut *tx).await?;
        for var in deployment.env {
            let var_insert = sqlx::query!(
                "insert into deployment_env (name, value, deployment) values (?, ?, ?)",
                var.name,
                var.value,
                id,
            );
            var_insert.execute(&mut *tx).await?;
        }
        tx.commit().await?;

        Ok(id)
    }

    #[tracing::instrument]
    pub(crate) async fn update_deployment_result(
        &self,
        id: &NanoId,
        status: BuildResult,
    ) -> anyhow::Result<()> {
        let query = sqlx::query!("update deployments set result = ? where id = ?", status, id);
        query.execute(&self.conn).await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn update_deployment_build_start(
        &self,
        id: &NanoId,
        build_started: i64,
    ) -> anyhow::Result<()> {
        let query = sqlx::query!(
            "update deployments set build_started = ? where id = ?",
            build_started,
            id
        );
        query.execute(&self.conn).await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn update_deployment_build_end(
        &self,
        id: &NanoId,
        build_finished: i64,
    ) -> anyhow::Result<()> {
        let query = sqlx::query!(
            "update deployments set build_finished = ? where id = ?",
            build_finished,
            id
        );
        query.execute(&self.conn).await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn reset_deployment_build_end(&self, id: &NanoId) -> anyhow::Result<()> {
        let query = sqlx::query!(
            "update deployments set build_finished = NULL where id = ?",
            id
        );
        query.execute(&self.conn).await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn get_deployment_build_logs(
        &self,
        deployment: &NanoId,
    ) -> anyhow::Result<Vec<BuildLog>> {
        let query = sqlx::query_as!(
            BuildLog,
            r#"select * from build where build.deployment = ?"#,
            deployment
        );
        Ok(query.fetch_all(&self.conn).await?)
    }

    #[tracing::instrument]
    pub(crate) async fn insert_deployment_build_log(
        &self,
        deployment: &NanoId,
        content: &str,
        error: bool,
    ) -> anyhow::Result<()> {
        let time = now();
        let error = error as i64;
        let query = sqlx::query!(
            "insert into build (timestamp, content, error, deployment) values (?, ?, ?, ?)",
            time,
            content,
            error,
            deployment
        );
        query.execute(&self.conn).await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn clear_deployment_build_logs(
        &self,
        deployment: &NanoId,
    ) -> anyhow::Result<()> {
        let query = sqlx::query!("delete from build where build.deployment = ?", deployment);
        query.execute(&self.conn).await?;
        Ok(())
    }

    #[tracing::instrument]
    pub(crate) async fn hash_exists_for_project(
        &self,
        sha: &str,
        project: &NanoId,
    ) -> anyhow::Result<bool> {
        let query = sqlx::query!(
            "select id from deployments where deployments.sha=? and deployments.project=?",
            sha,
            project
        );
        Ok(query.fetch_optional(&self.conn).await?.is_some())
    }
}
