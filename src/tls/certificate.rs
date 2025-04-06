use std::{fs, path::Path};

use futures::{stream, StreamExt};
use openssl::asn1::Asn1Time;
use pingora::tls;

use crate::paths::{get_domain_cert_path, get_domain_key_path, get_intermediate_domain_path};

use super::now_in_seconds;

#[derive(Debug, Clone)]
pub(crate) struct TlsCertificate {
    pub(crate) domain: String,
    pub(crate) cert: String,
    pub(crate) key: String,
    pub(crate) intermediates: Vec<tls::x509::X509>,
}

pub(super) fn read_pem_from_path(path: &Path) -> anyhow::Result<tls::x509::X509> {
    Ok(tls::x509::X509::from_pem(&fs::read(&path)?)?)
}

async fn get_intermediate_pem_path(url: &str) -> anyhow::Result<tls::x509::X509> {
    let intermediate_domain = url.replace("/", "").replace(":", "");
    let path = get_intermediate_domain_path(&intermediate_domain);
    match read_pem_from_path(&path) {
        Ok(cert) => Ok(cert),
        Err(_) => {
            let response = reqwest::get(url).await?;
            let content = response.bytes().await?;
            let cert = tls::x509::X509::from_der(&content)?;
            fs::write(&path, cert.to_pem()?)?;
            read_pem_from_path(&path)
        }
    }
}

impl TlsCertificate {
    pub(crate) async fn load_from_disk(domain: String) -> anyhow::Result<Self> {
        let key = get_domain_key_path(&domain);
        tls::pkey::PKey::private_key_from_pem(&fs::read(&key)?)?;

        let cert = get_domain_cert_path(&domain);
        let cert_content = tls::x509::X509::from_pem(&fs::read(&cert)?)?;
        let info = cert_content.authority_info().unwrap();
        let uris = info
            .iter()
            .filter(|access| matches!(access.method().nid().long_name(), Ok("CA Issuers")))
            .filter_map(|access| access.location().uri())
            .map(|uri| uri.to_owned());
        let intermediates = stream::iter(uris)
            .then(|uri| async move { get_intermediate_pem_path(&uri).await.unwrap() })
            .collect()
            .await;

        Ok(Self {
            domain,
            intermediates,
            cert: cert.display().to_string(),
            key: key.display().to_string(),
        })
    }

    pub(crate) fn load_pem(&self) -> anyhow::Result<tls::x509::X509> {
        read_pem_from_path(&Path::new(&self.cert))
    }

    pub(crate) fn is_expiring_soon(&self) -> bool {
        // FIXME: remove these unwraps?
        // maybe return true if there is an error reading?
        if let Ok(cert) = self.load_pem() {
            let now = Asn1Time::from_unix(now_in_seconds()).unwrap();
            let diff = now.diff(cert.not_after()).unwrap();
            diff.days < 15
        } else {
            println!("error reading pem"); // FIXME: ???????
            false
        }
    }
}

pub(crate) fn write_certificate_to_disk(
    domain: &str,
    cert: tls::x509::X509,
    key: tls::pkey::PKey<tls::pkey::Private>,
) -> anyhow::Result<()> {
    fs::write(get_domain_cert_path(domain), cert.to_pem()?)?;
    fs::write(get_domain_key_path(domain), key.private_key_to_pem_pkcs8()?)?;
    Ok(())
}
