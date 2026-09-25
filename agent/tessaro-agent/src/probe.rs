//! One probe of `KIOSK_PROBE_URL` (or `KIOSK_URL`): is the kiosk site
//! reachable from this device? The answer is always a `ProbeResult`, never an
//! error - "the site is down" is the case this program exists for.

use async_trait::async_trait;

use crate::http::{HttpError, HttpGet};
use crate::log::Log;
use crate::ports::{ProbeResult, Prober};

/// Hops, not attempts: the sixth request in a chain is reported as a loop.
const REDIRECT_LIMIT: u32 = 5;

pub struct Probe<'a, H: HttpGet> {
    log: &'a Log,
    http: H,
    connect_timeout: i64,
    timeout: i64,
}

impl<'a, H: HttpGet> Probe<'a, H> {
    pub fn new(log: &'a Log, http: H, connect_timeout: i64, timeout: i64) -> Self {
        Self {
            log,
            http,
            connect_timeout,
            timeout,
        }
    }

    fn reason_for(&self, error: HttpError) -> String {
        match error {
            HttpError::InvalidUrl => "the URL is not valid".to_string(),
            HttpError::Dns => "DNS did not resolve the host".to_string(),
            // Its own line, not "timed out": a nameserver that swallows the
            // query is the realistic way to stall this probe, and "timed out
            // after 10s" would send a technician to the web server instead.
            HttpError::DnsTimeout => {
                format!("DNS did not answer within {}s", self.connect_timeout)
            }
            HttpError::ConnectionRefused => "connection refused".to_string(),
            HttpError::NetworkUnreachable => "network unreachable".to_string(),
            HttpError::ConnectTimeout => {
                format!("connection timed out after {}s", self.connect_timeout)
            }
            HttpError::ReadTimeout => format!("timed out after {}s", self.timeout),
            HttpError::CertificateVerification => {
                "certificate verification failed - check the device clock and the CA store"
                    .to_string()
            }
            HttpError::Tls => "TLS handshake failed".to_string(),
            HttpError::ProxyUnreachable => {
                "the local proxy is not answering - see `tessaro-ctl network proxy show`"
                    .to_string()
            }
            HttpError::Proxy(status @ (401 | 407)) => crate::http::proxy_refused(status),
            HttpError::Proxy(status) => format!(
                "the proxy could not reach it: HTTP {status} - `tessaro-ctl network proxy test`"
            ),
            HttpError::Other(text) => text,
        }
    }
}

#[async_trait(?Send)]
impl<H: HttpGet> Prober for Probe<'_, H> {
    async fn call(&self, url: &str) -> ProbeResult {
        if !is_http(url) {
            return ProbeResult::failed(self.reason_for(HttpError::InvalidUrl));
        }

        let mut target = url.to_string();

        for hop in 0..=REDIRECT_LIMIT {
            if hop == REDIRECT_LIMIT {
                return ProbeResult::failed("redirect loop");
            }

            let response = match self.http.get(&target).await {
                Ok(response) => response,
                Err(error) => return ProbeResult::failed(self.reason_for(error)),
            };

            match response.status {
                200..=299 => return ProbeResult::ok(response.status),
                300..=399 => match response.location.as_deref() {
                    None => {
                        return ProbeResult::failed(format!(
                            "server answered HTTP {} without a location",
                            response.status
                        ))
                    }
                    Some(location) => {
                        self.log
                            .debug(format!("probe: following redirect to {location}"));
                        target = crate::url::join(&target, location);
                    }
                },
                status => return ProbeResult::failed(format!("server answered HTTP {status}")),
            }
        }

        // Unreachable: the hop == REDIRECT_LIMIT arm above returns first.
        ProbeResult::failed("redirect loop")
    }
}

