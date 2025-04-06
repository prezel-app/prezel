use anyhow::{bail, Result};
use nixpacks::{
    nixpacks::{
        app::App,
        environment::Environment,
        plan::{
            phase::{Phase, StartPhase},
            BuildPlan,
        },
    },
    providers::{
        node::{NodeProvider, PackageJson},
        Provider,
    },
};

pub(crate) struct AstroProvider;

// this still needs the user to use astro build --remote for astrodb projects, not the end of the world
impl Provider for AstroProvider {
    fn name(&self) -> &str {
        "astro"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(app.includes_file("astro.config.mjs"))
    }

    fn get_build_plan(
        &self,
        app: &App,
        environment: &Environment,
    ) -> anyhow::Result<Option<BuildPlan>> {
        let node = NodeProvider {};
        let plan = node.get_build_plan(app, environment)?;

        if let Some(mut plan) = plan {
            if plan.start_phase.is_none() {
                let package_json: PackageJson = app.read_json("package.json").unwrap_or_default();
                let start = if let Some(server_command) = get_server_command(&package_json) {
                    // TODO: this branch is disabled because get_server_command always returns None, remove?
                    // we assume SSR
                    // FIXME: this pot-install phase is missing the env variables !!!!!!!!!
                    let node_version = get_node_version(&package_json, &app, &environment);
                    // let phase_name = format!("post-install");
                    let mut post_install = Phase::new("post-install");
                    post_install.depends_on_phase("build");
                    let install_command = NodeProvider::get_install_command(app);
                    let omit_dev = install_command
                        .map(
                            |cmd| match NodeProvider::get_package_manager(app).as_str() {
                                "npm" => Ok(format!("{cmd} --omit=dev")),
                                "pnpm" => Ok(format!("{cmd} --prod")),
                                "yarn" => Ok(format!("{cmd} --prod")),
                                "bun" => Ok(format!("{cmd} --production")),
                                _ => bail!("unsupported package manager"),
                            },
                        )
                        .transpose()?;
                    post_install.add_cmd("rm -fr node_modules");
                    if let Some(omit_dev) = omit_dev {
                        post_install.add_cmd(omit_dev);
                    }
                    plan.add_phase(post_install);

                    let mut start = StartPhase::new(server_command);
                    start.add_file_dependency("./node_modules");
                    start.add_file_dependency("./dist");
                    // can start.run_image be some already because of some user config???????
                    start.run_image = Some(format!(
                        "node:{node_version}-alpine
ENTRYPOINT [\"/bin/sh\", \"-l\", \"-c\"]
ENV HOST=0.0.0.0
ENV PORT=80
RUN echo \\"
                    )); // FIXME: Im not getting any env variables in here???
                    start
                } else {
                    // we assume static site
                    let mut start = StartPhase::new("rm -fr /usr/local/apache2/htdocs && mv /app/dist /usr/local/apache2/htdocs && /usr/local/apache2/bin/httpd -DFOREGROUND");
                    start.add_file_dependency("./dist");
                    // can start.run_image be some already because of some user config???????
                    start.run_image = Some(
                    "httpd:2.4.63-alpine\nENTRYPOINT [\"/bin/sh\", \"-l\", \"-c\"]\nRUN echo \\"
                        .to_owned(),
                );
                    start
                };
                plan.set_start_phase(start);
            }
            Ok(Some(plan))
        } else {
            Ok(None)
        }
    }
}

fn get_server_command(package_json: &PackageJson) -> Option<String> {
    // package_json.scripts.clone()?.get("server").cloned()
    None
}

const DEFAULT_NODE_VERSION: &str = "18";

fn get_node_version(package_json: &PackageJson, app: &App, environment: &Environment) -> String {
    let env_node_version = environment.get_config_variable("NODE_VERSION");
    let pkg_node_version = package_json
        .engines
        .clone()
        .and_then(|engines| engines.get("node").cloned());
    let nvmrc_node_version = if app.includes_file(".nvmrc") {
        app.read_file(".nvmrc")
            .ok()
            .map(|nvmrc| nvmrc.trim().replace('v', ""))
    } else {
        None
    };
    let node_version = env_node_version.or(pkg_node_version).or(nvmrc_node_version);
    match node_version.as_deref() {
        // If *, any version will work, use default
        None | Some("*") => DEFAULT_NODE_VERSION.to_owned(),
        Some(version) => version.to_owned(),
    }
}
