use actix_web::{get, post, web::Json, HttpResponse, Responder};
use anyhow::ensure;
use tracing::error;

use crate::{
    api::bearer::{AdminRole, AnyRole},
    docker::{
        create_container_with_explicit_binds, generate_unmanaged_container_name, get_image,
        get_prezel_image_version, pull_image, run_container,
    },
};

/// Get system version
#[utoipa::path(
    responses(
        (status = 200, description = "Fetched system version", body = String),
        (status = 500, description = "A problem was found when trying to read the version"),
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[get("/version")]
async fn get_version(_auth: AnyRole) -> impl Responder {
    match get_prezel_image_version().await {
        Some(version) => HttpResponse::Ok().json(version),
        None => HttpResponse::InternalServerError().json("internal server error"),
    }
}

/// Update version
#[utoipa::path(
    request_body = String,
    responses(
        (status = 200, description = "Version update was initiated"),
        (status = 500, description = "A problem was found when trying to update the version"),
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[post("/version")]
async fn update_version(_auth: AdminRole, version: Json<String>) -> impl Responder {
    match run_update_container(&version.0).await {
        Ok(()) => HttpResponse::Ok().finish(),
        Err(error) => {
            error!("{error}");
            HttpResponse::InternalServerError().json("internal server error")
        }
    }
}

async fn run_update_container(version: &str) -> anyhow::Result<()> {
    let name = format!("prezel/prezel:{version}");
    let image = get_image(&name).await;
    if image.is_none() {
        pull_image(&name).await;
        let image = get_image(&name).await;
        ensure!(image.is_some());
    }

    let create_template = r#"&& curl --unix-socket /var/run/docker.sock -H "Content-Type: application/json" -X POST \
        -d '{
              "Image": "$IMAGE",
              "ExposedPorts": {
                "80/tcp": {},
                "443/tcp": {}
              },
              "HostConfig": {
                "PortBindings": {
                  "80/tcp": [{"HostPort": "80"}],
                  "443/tcp": [{"HostPort": "443"}]
                },
                "Binds": [
                  "/opt/prezel:/opt/prezel",
                  "/var/run/docker.sock:/var/run/docker.sock"
                ],
                "NetworkMode": "prezel",
                "RestartPolicy": {
                  "Name": "always"
                }
              }
            }' \
        http://localhost/containers/create?name=prezel"#;
    let create = create_template.replace("$IMAGE", &name);
    let command = [
        "curl --unix-socket /var/run/docker.sock -X POST http://localhost/containers/prezel/stop",
        "&& curl --unix-socket /var/run/docker.sock -X DELETE http://localhost/containers/prezel",
        &create,
        "&& curl --unix-socket /var/run/docker.sock -X POST http://localhost/containers/prezel/start",
    ]
    .join(" ");

    let image = "alpine/curl:8.12.1".to_owned();
    pull_image(&image).await;
    let binds = vec!["/var/run/docker.sock:/var/run/docker.sock".to_owned()];
    let name = generate_unmanaged_container_name();
    let container =
        create_container_with_explicit_binds(name, image, Default::default(), binds, Some(command))
            .await?;
    Ok(run_container(&container).await?)
}
