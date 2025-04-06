// extern crate dotenv;

// use openapi::apis::apps_api::{create_project, get_projects};
// use openapi::apis::configuration::Configuration;
// use openapi::apis::system_api::get_system_version;
// use openapi::models::InsertProject;
// use std::path::PathBuf;
// use std::process::Command;
// use std::sync::{LazyLock, RwLock};
// use std::time::{Duration, SystemTime, UNIX_EPOCH};
// use std::{env, fs};

// use actix_web::dev::Server;
// use actix_web::web::{Json, Path};
// use actix_web::{get, post, App, HttpResponse, HttpServer, Responder};
// use aws_sdk_route53::types::{
//     Change, ChangeAction, ChangeBatch, ChangeStatus, ResourceRecord, ResourceRecordSet, RrType,
// };
// use dotenv::dotenv;
// use jsonwebtoken::{encode, EncodingKey, Header};
// use octocrab::models::{AppId, Repository};
// use secrecy::ExposeSecret;
// use serde::{Deserialize, Serialize};

// const INSTANCE_HOSTNAME: &str = "ci.dev.prezel.app";

// const SECRET: &str = "secret";

// #[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
// pub(crate) enum Role {
//     admin,
//     user,
// }

// #[derive(Serialize, Deserialize, Debug, Clone)]
// pub(crate) struct TokenClaims {
//     pub(crate) exp: u64,
//     pub(crate) role: Role,
// }

// // TODO: remove this?
// fn generate_token(role: Role) -> String {
//     let exp = SystemTime::now()
//         .duration_since(UNIX_EPOCH)
//         .unwrap()
//         .as_secs()
//         + 3600;
//     encode(
//         &Header::default(),
//         &TokenClaims { role, exp },
//         &EncodingKey::from_secret(SECRET.as_ref()),
//     )
//     .expect("Failed to encode claims")
// }

// #[derive(Serialize)]
// pub(crate) struct Conf {
//     pub(crate) secret: String,
//     pub(crate) hostname: String,
//     pub(crate) provider: String,
// }

// fn get_prezel_home() -> String {
//     let home = env::var("HOME").unwrap();
//     format!("{home}/prezel-test")
// }

// #[tokio::test]
// async fn test_startup_deadlocks() {
//     dotenv().ok();

//     let command = "curl https://ipinfo.io/ip";
//     let output = Command::new("sh").arg("-c").arg(command).output().unwrap();
//     assert!(output.status.success());
//     let ip = String::from_utf8(output.stdout).unwrap();

//     // conf
//     fs::create_dir_all(get_prezel_home()).unwrap();
//     let conf = Conf {
//         secret: SECRET.to_owned(),
//         hostname: INSTANCE_HOSTNAME.to_owned(),
//         provider: format!("http://{ip}:3000"),
//     };
//     let conf_path = PathBuf::from(get_prezel_home()).join("config.json");
//     fs::write(conf_path, serde_json::to_string(&conf).unwrap()).unwrap();

//     // create DNS A record and save change id:
//     create_a_record(INSTANCE_HOSTNAME, &ip).await;

//     let server = run_api_server().unwrap();
//     tokio::spawn(server);

//     run_prezel_container();

//     loop {
//         println!("checking if for A record is ready");
//         if is_change_ready(A_CHANGE_ID.read().unwrap().clone()).await {
//             break;
//         }
//         println!("waiting for A record to be ready");
//         tokio::time::sleep(Duration::from_secs(5)).await;
//     }

//     for _ in 0..10 {
//         println!("checking if instance is up");
//         let result = get_system_version(&owner_conf()).await;
//         // let response = reqwest::get(&version_path).await;
//         if result.is_ok() {
//             break;
//         }
//         println!("not yet, trying again");
//         tokio::time::sleep(Duration::from_secs(5)).await;
//     }

//     let version = get_system_version(&owner_conf()).await.unwrap();
//     assert_eq!(version, "test");

//     let projects = get_projects(&owner_conf()).await.unwrap();
//     assert!(projects.is_empty());

//     let insert_project = InsertProject {
//         env: vec![],
//         name: "astrodb".to_owned(),
//         repo_id: 896862196,
//         root: "examples/astrodb".to_owned(),
//     };
//     create_project(&owner_conf(), insert_project).await.unwrap();
//     let projects = get_projects(&owner_conf()).await.unwrap();
//     assert_eq!(projects.get(0).unwrap().name, "astrodb");
// }

// fn owner_conf() -> Configuration {
//     Configuration {
//         base_path: format!("https://api.{INSTANCE_HOSTNAME}"),
//         // client: reqwest::Client::new(),
//         bearer_access_token: Some(generate_token(Role::admin)),
//         ..Default::default()
//     }
// }

// fn run_prezel_container() {
//     let command = "(docker stop prezel || true) && (docker rm prezel || true)";
//     let status = Command::new("sh").arg("-c").arg(command).status().unwrap();
//     assert!(status.success());

