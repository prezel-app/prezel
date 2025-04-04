use std::{
    collections::HashMap,
    fmt::Debug,
    ops::Deref,
    sync::{Arc, RwLock},
    time::Duration,
};

use account::{create_new_account, persist_credentials, read_account};
use certificate::TlsCertificate;
use instant_acme::{Account, ChallengeType, LetsEncrypt};
use registration::{generate_certificate_and_persist, generate_default_certificate_and_persist};

use crate::{conf::Conf, utils::now_in_seconds};

mod account;
pub(crate) mod certificate;
mod registration;

#[derive(Clone, Debug)]
pub(crate) enum TlsState {
    // Pending,
    Challenge {
        challenge_file: String,
        challenge_content: String,
    },
    Ready(TlsCertificate),
}

// TODO: move this to utils file
struct IgnoreDebug<T>(T);
impl<T> Debug for IgnoreDebug<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ignored field")
    }
}
impl<T> Deref for IgnoreDebug<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T: Clone> Clone for IgnoreDebug<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> From<T> for IgnoreDebug<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CertificateStore {
    account: IgnoreDebug<Account>,
    default: Arc<RwLock<TlsCertificate>>,
    domains: Arc<RwLock<HashMap<String, TlsState>>>,
    conf: Conf,
}

impl CertificateStore {
    pub(crate) fn get_all_domains(&self) -> HashMap<String, TlsState> {
        self.domains.read().unwrap().clone()
    }

    pub(crate) fn get_domain(&self, domain: &str) -> Option<TlsState> {
        self.domains.read().unwrap().get(domain).cloned()
    }

    pub(crate) fn has_domain(&self, domain: &str) -> bool {
        self.domains.read().unwrap().contains_key(domain)
    }

    pub(crate) fn get_default_certificate(&self) -> TlsCertificate {
        self.default.read().unwrap().clone()
    }

    /// load certificates from disk, generate any missing, and sets up a task to renew them
    pub(crate) async fn load(conf: &Conf) -> Self {
        let account = match read_account().await {
            Ok(account) => account,
            Err(_) => {
                let (account, credentials) =
                    create_new_account(LetsEncrypt::Production.url()).await;
                persist_credentials(&credentials).await;
                account
            }
        };
        let wildcard_domain = conf.wildcard_domain();
        let default = match TlsCertificate::load_from_disk(wildcard_domain).await {
            Ok(certificate) => certificate,
            _ => generate_default_certificate_and_persist(&account, conf.clone()).await,
        };

        let store = Self {
            account: account.into(),
            default: RwLock::new(default).into(),
            domains: Default::default(),
            conf: conf.clone(),
        };

        let cloned_store = store.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(24 * 3600)); // Every 1 day
            loop {
                interval.tick().await;
                cloned_store.renew().await;
            }
        });

        store
    }

    pub(crate) fn insert_domain(&self, domain: String) {
        let domains = self.domains.clone();
        let account = self.account.clone();
        let cloned_domain = domain.clone();

        tokio::spawn(async move {
            let certificate = match TlsCertificate::load_from_disk(domain.clone()).await {
                Ok(certificate) => certificate,
                _ => {
                    generate_certificate_and_persist(
                        &account,
                        domain.clone(),
                        ChallengeType::Http01,
                        |challenge| {
                            let challenge_file = challenge.get_http_file_name();
                            let challenge_content = challenge.get_http_file_content();
                            domains.write().unwrap().insert(
                                cloned_domain,
                                TlsState::Challenge {
                                    challenge_file,
                                    challenge_content,
                                },
                            );
                            async {}
                        },
                    )
                    .await
                }
            };
            domains
                .write()
                .unwrap()
                .insert(domain, TlsState::Ready(certificate));
        });
    }

    async fn renew(&self) {
        // bear in mind that the intermidiate for a given certificate might change, so I need to read them back
        let default = self.default.read().unwrap().clone();
        if default.is_expiring_soon() {
            let new_default =
                generate_default_certificate_and_persist(&self.account, self.conf.clone()).await;
            *self.default.write().unwrap() = new_default;
        }

        let domains = self.domains.read().unwrap().clone();
        for (domain, state) in domains {
            if let TlsState::Ready(cert) = state {
                if cert.is_expiring_soon() {
                    let new_cert = generate_certificate_and_persist(
                        &self.account,
                        domain.clone(),
                        ChallengeType::Http01,
                        |challenge| {
                            let challenge_file = challenge.get_http_file_name();
                            let challenge_content = challenge.get_http_file_content();
                            self.domains.write().unwrap().insert(
                                domain.clone(),
                                TlsState::Challenge {
                                    challenge_file,
                                    challenge_content,
                                },
                            );
                            async {}
                        },
                    )
                    .await;
                    // FIXME: during this period of time where the cert is back in Challenge state
                    // any incoming requests will fail
                    // maybe I should have a third state Renewing
                    // where I still have access to the old certificate
                    self.domains
                        .write()
                        .unwrap()
                        .insert(domain, TlsState::Ready(new_cert));
                }
            }
        }
    }
}

#[cfg(test)]
mod tls_tests {
    use instant_acme::LetsEncrypt;

    use crate::{conf::Conf, tls::account::create_new_account};

    #[tokio::test]
    async fn test_registration() {
        let _conf = Conf::read();
        let (_account, _credentials) = create_new_account(LetsEncrypt::Staging.url()).await;
        // let certificate = generate_tls_certificate(account, &conf).await;
        // dbg!(certificate);
    }

    #[tokio::test]
    async fn test_account() {
        // TODO: re-enable this test!
        // let account = get_saved_account().await;
        // assert!(account.is_err());

        // get_saved_account_or_new(LetsEncrypt::Staging.url()).await;

        // let account = get_saved_account().await;
        // assert!(account.is_ok());
    }
}
