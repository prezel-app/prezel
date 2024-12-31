use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4};

use async_trait::async_trait;
use cookie::Cookie;
use http::{header, Response, StatusCode};
use hyper::body::Bytes;
use pingora::apps::http_app::ServeHttp;
use pingora::http::ResponseHeader;
use pingora::listeners::TlsSettings;
use pingora::prelude::http_proxy_service;
use pingora::prelude::{HttpPeer, ProxyHttp, Result, Session};
use pingora::protocols::http::ServerSession;
use pingora::server::Server;
use pingora::services::listening::Service;
use pingora::tls::ssl::{NameType, SniError, SslContext, SslFiletype, SslMethod};
use pingora::ErrorType::Custom;
use pingora::{tls, Error, ErrorSource};
use url::Url;

use crate::api::API_PORT;
use crate::conf::Conf;
use crate::deployments::manager::Manager;
use crate::listener::{Access, Listener};
use crate::logging::{Level, RequestLog, RequestLogger};
use crate::paths::get_container_root;
use crate::time::now;
use crate::tls::{CertificateStore, TlsState};

struct ApiListener;

// TODO: move this to api mod
#[async_trait]
impl Listener for ApiListener {
    async fn access(&self) -> anyhow::Result<Access> {
        Ok(SocketAddrV4::new(Ipv4Addr::LOCALHOST, API_PORT).into())
    }
    fn is_public(&self) -> bool {
        true
    }
}

struct Peer {
    listener: Box<dyn Listener>,
    deployment_id: Option<i64>,
}

impl<L: Listener + 'static> From<L> for Peer {
    fn from(value: L) -> Self {
        Peer {
            listener: Box::new(value),
            deployment_id: None,
        }
    }
}

struct ProxyApp {
    manager: Manager,
    config: Conf,
    request_logger: RequestLogger,
}

impl ProxyApp {
    async fn get_listener_inner(&self, session: &Session) -> Option<Peer> {
        // TODO: try to use session.req_header().uri.host()
        let host = session.get_header(header::HOST)?.to_str().ok()?;

        if host == self.config.api_hostname() {
            Some(ApiListener.into())
        } else {
            let container = self.manager.get_container_by_hostname(host).await?;
            let deployment_id = container.logging_deployment_id.clone();
            Some(Peer {
                listener: Box::new(container),
                deployment_id,
            })
        }
    }

    async fn get_listener(&self, session: &Session) -> Result<Peer, Box<Error>> {
        self.get_listener_inner(session)
            .await
            .ok_or(Error::new_str("No peer found"))
    }

    fn is_authenticated(&self, session: &Session) -> bool {
        let hostname = &self.config.hostname;
        session
            .get_header(header::COOKIE)
            .and_then(|header| header.to_str().ok())
            .and_then(|cookie_header| {
                Cookie::split_parse(cookie_header)
                    .filter_map(|cookie| cookie.ok())
                    .find(|cookie| cookie.name() == hostname && cookie.value() == self.config.token)
            })
            .is_some()
    }
}

#[derive(Default)]
struct RequestCtx {
    deployment: Option<i64>,
    socket: Option<SocketAddrV4>,
}

#[async_trait]
impl ProxyHttp for ProxyApp {
    type CTX = RequestCtx;
    fn new_ctx(&self) -> Self::CTX {
        Default::default()
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let socket = ctx
            .socket
            .ok_or_else(|| Error::new_str("illegal upstream_peer call with empty socket"))?;
        let proxy_to = HttpPeer::new(socket, false, "".to_owned());
        let peer = Box::new(proxy_to);
        Ok(peer)
    }

    // I never simply return true, so maybe I could simply do the redirect from inside upstream_peer?
    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        let Peer {
            listener,
            deployment_id,
        } = self.get_listener(session).await?;
        ctx.deployment = deployment_id;