//     let command = "docker build -t prezel/prezel:test .";
//     let status = Command::new("sh").arg("-c").arg(command).status().unwrap();
//     assert!(status.success());

//     let prezel_home = get_prezel_home();
//     let command = format!("docker run -p 80:80 -p 443:443 --name prezel -v /opt/prezel:/opt/prezel --network prezel -v /var/run/docker.sock:/var/run/docker.sock -d prezel/prezel:test");
//     let status = Command::new("sh").arg("-c").arg(command).status().unwrap();
//     assert!(status.success());
// }

// ///////////////////////// provider //////////////////

// static A_CHANGE_ID: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("".to_owned()));
// static TXT_CHANGE_ID: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("".to_owned()));

// #[derive(Deserialize)]
// struct Key {
//     key: String,
// }

// #[derive(Deserialize)]
// struct TokenBody {
//     secret: String,
//     id: String,
//     repo: i64,
// }

// #[post("/api/instance/token")]
// async fn token(body: Json<TokenBody>) -> impl Responder {
//     let app_id: u64 = env::var("GH_ID").unwrap().parse().unwrap();

//     let key_json: String = env::var("GH_APP_PRIVATE_KEY").unwrap();
//     let key: Key = serde_json::from_str(&key_json).unwrap();
//     let key = EncodingKey::from_rsa_pem(key.key.as_bytes()).unwrap();

//     let crab = octocrab::OctocrabBuilder::default()
//         .app(AppId(app_id), key)
//         .build()
//         .unwrap();

//     let TokenBody { repo, .. } = body.0;
//     let repo: Option<Repository> = crab
//         .get(format!("/repositories/{repo}"), None::<&()>)
//         .await
//         .unwrap();

//     let Repository { owner, name, .. } = repo.unwrap();
//     let owner = owner.unwrap().login;

//     let installation = crab
//         .apps()
//         .get_repository_installation(owner, name)
//         .await
//         .unwrap();

//     let (_, token) = crab.installation_and_token(installation.id).await.unwrap();

//     HttpResponse::Ok().body(token.expose_secret().clone())
// }

// #[derive(Serialize)]
// struct DnsResponse {
//     ready: bool,
// }

// #[get("/api/instance/dns/{subdomain}")]
// async fn get_dns(_subdomain: Path<String>) -> impl Responder {
//     println!("executing get /api/instance/dns/");
//     let a_ready = is_change_ready(A_CHANGE_ID.read().unwrap().clone()).await;
//     let txt_ready = is_change_ready(TXT_CHANGE_ID.read().unwrap().clone()).await;

//     if a_ready && txt_ready {
//         HttpResponse::Ok().json(DnsResponse { ready: true })
//     } else {
//         HttpResponse::Ok().json(DnsResponse { ready: false })
//     }
// }

// async fn is_change_ready(id: String) -> bool {
//     let config = aws_config::load_from_env().await;
//     let client = aws_sdk_route53::Client::new(&config);

//     let response = client.get_change().id(id).send().await;
//     response.is_ok_and(|response| response.change_info.unwrap().status == ChangeStatus::Insync)
// }

// #[post("/api/instance/dns/{subdomain}")]
// async fn post_dns(subdomain: Path<String>, body: String) -> impl Responder {
//     let name = format!("_acme-challenge.{subdomain}");
//     let value = format!("\"{body}\"");
//     *TXT_CHANGE_ID.write().unwrap() = create_record(&name, RrType::Txt, &value).await;

//     HttpResponse::Ok()
// }

// fn run_api_server() -> anyhow::Result<Server> {
//     let server =
//         HttpServer::new(move || App::new().service(token).service(get_dns).service(post_dns))
//             .workers(1)
//             .bind("0.0.0.0:3000")?
//             .run();
//     Ok(server)
// }

// async fn create_a_record(subdomain: &str, ip: &str) {
//     let name = format!("*.{subdomain}");
//     *A_CHANGE_ID.write().unwrap() = create_record(&name, RrType::A, ip).await;
// }

// async fn create_record(name: &str, r#type: RrType, value: &str) -> String {
//     let config = aws_config::load_from_env().await;
//     let client = aws_sdk_route53::Client::new(&config);

//     let record = ResourceRecord::builder().value(value).build().unwrap();
//     let record_set = ResourceRecordSet::builder()
//         .name(name)
//         .r#type(r#type)
//         .ttl(10)
//         .resource_records(record)
//         .build()
//         .unwrap();
//     let change = Change::builder()
//         .action(ChangeAction::Upsert)
//         .resource_record_set(record_set)
//         .build()
//         .unwrap();
//     let change_batch = ChangeBatch::builder().changes(change).build().unwrap();

//     let response = client
//         .change_resource_record_sets()
//         .hosted_zone_id(env::var("AWS_HOSTED_ZONE_ID").unwrap())
//         .change_batch(change_batch)
//         .send()
//         .await
//         .unwrap();

//     response.change_info.unwrap().id
// }
