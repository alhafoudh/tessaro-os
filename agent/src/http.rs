//! The one HTTP client, behind a trait.
//!
//! Two call sites use it: the reachability probe (which may be https, against
//! the real internet) and CDP's `/json/list` (plaintext, loopback). They get
//! separate instances because their timeouts differ - the probe distinguishes
//! connect from read, CDP uses one number for everything.
//!
//! The trait exists so `Probe`'s redirect walk and its error-to-message
//! mapping can be tested without a network or a test server. Those messages
//! are what a technician reads on the serial console, so they are worth
//! testing.

use std::io::ErrorKind;
use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

pub const USER_AGENT: &str = "tessaro-agent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub location: Option<String>,
    pub body: String,
}

/// Classified transport failures. The probe turns these into the sentences
/// that reach the journal; keeping the classification separate from the
/// wording means the wording can be asserted on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("the URL is not valid")]
    InvalidUrl,
    #[error("DNS did not resolve the host")]
    Dns,
    #[error("connection refused")]
    ConnectionRefused,
    #[error("network unreachable")]
    NetworkUnreachable,
    #[error("connection timed out")]
    ConnectTimeout,
    #[error("timed out")]
    ReadTimeout,
    #[error("certificate verification failed")]
    CertificateVerification,
    #[error("TLS handshake failed")]
    Tls,
    #[error("{0}")]
    Other(String),
}

pub trait HttpGet {
    fn get(&self, url: &str) -> Result<HttpResponse, HttpError>;
}

pub struct UreqHttp {
    agent: Agent,
}

impl UreqHttp {
    /// `connect_timeout` and `read_timeout` are seconds.
    pub fn new(connect_timeout: i64, read_timeout: i64) -> Self {
        let config = Agent::config_builder()
            .timeout_connect(Some(seconds(connect_timeout)))
            .timeout_recv_response(Some(seconds(read_timeout)))
            // Redirects are followed by hand in Probe, so that a 3xx without a
            // Location and a redirect loop stay distinguishable in the log.
            .max_redirects(0)
            // A 404 is a result, not a transport failure: the probe reports
            // the status and CDP has its own message for it.
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            // NativeTls means openssl, and PlatformVerifier means openssl's
            // own default store - the device's /etc/ssl/certs, via
            // ca-certificates. Not RootCerts::WebPki, which would use the
            // Mozilla roots compiled into this binary and freeze the trust
            // store at build time.
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();

        Self {
            agent: config.new_agent(),
        }
    }
}

impl HttpGet for UreqHttp {
    fn get(&self, url: &str) -> Result<HttpResponse, HttpError> {
        let mut response = self.agent.get(url).call().map_err(classify)?;

        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|err| HttpError::Other(err.to_string()))?;

        Ok(HttpResponse {
            status,
            location,
            body,
        })
    }
}

fn seconds(value: i64) -> Duration {
    Duration::from_secs(value.max(0) as u64)
}

/// Map a ureq failure onto the classes the probe reports on.
///
/// The io::ErrorKind cases are matched structurally because they are the ones
/// that matter operationally - refused, unreachable, DNS. TLS is matched on
/// the message: OpenSSL's verification failures are only distinguishable by
/// their text, and telling "the clock is wrong / the CA store is empty" apart
/// from "the handshake broke" is worth the string match.
fn classify(err: ureq::Error) -> HttpError {
    match err {
        ureq::Error::Io(inner) => match inner.kind() {
            ErrorKind::ConnectionRefused => HttpError::ConnectionRefused,
            ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable => {
                HttpError::NetworkUnreachable
            }
            ErrorKind::TimedOut => HttpError::ReadTimeout,
            _ => classify_text(&inner.to_string()),
        },
        ureq::Error::Timeout(timeout) => match timeout {
            ureq::Timeout::Connect => HttpError::ConnectTimeout,
            _ => HttpError::ReadTimeout,
        },
        ureq::Error::HostNotFound => HttpError::Dns,
        ureq::Error::BadUri(_) => HttpError::InvalidUrl,
        other => classify_text(&other.to_string()),
    }
}

fn classify_text(text: &str) -> HttpError {
    let lowered = text.to_ascii_lowercase();

    if lowered.contains("certificate verify failed")
        || lowered.contains("unable to get local issuer")
        || lowered.contains("self-signed")
        || lowered.contains("self signed")
    {
        HttpError::CertificateVerification
    } else if lowered.contains("tls") || lowered.contains("ssl") || lowered.contains("handshake") {
        HttpError::Tls
    } else if lowered.contains("connection refused") {
        HttpError::ConnectionRefused
    } else if lowered.contains("unreachable") {
        HttpError::NetworkUnreachable
    } else if lowered.contains("failed to lookup") || lowered.contains("name or service not known")
    {
        HttpError::Dns
    } else {
        HttpError::Other(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ureq gates its native-tls connector on `cfg(feature = "native-tls")`
    /// and, when the configured provider is missing, *panics* the first time
    /// it is handed an https URL - it does not fail to build and it does not
    /// return an error. Picking the wrong ureq feature therefore produces a
    /// binary that passes every other test here, ships without libssl, and
    /// then logs a panic on every probe of the real (https) kiosk site.
    ///
    /// Port 1 on the loopback refuses instantly, so this costs nothing and
    /// needs no network: reaching a transport error at all means the TLS
    /// provider was compiled in.
    #[test]
    fn https_reaches_the_transport_instead_of_panicking() {
        let result = UreqHttp::new(1, 1).get("https://127.0.0.1:1/");

        assert!(
            matches!(
                result,
                Err(HttpError::ConnectionRefused | HttpError::ConnectTimeout)
            ),
            "expected a transport failure, got {result:?}"
        );
    }
}
