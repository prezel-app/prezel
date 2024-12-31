use anyhow::ensure;

/// The prefix of the hostname that refers to a resource of a particular app hosted in the server
#[derive(Debug)]
pub(crate) enum Label {
    Prod { project: String },
    ProdDb { project: String },
    Deployment { project: String, deployment: String },
    BranchDb { project: String, deployment: String },
}

impl Label {
    pub(crate) fn format_hostname(&self, box_domain: &str) -> String {
        match self {
            Label::Prod { project } => format!("{project}.{box_domain}"),
            Label::ProdDb { project } => format!("{project}-db.{box_domain}"),
            Label::Deployment {
                project,
                deployment,
            } => format!("{project}-{deployment}.{box_domain}"),
            Label::BranchDb {
                project,
                deployment,
            } => format!("{project}-{deployment}-db.{box_domain}"),
        }
    }

    pub(crate) fn strip_from_domain(hostname: &str, box_domain: &str) -> anyhow::Result<Vec<Self>> {
        let label_with_dot = hostname.strip_suffix(box_domain).ok_or(anyhow::Error::msg(
            "invalid hostname not ending with the box domain",
        ))?;
        // FIXME: double check len > 0 ?
        let label = &label_with_dot[..label_with_dot.len() - 1];
        ensure!(
            label.find(".").is_none(),
            "invalid label, more dots than expected"
        );
        Ok(parse_label(label))
    }
}

fn parse_label(label: &str) -> Vec<Label> {
    let deployment_label = match label.split("-").collect::<Vec<_>>().as_slice() {
        [project @ .., deployment, "db"] => Some(Label::BranchDb {
            project: project.join("-"),
            deployment: deployment.to_string(),
        }),
        [project @ .., deployment] => Some(Label::Deployment {
            project: project.join("-"),
            deployment: deployment.to_string(),
        }),
        _ => None,
    };

    let prod_db_label = match label.split("-").collect::<Vec<_>>().as_slice() {
        [project @ .., "db"] => Some(Label::ProdDb {
            project: project.join("-"),
        }),
        _ => None,
    };

    let prod_label = Label::Prod {
        project: label.to_owned(),
    };

    [Some(prod_label), prod_db_label, deployment_label]
        .into_iter()
        .filter_map(|label| label)
        .collect()
}
