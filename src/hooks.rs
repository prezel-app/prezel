use std::{collections::HashMap, fmt};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use octocrab::{
    models::issues::Comment,
    params::checks::{CheckRunConclusion, CheckRunStatus},
};
use serde::{Deserialize, Serialize};

use crate::{
    conf::Conf,
    db::{nano_id::NanoId, BuildResult, Db},
    github::Github,
    provider,
    tokens::{decode_token, generate_token},
    utils::now,
};

#[async_trait]
pub(crate) trait DeploymentHooks: 'static + Send + Sync + fmt::Debug {
    async fn on_build_log(&self, output: &str, error: bool);
    async fn on_build_started(&self);
    async fn on_build_finished(&self);
    async fn on_build_failed(&self);
}

#[derive(Debug)]
pub(crate) struct NoopHooks;

#[async_trait]
impl DeploymentHooks for NoopHooks {
    async fn on_build_log(&self, _output: &str, _error: bool) {}
    async fn on_build_started(&self) {}
    async fn on_build_finished(&self) {}
    async fn on_build_failed(&self) {}
}

#[derive(Debug, Clone)]
pub(crate) struct StatusHooks {
    db: Db,
    id: NanoId,
    github: Github,
}

impl StatusHooks {
    pub(crate) fn new(deployment_id: NanoId, db: Db, github: Github) -> Self {
        Self {
            db,
            id: deployment_id,
            github,
        }
    }
}

// TODO: write also error status to db, and send updates to github!!
#[async_trait]
impl DeploymentHooks for StatusHooks {
    async fn on_build_log(&self, output: &str, error: bool) {
        let _ = self
            .db
            .insert_deployment_build_log(&self.id, output, error) // TODO: differentiate error logs
            .await;
    }

    async fn on_build_started(&self) {
        let _ = self.db.clear_deployment_build_logs(&self.id).await;
        let _ = self.db.update_deployment_build_start(&self.id, now()).await;
        let _ = self.db.reset_deployment_build_end(&self.id).await;
        self.update_github(Status::Building);
    }

    async fn on_build_finished(&self) {
        let _ = self.db.update_deployment_build_end(&self.id, now()).await;
        let _ = self
            .db
            .update_deployment_result(&self.id, BuildResult::Built) // FIXME: the db should maybe only have a flag error: bool
            .await;
        self.update_github(Status::Ready);
    }

    async fn on_build_failed(&self) {
        let _ = self.db.update_deployment_build_end(&self.id, now()).await;
        let _ = self
            .db
            .update_deployment_result(&self.id, BuildResult::Failed)
            .await;
        self.update_github(Status::Failed);
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
enum Status {
    Building,
    Ready,
    Failed,
}

impl From<Status> for CheckRunStatus {
    fn from(value: Status) -> Self {
        match value {
            Status::Building => Self::InProgress,
            Status::Ready => Self::Completed,
            Status::Failed => Self::Completed,
        }
    }
}

impl StatusHooks {
    // TODO: record updated time before the async code and check it to avoid overwriting a newer comment
    fn update_github(&self, status: Status) {
        let hooks = self.clone();
        tokio::spawn(async move {
            let bot = hooks.github.allocate_bot().await;
            let Conf {
                hostname,
                provider,
                secret,
                ..
            } = Conf::read_async().await; // FIXME: this should be async
            let deployment = hooks
                .db
                .get_deployment_with_project(&hooks.id)
                .await
                .unwrap()
                .unwrap();
            let repo_id = deployment.project.repo_id;
            let prs = hooks.github.get_open_pulls(repo_id).await.unwrap();
            for pr in prs {
                if deployment.branch == pr.head.ref_field {
                    let team = provider::get_team_name().await.unwrap();

                    let comment = bot
                        .read_pull_comment_with_prefix(repo_id, pr.number, "[prezel]: ey")
                        .await
                        .unwrap();

                    let project_name = &deployment.project.name;
                    let slug = &deployment.url_id;
                    let app_comment = GithubCommentApp {
                        status,
                        provider_url: format!("{provider}/{team}/{project_name}/{slug}"),
                        preview_url: deployment.get_app_base_url(&hostname),
                        updated: chrono::offset::Utc::now(),
                    };

                    if let Some(comment) = comment {
                        let updated_info =
                            if let Some(mut info) = get_comment_info(&comment, &secret) {
                                info.insert(project_name.clone(), app_comment);
                                info
                            } else {
                                HashMap::from([(project_name.clone(), app_comment)])
                            };
                        let content = create_comment(updated_info, &secret); // TODO: this
                        let _ = bot
                            .update_pull_comment(repo_id, pr.number, comment.id, &content)
                            .await;
                    } else {
                        let info = HashMap::from([(project_name.clone(), app_comment)]);
                        let content = create_comment(info, &secret);
                        let _ = bot.create_pull_comment(repo_id, pr.number, &content).await;
                    };

                    let (status, conclusion) = match status {
                        Status::Building => (CheckRunStatus::InProgress, None),
                        Status::Ready => {
                            (CheckRunStatus::Completed, Some(CheckRunConclusion::Success))
                        }
                        Status::Failed => {
                            (CheckRunStatus::Completed, Some(CheckRunConclusion::Failure))
                        }
                    };
                    let check_name = format!("Prezel - {project_name}");
                    let _ = bot
                        .upsert_pull_check(
                            repo_id,
                            &deployment.sha,
                            &check_name,
                            status,
                            conclusion,
                        )
                        .await;
                }
            }
        });
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct GithubCommentApp {
    status: Status,
    provider_url: String,
    preview_url: String,
    updated: DateTime<Utc>,
}

type GithubCommentInfo = HashMap<String, GithubCommentApp>;

fn get_comment_info(comment: &Comment, secret: &[u8]) -> Option<GithubCommentInfo> {
    let body = comment.body.as_ref()?;
    let header = body.split("\n").next()?;
    let jwt = header.split("[prezel]: ").last()?;
    decode_token(jwt, secret, false).ok()
}

fn create_comment(info: GithubCommentInfo, secret: &[u8]) -> String {
    let rows = info.iter().map(|(name, GithubCommentApp{status, provider_url, preview_url, updated})| {
        let formatted_status = match status {
            // Status::Queued => "⏳ Queued",
            Status::Building => "🔨 Building",
            Status::Ready => "✅ Ready",
            Status::Failed => "❌ Failed",
        };
        let updated = updated.format("%b %e, %Y %l:%M%P").to_string();
        format!("| **{name}** | {formatted_status} ([Inspect]({provider_url})) | [Visit Preview]({preview_url}) | {updated} |")
    });

    let table_content = rows.collect::<Vec<_>>().join("\n");

    let jwt = generate_token(info, secret);

    format!(
        "[prezel]: {jwt}
**The latest updates on your projects**. Learn more about [prezel.app ↗︎](https://prezel.app)

| Name | Status | Preview | Updated (UTC) |
| :--- | :----- | :------ | :------ |
{table_content}"
    )
}
