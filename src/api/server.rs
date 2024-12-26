use std::{error::Error, net::Ipv4Addr};

use actix_cors::Cors;
use actix_web::{middleware::Logger, web::Data, App, HttpServer};
use tracing::info;
use utoipa::{
    openapi::{
        security::{ApiKey, ApiKeyValue, SecurityScheme},
        OpenApi as OpenApiStruct, Server,
    },
    OpenApi,
};
use utoipa_rapidoc::RapiDoc;

use crate::{
    api::{configure_service, security::API_KEY_NAME, AppState, API_PORT},
    db::Db,
    deployments::manager::Manager,
    github::Github,
};

use super::ApiDoc;

pub(crate) fn get_open_api() -> OpenApiStruct {
    let mut openapi = ApiDoc::openapi();
    openapi.components.as_mut().unwrap().add_security_scheme(
        "api_key",
        SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new(API_KEY_NAME))),
    );
    openapi
}

pub(crate) async fn run_api_server(
    manager: Manager,
    db: Db,
    github: Github,
    api_hostname: &str,
    coordinator_hostname: String,
) -> Result<(), impl Error> {
    let state = AppState {
        db,
        manager: manager.clone(),
        github,
    };

    let base_url = format!("https://{api_hostname}");
    let localhost = "http://127.0.0.1:5045";

    let mut openapi = get_open_api();
    openapi.servers = Some(vec![Server::new(&base_url), Server::new(localhost)]);

    info!("Prezel API service listening at {base_url}");
    info!("Docs available at {base_url}/docs");
    HttpServer::new(move || {
        let cors = Cors::permissive();
        // .allowed_origin(&coordinator_hostname) // TODO: review if I should enable this
        // .allowed_origin("https://libsqlstudio.com")
        // .allowed_origin(localhost)
        // .allow_any_method()
        // .allow_any_header()
        // .max_age(3600);
        // .max_age(1);
        // This factory closure is called on each worker thread independently.
        App::new()
            .wrap(Logger::default())
            .wrap(cors)
            .configure(configure_service(Data::new(state.clone())))
            // .service(web::scope("/api").configure(configure_service(Data::new(state.clone()))))
            .service(RapiDoc::with_openapi("/openapi.json", openapi.clone()).path("/docs"))
    })
    .workers(1)
    .bind((Ipv4Addr::LOCALHOST, API_PORT))?
    .run()
    .await
}
