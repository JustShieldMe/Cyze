//! frostd HTTP client.
//!
//! This is a thin vendoring of upstream `frost_client::client::Client`
//! (which constructs its own `reqwest::Client` internally) so that we can
//! inject a client with a pinned self-signed certificate. The API surface
//! and wire format are identical; all request/response types come from
//! `frost_client::api`.

use frost_client::api;
use frost_client::cipher::PrivateKey;
use uuid::Uuid;

use crate::error::CoreError;

/// How to trust the server's TLS certificate.
#[derive(Debug, Clone)]
pub enum ServerTrust {
    /// Standard WebPKI roots only (public servers with real certificates).
    SystemRoots,
    /// Trust exactly this certificate (PEM) and nothing else: the embedded
    /// server's self-signed certificate, or one imported for an external
    /// server. A certificate a public CA issued for the same host is refused.
    PinnedCertificate(Vec<u8>),
}

/// Accepts a server only if its certificate is byte-for-byte the pinned one,
/// and still checks the handshake signature against that certificate's key.
/// Names and expiry don't apply: the certificate itself is what's trusted.
#[derive(Debug)]
struct PinnedCertVerifier {
    pinned: Vec<u8>,
    provider: std::sync::Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.pinned.as_slice() {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "the server's certificate isn't the one you trusted for it".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn pinned_tls_config(pem: &[u8]) -> Result<rustls::ClientConfig, CoreError> {
    let pem = std::str::from_utf8(pem).map_err(|e| CoreError::Tls(e.to_string()))?;
    let pinned = crate::tls::pem_to_der(pem)?;
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let verifier = std::sync::Arc::new(PinnedCertVerifier { pinned, provider: provider.clone() });
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| CoreError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    // reqwest is built without HTTP/2 here, so offer only HTTP/1.1.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

/// A frostd client bound to one server URL.
pub struct FrostdClient {
    base_url: String,
    client: reqwest::Client,
    access_token: Option<Uuid>,
}

impl FrostdClient {
    /// `base_url` like `https://host:port` (no trailing slash).
    pub fn new(base_url: String, trust: &ServerTrust) -> Result<Self, CoreError> {
        let mut builder = reqwest::Client::builder();
        if let ServerTrust::PinnedCertificate(pem) = trust {
            builder = builder.use_preconfigured_tls(pinned_tls_config(pem)?);
        }
        let client = builder
            .build()
            .map_err(|e| CoreError::Tls(e.to_string()))?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            access_token: None,
        })
    }

    async fn call<A, O>(&self, name: &str, args: &A) -> Result<O, CoreError>
    where
        A: serde::Serialize,
        O: serde::de::DeserializeOwned,
    {
        let req = self.client.post(format!("{}/{}", self.base_url, name)).json(args);
        let req = if let Some(token) = &self.access_token {
            req.bearer_auth(token.to_string())
        } else {
            req
        };
        // `e.to_string()` on a reqwest error yields only "error sending request
        // for url (…)" — the DNS/refused/TLS cause underneath is dropped. Walk the
        // chain and name the fix (a dead quick tunnel, notably, is just a DNS
        // failure and is otherwise indistinguishable from a typo).
        let response = req.send().await.map_err(|e| {
            crate::neterr::connection_error("reaching the signing server", &self.base_url, &e)
        })?;
        if !response.status().is_success() {
            if response.status() == reqwest::StatusCode::INTERNAL_SERVER_ERROR {
                let err = response
                    .json::<api::LowError>()
                    .await
                    .map_err(|e| CoreError::Connection(e.to_string()))?;
                let err: api::Error = err.into();
                Err(CoreError::Server(err.to_string()))
            } else {
                Err(CoreError::Connection(format!(
                    "HTTP {}",
                    response.status()
                )))
            }
        } else {
            let body = response
                .text()
                .await
                .map_err(|e| CoreError::Connection(e.to_string()))?;
            let json_str = if body.is_empty() { "null" } else { &body };
            serde_json::from_str(json_str).map_err(|e| CoreError::Connection(e.to_string()))
        }
    }

    pub async fn challenge(&self) -> Result<api::ChallengeOutput, CoreError> {
        self.call("challenge", &()).await
    }

    /// Challenge–response login: fetch a challenge, sign it with XEdDSA using
    /// the user's communication private key, and store the bearer token.
    pub async fn login(
        &mut self,
        comm_privkey: &PrivateKey,
        comm_pubkey: &api::PublicKey,
    ) -> Result<(), CoreError> {
        let challenge = self.challenge().await?.challenge;
        let signature: [u8; 64] = comm_privkey
            .sign(challenge.as_bytes(), rand::thread_rng())
            .map_err(|e| CoreError::Crypto(e.to_string()))?;
        let output: api::LoginOutput = self
            .call(
                "login",
                &api::LoginArgs {
                    challenge,
                    pubkey: comm_pubkey.clone(),
                    signature: signature.to_vec(),
                },
            )
            .await?;
        self.access_token = Some(output.access_token);
        Ok(())
    }

    pub async fn logout(&mut self) -> Result<(), CoreError> {
        self.call::<(), ()>("logout", &()).await?;
        self.access_token = None;
        Ok(())
    }

    pub async fn create_new_session(
        &self,
        args: &api::CreateNewSessionArgs,
    ) -> Result<api::CreateNewSessionOutput, CoreError> {
        self.call("create_new_session", args).await
    }

    pub async fn list_sessions(&self) -> Result<api::ListSessionsOutput, CoreError> {
        self.call("list_sessions", &()).await
    }

    pub async fn get_session_info(
        &self,
        args: &api::GetSessionInfoArgs,
    ) -> Result<api::GetSessionInfoOutput, CoreError> {
        self.call("get_session_info", args).await
    }

    pub async fn send(&self, args: &api::SendArgs) -> Result<(), CoreError> {
        self.call("send", args).await
    }

    pub async fn receive(&self, args: &api::ReceiveArgs) -> Result<api::ReceiveOutput, CoreError> {
        self.call("receive", args).await
    }

    pub async fn close_session(&self, args: &api::CloseSessionArgs) -> Result<(), CoreError> {
        self.call("close_session", args).await
    }
}
