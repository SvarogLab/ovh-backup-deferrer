//! The OVHcloud API calls the deferrer needs, signed with an application and consumer key.

use std::time::Duration;

use anyhow::{Context, Result};
use jiff::Timestamp;
use jiff::civil::Time;
use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use sha1::{Digest, Sha1};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Credentials {
    pub application_key: String,
    pub application_secret: String,
    pub consumer_key: String,
}

/// The API root for a region key as the official SDKs name them.
pub fn base_url(endpoint: &str) -> Option<&'static str> {
    match endpoint {
        "ovh-eu" => Some("https://eu.api.ovh.com/1.0"),
        "ovh-ca" => Some("https://ca.api.ovh.com/1.0"),
        "ovh-us" => Some("https://api.us.ovhcloud.com/1.0"),
        _ => None,
    }
}

/// Whether `name` can go into the request path verbatim: no separators, and not exactly `.` or
/// `..`, which URL normalisation would resolve into a different path than the one signed.
pub fn is_service_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// The `X-Ovh-Signature` value for one request.
pub fn signature(
    application_secret: &str,
    consumer_key: &str,
    method: &str,
    url: &str,
    body: &str,
    timestamp: i64,
) -> String {
    let signed = format!("{application_secret}+{consumer_key}+{method}+{url}+{body}+{timestamp}");
    format!("$1${}", hex::encode(Sha1::digest(signed)))
}

#[derive(Debug, thiserror::Error)]
pub enum RescheduleError {
    #[error("POST {url} failed")]
    Transport {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("POST {url} returned {status}: {body}")]
    Status {
        url: String,
        status: StatusCode,
        body: String,
    },
}

impl RescheduleError {
    /// Whether the same request may succeed when repeated: the API was unreachable, overloaded
    /// or failed internally. A rejected key or an unknown service fails the same way every time.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Transport { source, .. } => !source.is_builder(),
            Self::Status { status, .. } => {
                status.is_server_error() || *status == StatusCode::TOO_MANY_REQUESTS
            }
        }
    }
}

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    credentials: Credentials,
}

impl Client {
    pub fn new(base_url: impl Into<String>, credentials: Credentials) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("cannot set up the HTTP client")?;
        Ok(Self {
            http,
            base_url: base_url.into(),
            credentials,
        })
    }

    /// Queues the change; the provider applies it asynchronously, minutes later.
    pub async fn reschedule(&self, service: &str, schedule: Time) -> Result<(), RescheduleError> {
        let url = format!("{}/vps/{service}/automatedBackup/reschedule", self.base_url);
        let body = serde_json::json!({ "schedule": schedule }).to_string();
        let timestamp = Timestamp::now().as_second();
        let credentials = &self.credentials;
        let signature = signature(
            &credentials.application_secret,
            &credentials.consumer_key,
            "POST",
            &url,
            &body,
            timestamp,
        );
        let response = self
            .http
            .post(&url)
            .header("X-Ovh-Application", &credentials.application_key)
            .header("X-Ovh-Consumer", &credentials.consumer_key)
            .header("X-Ovh-Timestamp", timestamp.to_string())
            .header("X-Ovh-Signature", signature)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(source) => return Err(RescheduleError::Transport { url, source }),
        };
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        // The status is the diagnosis; the body only elaborates, so failing to read it must not
        // replace the status in the error.
        let body = response
            .text()
            .await
            .unwrap_or_else(|error| format!("(body unreadable: {error})"));
        Err(RescheduleError::Status { url, status, body })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use jiff::civil::time;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;

    const APPLICATION_KEY: &str = "app-key";
    const APPLICATION_SECRET: &str = "app-secret";
    const CONSUMER_KEY: &str = "consumer-key";

    pub(crate) fn client_of(server: &MockServer) -> Client {
        client_at(&server.uri())
    }

    fn client_at(origin: &str) -> Client {
        let credentials = Credentials {
            application_key: APPLICATION_KEY.into(),
            application_secret: APPLICATION_SECRET.into(),
            consumer_key: CONSUMER_KEY.into(),
        };
        Client::new(format!("{origin}/1.0"), credentials).unwrap()
    }

    #[test]
    fn signature_matches_the_reference_vector() {
        assert_eq!(
            signature(
                "secret-as",
                "consumer-ck",
                "POST",
                "https://ca.api.ovh.com/1.0/vps/vps-aaaa.vps.ovh.net/automatedBackup/reschedule",
                r#"{"schedule":"14:00:00"}"#,
                1_790_000_000
            ),
            "$1$1ff7993222922400fa5bdad8e8791b36f26ae32c"
        );
    }

    #[tokio::test]
    async fn reschedule_signs_what_it_sends() {
        let server = MockServer::start().await;
        let origin = server.uri();
        Mock::given(method("POST"))
            .and(path(
                "/1.0/vps/vps-aaaa.vps.ovh.net/automatedBackup/reschedule",
            ))
            .and(body_json(serde_json::json!({ "schedule": "13:40:00" })))
            .and(move |request: &Request| is_signed(request, &origin))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        client_of(&server)
            .reschedule("vps-aaaa.vps.ovh.net", time(13, 40, 0, 0))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_server_error_is_transient_and_a_client_error_is_not() {
        for (status, transient) in [
            (500, true),
            (503, true),
            (429, true),
            (403, false),
            (404, false),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;

            let error = client_of(&server)
                .reschedule("vps-aaaa.vps.ovh.net", time(13, 40, 0, 0))
                .await
                .unwrap_err();

            assert_eq!(error.is_transient(), transient, "{status}");
        }
    }

    #[tokio::test]
    async fn an_unreachable_api_is_transient() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let client = client_at(&format!("http://{closed}"));

        let error = client
            .reschedule("vps-aaaa.vps.ovh.net", time(13, 40, 0, 0))
            .await
            .unwrap_err();

        assert!(error.is_transient(), "{error}");
    }

    /// Whether `request` carries a valid signature over a timestamp from the local clock.
    /// wiremock reports requests against `localhost`, so the signed URL is rebuilt from `origin`.
    fn is_signed(request: &Request, origin: &str) -> bool {
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
        };
        let timestamp: i64 = header("X-Ovh-Timestamp").parse().unwrap_or_default();
        let expected = signature(
            APPLICATION_SECRET,
            CONSUMER_KEY,
            "POST",
            &format!("{origin}{}", request.url.path()),
            &String::from_utf8_lossy(&request.body),
            timestamp,
        );
        header("X-Ovh-Application") == APPLICATION_KEY
            && header("X-Ovh-Consumer") == CONSUMER_KEY
            && (Timestamp::now().as_second() - timestamp).abs() <= 5
            && header("X-Ovh-Signature") == expected
    }
}
