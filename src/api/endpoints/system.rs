use std::time::UNIX_EPOCH;

use actix_web::{get, web::Data, HttpResponse, Responder};
use openssl::{asn1::Asn1Time, nid::Nid};
use pingora::tls;

use crate::{
    api::{bearer::AnyRole, AppState, Certificate},
    docker::get_container_execution_logs,
    Conf,
};

/// Get system logs
#[utoipa::path(
    responses(
        (status = 200, description = "Fetched system logs", body = [Log])
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[get("/api/system/logs")]
async fn get_logs(_auth: AnyRole) -> impl Responder {
    let logs = get_container_execution_logs("prezel").await;
    HttpResponse::Ok().json(logs.collect::<Vec<_>>())
}

/// Get system certificates
#[utoipa::path(
    responses(
        (status = 200, description = "Fetched system certificates", body = [Certificate])
    ),
    security(
        ("bearerAuth" = [])
    )
)]
#[get("/api/system/certs")]
async fn get_certs(_auth: AnyRole, state: Data<AppState>) -> impl Responder {
    let conf = Conf::read_async().await;
    let main_cert = state.manager.get_main_certificate().await.unwrap();
    let custom_certs = state
        .manager
        .get_custom_domain_certificates()
        .await
        .unwrap();
    let certs: Vec<_> = std::iter::once((conf.wildcard_domain(), main_cert))
        .chain(custom_certs.into_iter())
        .map(|(domain, pem)| prepare_certificate(domain, pem))
        .collect();
    HttpResponse::Ok().json(certs)
}

fn prepare_certificate(domain: String, pem: tls::x509::X509) -> Certificate {
    let epoch = Asn1Time::from_unix(0).unwrap();

    let diff = epoch.diff(pem.not_after()).unwrap();
    let seconds = diff.days * 24 * 60 * 60 + diff.secs;

    dbg!(seconds);
    dbg!(seconds as i64 * 1000);

    dbg!(pem.issuer_name());
    dbg!(pem.subject_name());
    dbg!(pem.public_key());
    dbg!(pem.subject_alt_names());
    for resp in pem.ocsp_responders().unwrap() {
        let str: String = resp.chars().collect();
        dbg!(str);
    }
    for desc in pem.authority_info().unwrap() {
        dbg!(desc.location());
        dbg!(desc.method());
        dbg!(desc.method().nid());
        dbg!(desc.method().nid().as_raw());
        dbg!(desc.method().nid().long_name());
        dbg!(desc.method().nid().short_name());
    }

    let issuer = pem.issuer_name();
    let issuer_org = get_field_value(issuer, Nid::ORGANIZATIONNAME).unwrap();
    let issuer_country = get_field_value(issuer, Nid::COUNTRYNAME).unwrap();
    let issuer_name = get_field_value(issuer, Nid::COMMONNAME).unwrap();
    dbg!(get_field_value(issuer, Nid::MS_CSP_NAME));
    dbg!(get_field_value(issuer, Nid::ISSUER_ALT_NAME));
    dbg!(get_field_value(issuer, Nid::NETSCAPE_SSL_SERVER_NAME));
    dbg!(get_field_value(issuer, Nid::NAME_CONSTRAINTS));
    dbg!(get_field_value(issuer, Nid::COUNTRYNAME));
    dbg!(get_field_value(issuer, Nid::COMMONNAME));

    Certificate {
        domain,
        expiring: seconds as i64 * 1000,
        issuer_org,
        issuer_name,
        issuer_country,
    }
}

fn get_field_value(name: &openssl::x509::X509NameRef, nid: openssl::nid::Nid) -> Option<String> {
    name.entries_by_nid(nid)
        .next()?
        .data()
        .as_utf8()
        .ok()
        .map(|s| s.to_string())
}
