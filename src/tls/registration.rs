use instant_acme::{
    Account, AuthorizationStatus, Challenge, ChallengeType, Identifier, KeyAuthorization, NewOrder,
    Order, OrderStatus,
};
use pingora::tls;
use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::time::sleep;

use super::certificate::{write_certificate_to_disk, TlsCertificate};
use crate::{conf::Conf, provider};

pub(crate) async fn generate_default_certificate_and_persist(
    account: &Account,
    conf: Conf,
) -> TlsCertificate {
    let wildcard_domain = conf.wildcard_domain();
    generate_certificate_and_persist(account, wildcard_domain, ChallengeType::Dns01, |handle| {
        write_dns_challenge(handle)
    })
    .await
}

pub(crate) async fn generate_certificate_and_persist<
    O: Future<Output = ()>,
    F: FnOnce(Arc<ChallengeTask>) -> O,
>(
    account: &Account,
    domain: String,
    challenge_type: ChallengeType,
    handle_challenge: F,
) -> TlsCertificate {
    let mut order = create_order(account, domain.clone()).await;
    if order.state().status == OrderStatus::Pending {
        let challenge = get_challenge(&mut order, challenge_type).await;
        handle_challenge(challenge.clone()).await;
        complete_challenge(&mut order, challenge.as_ref()).await;
    }
    aquire_certificate(order, domain.clone()).await;
    TlsCertificate::load_from_disk(domain).await.unwrap()
}

pub(crate) struct ChallengeTask {
    challenge: Challenge,
    key_authorization: KeyAuthorization,
}

impl ChallengeTask {
    pub(crate) fn get_dns_value(&self) -> String {
        self.key_authorization.dns_value()
    }

    pub(crate) fn get_http_file_name(&self) -> String {
        self.challenge.token.clone()
    }

    pub(crate) fn get_http_file_content(&self) -> String {
        self.key_authorization.as_str().to_owned()
    }
}

async fn create_order(account: &Account, domain: String) -> Order {
    account
        .new_order(&NewOrder {
            identifiers: &[Identifier::Dns(domain)],
        })
        .await
        .unwrap()
}

async fn get_challenge(order: &mut Order, challenge_type: ChallengeType) -> Arc<ChallengeTask> {
    let authorizations = order.authorizations().await.unwrap();
    let authorization = authorizations.into_iter().next().unwrap();

    // wait for the authorization to be pending
    while authorization.status != AuthorizationStatus::Pending {
        if authorization.status != AuthorizationStatus::Valid {
            panic!("Unrecognized authorization status");
        }
        sleep(Duration::from_secs(1)).await
    }

    let challenge = authorization
        .challenges
        .into_iter()
        .find(|c| c.r#type == challenge_type)
        .ok_or_else(|| anyhow::anyhow!("no dns01 challenge found"))
        .unwrap();
    let key_authorization = order.key_authorization(&challenge);
    ChallengeTask {
        challenge,
        key_authorization,
    }
    .into()
}

async fn complete_challenge(order: &mut Order, challenge: &ChallengeTask) {
    order
        .set_challenge_ready(&challenge.challenge.url)
        .await
        .unwrap();

    // Exponentially back off until the order becomes ready or invalid.
    let mut tries = 1u8;
    let mut delay = Duration::from_millis(250);
    loop {
        sleep(delay).await;
        let state = order.refresh().await.unwrap();
        if let OrderStatus::Ready | OrderStatus::Invalid = state.status {
            break;
        }

        delay *= 2;
        tries += 1;
        if tries > 10 {
            panic!("order is not ready after 5 tries");
        }
    }

    dbg!(order.state());
    assert_eq!(order.state().status, OrderStatus::Ready);
}

async fn aquire_certificate(mut order: Order, domain: String) {
    let mut params = CertificateParams::new(vec![domain.clone()]).unwrap();
    params.distinguished_name = DistinguishedName::new();
    let private_key = KeyPair::generate().unwrap();
    let csr = params.serialize_request(&private_key).unwrap();

    order.finalize(csr.der()).await.unwrap();
    let cert = loop {
        match order.certificate().await.unwrap() {
            Some(cert_chain_pem) => break cert_chain_pem,
            None => sleep(Duration::from_secs(1)).await,
        }
    };

    let key_der = private_key.serialize_der();
    write_certificate_to_disk(
        &domain,
        tls::x509::X509::from_pem(cert.as_bytes()).unwrap(),
        tls::pkey::PKey::private_key_from_der(key_der.as_slice()).unwrap(),
    )
    .unwrap();
}

async fn write_dns_challenge(handle: Arc<ChallengeTask>) {
    let response = handle.get_dns_value();
    provider::post_challenge_response(response).await.unwrap();

    // wait until DNS is ready (both A and TXT record)
    loop {
        if provider::is_dns_ready().await.unwrap() {
            break;
        } else {
            sleep(Duration::from_secs(5)).await;
        }
    }
}