fn is_http(url: &str) -> bool {
    crate::url::origin(url).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpResponse;
    use std::cell::RefCell;

    /// Scripted responses plus a record of what was actually requested, which
    /// is how the redirect walk is asserted on.
    struct FakeHttp {
        replies: RefCell<Vec<Result<HttpResponse, HttpError>>>,
        requested: RefCell<Vec<String>>,
    }

    impl FakeHttp {
        fn new(replies: Vec<Result<HttpResponse, HttpError>>) -> Self {
            Self {
                replies: RefCell::new(replies),
                requested: RefCell::new(Vec::new()),
            }
        }

        fn ok(status: u16) -> Result<HttpResponse, HttpError> {
            Ok(HttpResponse {
                status,
                location: None,
                body: String::new(),
            })
        }

        fn redirect(location: &str) -> Result<HttpResponse, HttpError> {
            Ok(HttpResponse {
                status: 302,
                location: Some(location.to_string()),
                body: String::new(),
            })
        }
    }

    #[async_trait(?Send)]
    impl HttpGet for &FakeHttp {
        async fn get(&self, url: &str) -> Result<HttpResponse, HttpError> {
            self.requested.borrow_mut().push(url.to_string());
            self.replies
                .borrow_mut()
                .pop()
                .unwrap_or_else(|| panic!("FakeHttp ran out of replies for {url}"))
        }
    }

    fn probe_with<'a>(log: &'a Log, http: &'a FakeHttp) -> Probe<'a, &'a FakeHttp> {
        Probe::new(log, http, 5, 10)
    }

    fn run(replies: Vec<Result<HttpResponse, HttpError>>, url: &str) -> (ProbeResult, Vec<String>) {
        let log = Log::buffered(true);
        // Replies are popped from the back, so script them in reverse.
        let http = FakeHttp::new(replies.into_iter().rev().collect());
        // Driven here rather than with #[tokio::test] on every case: the
        // cases stay about wording and redirects, not about the runtime.
        let result = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(probe_with(&log, &http).call(url));
        let requested = http.requested.borrow().clone();
        (result, requested)
    }

    #[test]
    fn a_200_is_a_success() {
        let (result, _) = run(vec![FakeHttp::ok(200)], "http://kiosk.test/health");

        assert!(result.ok);
        assert_eq!(result.status, Some(200));
    }

    #[test]
    fn redirects_are_followed() {
        let (result, requested) = run(
            vec![FakeHttp::redirect("/real"), FakeHttp::ok(200)],
            "http://kiosk.test/health",
        );

        assert!(result.ok);
        assert_eq!(
            requested,
            vec!["http://kiosk.test/health", "http://kiosk.test/real"]
        );
    }

    #[test]
    fn a_relative_redirect_resolves_against_the_current_path() {
        let (_, requested) = run(
            vec![FakeHttp::redirect("up"), FakeHttp::ok(200)],
            "http://kiosk.test/a/b",
        );

        assert_eq!(requested[1], "http://kiosk.test/a/up");
    }

    #[test]
    fn a_redirect_without_a_location_fails() {
        let response = Ok(HttpResponse {
            status: 302,
            location: None,
            body: String::new(),
        });
        let (result, _) = run(vec![response], "http://kiosk.test/health");

        assert!(!result.ok);
        assert_eq!(result.reason, "server answered HTTP 302 without a location");
    }

    #[test]
    fn a_redirect_loop_gives_up() {
        let replies = (0..6).map(|_| FakeHttp::redirect("/loop")).collect();
        let (result, requested) = run(replies, "http://kiosk.test/health");

        assert!(!result.ok);
        assert_eq!(result.reason, "redirect loop");
        assert_eq!(requested.len(), REDIRECT_LIMIT as usize);
    }

    #[test]
    fn a_404_is_a_failure_with_the_status_in_the_reason() {
        let (result, _) = run(vec![FakeHttp::ok(404)], "http://kiosk.test/health");

        assert!(!result.ok);
        assert_eq!(result.reason, "server answered HTTP 404");
    }

    #[test]
    fn a_refused_connection_says_so() {
        let (result, _) = run(
            vec![Err(HttpError::ConnectionRefused)],
            "http://kiosk.test/health",
        );

        assert_eq!(result.reason, "connection refused");
    }

    #[test]
    fn a_dns_failure_says_so() {
        let (result, _) = run(vec![Err(HttpError::Dns)], "http://kiosk.test/health");

        assert_eq!(result.reason, "DNS did not resolve the host");
    }

    #[test]
    fn a_silent_nameserver_is_named_as_dns_not_as_the_site() {
        let (result, _) = run(vec![Err(HttpError::DnsTimeout)], "http://kiosk.test/health");

        assert_eq!(result.reason, "DNS did not answer within 5s");
    }

    #[test]
    fn an_unreachable_network_says_so() {
        let (result, _) = run(
            vec![Err(HttpError::NetworkUnreachable)],
            "http://kiosk.test/health",
        );

        assert_eq!(result.reason, "network unreachable");
    }

    #[test]
    fn the_timeouts_name_their_own_limit() {
        let (connect, _) = run(vec![Err(HttpError::ConnectTimeout)], "http://kiosk.test/");
        assert_eq!(connect.reason, "connection timed out after 5s");

        let (read, _) = run(vec![Err(HttpError::ReadTimeout)], "http://kiosk.test/");
        assert_eq!(read.reason, "timed out after 10s");
    }

    #[test]
    fn a_certificate_failure_points_at_the_clock_and_the_ca_store() {
        let (result, _) = run(
            vec![Err(HttpError::CertificateVerification)],
            "https://kiosk.test/",
        );

        assert_eq!(
            result.reason,
            "certificate verification failed - check the device clock and the CA store"
        );
    }

    #[test]
    fn any_other_tls_failure_is_reported_as_a_handshake() {
        let (result, _) = run(vec![Err(HttpError::Tls)], "https://kiosk.test/");

        assert_eq!(result.reason, "TLS handshake failed");
    }

    #[test]
    fn a_non_http_url_is_rejected_without_a_request() {
        let (result, requested) = run(vec![], "not a url");

        assert!(!result.ok);
        assert_eq!(result.reason, "the URL is not valid");
        assert!(requested.is_empty());
    }
}