        // let listener = self.get_listener(session).await?.listener;
        if listener.is_public() || self.is_authenticated(session) {
            let access = listener.access().await.map_err(|error| {
                dbg!(&error);
                Error::create(
                    Custom("Failed to aquire socket"),
                    ErrorSource::Unset, // FIXME: is this correct ??
                    None,
                    Some(error.into()),
                )
            })?;
            match access {
                Access::Socket(socket) => {
                    ctx.socket = Some(socket);
                    Ok(false)
                }
                Access::Loading => {
                    let code = StatusCode::OK;
                    let mut resp: Box<_> = ResponseHeader::build(code, None)?.into();
                    resp.insert_header("Prezel-Loading", "true")?;
                    session.set_keepalive(None); // TODO: review this?
                    session.write_response_header(resp, false).await?;
                    session
                        .write_response_body(
                            Some(Bytes::from_static(include_bytes!(
                                "../resources/loading.html"
                            ))),
                            true,
                        )
                        .await?;
                    Ok(true)
                }
            }
        } else {
            let host = session.get_header(header::HOST).unwrap().to_str().unwrap();
            let path = session.req_header().uri.path();
            let callback = Url::parse(&format!("https://{host}{path}")).unwrap();

            let coordinator = &self.config.coordinator;
            let mut redirect = Url::parse(&format!("{coordinator}/api/instance/auth")).unwrap();
            redirect
                .query_pairs_mut()
                .append_pair("callback", callback.as_str());

            let code = StatusCode::FOUND;
            let mut resp: Box<_> = ResponseHeader::build(code, None)?.into();
            resp.insert_header(header::LOCATION, redirect.as_str())?;
            session.set_keepalive(None); // TODO: review this?
            session.write_response_header(resp, true).await?;
            Ok(true)
        }
    }

    async fn response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        _ctx: &mut Self::CTX,
    ) -> Result<()>
    where
        Self::CTX: Send + Sync,
    {
        let origin = session.get_header(header::ORIGIN);
        let console =
            origin.is_some_and(|header| header.to_str().unwrap() == self.config.coordinator);

        if console {
            upstream_response
                .insert_header(
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    &self.config.coordinator,
                )
                .unwrap();
        }
        Ok(())
    }

    async fn logging(
        &self,
        session: &mut Session,
        _e: Option<&pingora::Error>,
        ctx: &mut Self::CTX,
    ) {
        logging(session, ctx, &self.request_logger);
    }
}

fn logging(session: &Session, ctx: &RequestCtx, logger: &RequestLogger) -> Option<()> {
    let host = session.get_header(header::HOST)?.to_str().ok()?.to_owned();
    let path = session.req_header().uri.path().to_owned();
    let method = session.req_header().method.as_str().to_owned();
    let deployment = ctx.deployment?;
    let response = session.response_written()?;

    let level = if response.status.is_client_error() || response.status.is_server_error() {
        Level::ERROR
    } else {
        Level::INFO
    };

    logger.log(RequestLog {
        level,
        deployment,
        time: now(),
        host,
        method,
        path,
        status: response.status.as_u16(),
    });

    Some(())
}

// struct TlsCallback {
//     certificate: TlsCertificate,
// }

// #[async_trait]
// impl TlsAccept for TlsCallback {
//     async fn certificate_callback(&self, ssl: &mut ssl::SslRef) {
//         tls::ext::ssl_use_certificate(ssl, &self.certificate.cert).unwrap();
//         tls::ext::ssl_use_private_key(ssl, &self.certificate.key).unwrap();

//         //maybe this is enough
//         // let servername = ssl.servername(NameType::HOST_NAME);

//         // ssl.set_certificate(cert)?;
//         // ssl.set_private_key(cert)?;
//     }
// }

struct HttpHandler {
    pub(crate) certificates: CertificateStore,
}

