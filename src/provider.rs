use anyhow::ensure;
use http::Method;
use reqwest::Response;
use serde_json::json;

use crate::conf::Conf;

pub(crate) async fn setup_ip_address() -> anyhow::Result<Response> {
    let conf = Conf::read_async().await;
    let path = format!("/api/instance/ip/{}", conf.hostname);
    query(&conf, Method::POST, &path, None).await
}

pub(crate) async fn get_team_name() -> anyhow::Result<String> {
    let conf = Conf::read_async().await;
    let path = format!("/api/instance/team/{}", conf.hostname);
    let response = query(&conf, Method::GET, &path, None).await?;
    Ok(response.json::<String>().await?)
}

pub(crate) async fn post_challenge_response(response: String) -> anyhow::Result<Response> {
    let conf = Conf::read_async().await;
    let path = format!("/api/instance/dns/{}", conf.hostname);
    let body = json!(response).to_string();
    query(&conf, Method::POST, &path, Some(body)).await
}

pub(crate) async fn is_dns_ready() -> anyhow::Result<bool> {
    let conf = Conf::read_async().await;
    let path = format!("/api/instance/dns/{}", conf.hostname);
    let response = query(&conf, Method::GET, &path, None).await?;
    Ok(response.json::<bool>().await?)
}

pub(crate) async fn get_github_token(repo: i64) -> anyhow::Result<String> {
    let conf = Conf::read_async().await;
    let path = format!("/api/instance/token/{}/{repo}", conf.hostname);
    let response = query(&conf, Method::GET, &path, None).await?;
    Ok(response.json::<String>().await?)
}

async fn query(
    conf: &Conf,
    method: http::Method,
    path: &str,
    body: Option<String>,
) -> anyhow::Result<Response> {
    let client = reqwest::Client::new();
    let url = format!("{}{path}", conf.provider);
    let builder = client
        .request(method, url)
        .bearer_auth(&conf.encoded_secret); // FIXME: should change this at some point, can make it backward compatibly by allowing both secret/jwt on provider side
    let with_body = if let Some(body) = body {
        builder.body(body)
    } else {
        builder
    };
    let response = with_body.send().await?;
    let status_code = response.status().as_u16();
    ensure!(status_code >= 200 && status_code < 300);
    Ok(response)
}
