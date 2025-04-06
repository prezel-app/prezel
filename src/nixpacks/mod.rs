use std::path::Path;

use anyhow::{bail, Result};
use nixpacks::nixpacks::{
    app::App,
    builder::{
        docker::{docker_image_builder::DockerImageBuilder, DockerBuilderOptions},
        ImageBuilder,
    },
    environment::Environment,
    logger::Logger,
    plan::{
        generator::{GeneratePlanOptions, NixpacksBuildPlanGenerator},
        PlanGenerator,
    },
};
use providers::get_providers;
use tokio::fs;

mod providers;

pub(crate) async fn create_docker_image_with_nixpacks(path: &Path, envs: Vec<&str>) -> Result<()> {
    let path_str = path.to_str().unwrap();
    let app = App::new(path_str)?;
    let environment = Environment::from_envs(envs)?;
    let orig_path = app.source.clone();

    let mut generator =
        NixpacksBuildPlanGenerator::new(get_providers(), GeneratePlanOptions::default());
    let (plan, app) = generator.generate_plan(&app, &environment)?;

    if let Ok(subdir) = app.source.strip_prefix(orig_path) {
        if subdir != std::path::Path::new("") {
            println!("Using subdirectory \"{}\"", subdir.to_str().unwrap());
        }
    }

    let logger = Logger::new();
    let builder = DockerImageBuilder::new(
        logger,
        DockerBuilderOptions {
            out_dir: Some(path_str.to_owned()),
            ..Default::default()
        },
    );

    let phase_count = plan.phases.clone().map_or(0, |phases| phases.len());
    if phase_count > 0 {
        let start = plan.start_phase.clone().unwrap_or_default();
        if start.cmd.is_none() {
            bail!("No start command could be found")
        }
    } else {
        bail!("unable to generate a build plan for this app.\nPlease check the documentation for supported languages: https://nixpacks.com")
    }

    builder
        .create_image(app.source.to_str().unwrap(), &plan, &environment)
        .await?;

    fs::rename(
        path.join(".nixpacks").join("Dockerfile"),
        path.join("Dockerfile"),
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod provider_tests {
    use std::{
        path::{Path, PathBuf},
        time::Duration,
    };

    use tempfile::TempDir;
    use tokio::process::Command;

    use super::create_docker_image_with_nixpacks;

    async fn exec(path: &Path, command: &str) -> std::process::Output {
        println!("---> Executing: {command}");
        let output = Command::new("sh")
            .current_dir(path)
            .arg("-c")
            .arg(command)
            .output()
            .await
            .unwrap();
        println!("{}", String::from_utf8(output.stdout.clone()).unwrap());
        println!("{}", String::from_utf8(output.stderr.clone()).unwrap());
        assert!(output.status.success());
        output
    }

    async fn run_container_and_fetch(dir: &Path, path: &str, name: &str, port: u16) -> String {
        exec(dir, &format!("docker stop {name} || true")).await;
        exec(dir, &format!("docker rm {name} || true")).await;
        exec(dir, &format!("docker image rm {name} || true")).await;
        exec(dir, &format!("docker build -t {name} .")).await;
        let run =
            format!("docker run -d -e PORT=80 -e HOST=0.0.0.0 -p {port}:80 --name {name} {name}");
        exec(dir, &run).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        let output = exec(dir, &format!("curl http://localhost:{port}{path}")).await;
        exec(dir, &format!("docker stop {name} && docker rm {name}")).await;
        String::from_utf8(output.stdout).unwrap()
    }

    #[tokio::test]
    async fn test_astro_basics() {
        let tempdir = TempDir::new().unwrap();
        let path = tempdir.as_ref();

        let command = "pnpm create astro --no-install --no-git -y . -- --template basics";
        exec(path, command).await;

        create_docker_image_with_nixpacks(path, vec![])
            .await
            .unwrap();

        let output = run_container_and_fetch(path, "/", "astro-basics", 8909).await;
        assert!(output.contains("To get started, open the"))
    }

    #[tokio::test]
    async fn test_astro_ssr() {
        let tempdir = TempDir::new().unwrap();
        let path = tempdir.as_ref();

        let command = format!("cp -r resources/astro-ssr/ {}", path.to_str().unwrap());
        exec(&PathBuf::from("."), &command).await;
        exec(path, "echo --------------").await;
        exec(path, "pwd").await;
        exec(path, "ls").await;

        panic!("");

        create_docker_image_with_nixpacks(path, vec!["HOST=0.0.0.0", "PORT=80"])
            .await
            .unwrap();

        // println!(
        //     "{}",
        //     tokio::fs::read_to_string(path.join("Dockerfile"))
        //         .await
        //         .unwrap()
        // );

        let output = run_container_and_fetch(path, "/prezel.json", "astro-ssr", 8908).await;
        assert_eq!(output, "prezel")
    }
}