#[async_trait]
impl ServeHttp for HttpHandler {
    async fn response(&self, session: &mut ServerSession) -> Response<Vec<u8>> {
        // if let Some(host) = session.req_header().uri.host() { // FIXME: maybe I should check both?
        if let Some(Ok(host)) = session
            .get_header(header::HOST)
            .map(|header| header.to_str())
        {
            // println!("redirecting HTTP query to {host}");
            let path = session.req_header().uri.path();
            if let Some(TlsState::Challenge {
                challenge_file,
                challenge_content,
            }) = self.certificates.get_domain(host)
            {
                if path == format!("/.well-known/acme-challenge/{challenge_file}") {
                    let content_length = challenge_content.len();
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/plain")
                        .header(header::CONTENT_LENGTH, content_length)
                        .body(challenge_content.into())
                        .unwrap()
                } else {
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body("".into()) // should I set the content length?
                        .unwrap()
                }
            } else {
                let body = "<html><body>301 Moved Permanently</body></html>"
                    .as_bytes()
                    .to_owned();
                Response::builder()
                    .status(StatusCode::MOVED_PERMANENTLY)
                    .header(header::CONTENT_TYPE, "text/html")
                    .header(header::CONTENT_LENGTH, body.len())
                    .header(header::LOCATION, format!("https://{host}{path}"))
                    .body(body)
                    .unwrap()
            }
        } else {
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(vec![]) // FIXME: is this ok? how can I return an empty body?
                .unwrap()
        }
    }
}

pub(crate) fn run_proxy(manager: Manager, config: Conf, store: CertificateStore) {
    let request_logger = RequestLogger::new();
    let mut server = Server::new(None).unwrap();
    server.bootstrap();
    let proxy_app = ProxyApp {
        manager,
        config,
        request_logger,
    };
    let mut https_service = http_proxy_service(&server.configuration, proxy_app);
    let certificate = store.get_default_certificate();
    let mut tls_settings = TlsSettings::intermediate(&certificate.cert, &certificate.key).unwrap();
    for intermediate in certificate.intermediates {
        tls_settings.add_extra_chain_cert(intermediate).unwrap();
    }

    // let path = get_container_root().join("E5.der");
    // let interm = tls::x509::X509::from_der(&fs::read(&path).unwrap()).unwrap();
    // tls_settings.add_extra_chain_cert(interm).unwrap();

    // let path = get_container_root().join("isrg-root-x2.der");
    // let interm = tls::x509::X509::from_der(&fs::read(&path).unwrap()).unwrap();
    // tls_settings.add_extra_chain_cert(interm).unwrap();

    // TODO: tls_settings.enable_h2();

    let cloned = store.clone();
    tls_settings.set_servername_callback(move |ssl, alert| {
        let domain = ssl.servername(NameType::HOST_NAME);
        if let Some(domain) = domain {
            if let Some(TlsState::Ready(certificate)) = cloned.get_domain(domain) {
                // ssl.set_certificate(&certificate.cert); // this does not seem to work
                // ssl.set_private_key(&certificate.key);
                let mut ctx = SslContext::builder(SslMethod::tls()).unwrap();
                ctx.set_certificate_chain_file(&certificate.cert).unwrap();
                ctx.set_private_key_file(&certificate.key, SslFiletype::PEM)
                    .unwrap();
                for intermediate in certificate.intermediates {
                    ctx.add_extra_chain_cert(intermediate).unwrap();
                }
                // ctx.add_extra_chain_cert(cert)
                // ctx.set_alpn_select_callback(prefer_h2);
                let built = ctx.build();
                ssl.set_ssl_context(&built)
                    .map_err(|_| SniError::ALERT_FATAL)?;
            }
        }
        Ok(())
    });

    https_service.add_tls_with_settings("0.0.0.0:443", None, tls_settings);
    server.add_service(https_service);

    let mut http_service = Service::new(
        "HTTP service".to_string(), // TODO: review this name ?
        HttpHandler {
            certificates: store,
        },
    );
    http_service.add_tcp("0.0.0.0:80");
    server.add_service(http_service);

    server.run_forever();
}
