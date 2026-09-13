use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Result, Vars,
    include_json,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, InvalidHeaderValue};
use reqwest::redirect::Policy;
use reqwest::{Client, Response, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use url::Host;

const DATA_KEY: &str = "data";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub enum ContentType {
    #[serde(rename(deserialize = "none"))]
    None,
    #[serde(rename(deserialize = "text"))]
    Text,
    #[serde(rename(deserialize = "html"))]
    Html,
    #[default]
    #[serde(rename(deserialize = "json"))]
    Json,
    #[serde(rename(deserialize = "urlencoded"))]
    UrlEncoded,
    #[serde(rename(deserialize = "form-data"))]
    FormData,
    #[serde(rename(deserialize = "binary"))]
    Binary,
    #[serde(rename(deserialize = "image"))]
    Image,
    #[serde(rename(deserialize = "video"))]
    Video,
    #[serde(rename(deserialize = "audio"))]
    Audio,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pair {
    pub key: String,
    pub value: JsonValue,
}

/// Bytes read from a response body before the request fails.
pub const DEFAULT_MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// Connect timeout applied when `[http].connect-timeout-ms` is not set.
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 10_000;

fn default_max_response_bytes() -> u64 {
    DEFAULT_MAX_RESPONSE_BYTES
}

fn default_connect_timeout_ms() -> u64 {
    DEFAULT_CONNECT_TIMEOUT_MS
}

/// Package-level `[http]` configuration.
///
/// ```toml
/// [http]
/// # Egress allowlist. When non-empty only these hosts can be requested;
/// # "*.example.com" matches any subdomain but not example.com itself.
/// allowed-hosts = ["api.example.com", "*.example.org"]
/// # Opt-in for internal/on-prem endpoints. Cloud metadata addresses stay
/// # blocked even when this is true.
/// allow-private-addresses = false
/// # Hard cap on a response body; larger bodies fail the act.
/// max-response-bytes = 67108864
/// # Connect timeout; 0 disables it.
/// connect-timeout-ms = 10000
/// # Whole-request timeout; omitted or 0 disables it.
/// timeout-ms = 30000
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct HttpConfig {
    /// Hosts allowed to be requested. Empty means any host (subject to the
    /// address checks below).
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Allow loopback/RFC1918/link-local/unique-local addresses. Cloud
    /// metadata endpoints are rejected regardless.
    #[serde(default)]
    pub allow_private_addresses: bool,
    /// Maximum response body size in bytes; the act fails beyond it.
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: u64,
    /// Connection establishment timeout in milliseconds; 0 disables it.
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    /// Total request timeout in milliseconds, including reading the body;
    /// `None` or 0 leaves the request unbounded.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            allowed_hosts: Vec::new(),
            allow_private_addresses: false,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            connect_timeout_ms: DEFAULT_CONNECT_TIMEOUT_MS,
            timeout_ms: None,
        }
    }
}

/// Egress rules enforced on every request: a host allowlist, an address
/// range policy, and the connect-time resolver check.
#[derive(Debug, Clone)]
struct EgressPolicy {
    allowed_hosts: Vec<String>,
    allow_private_addresses: bool,
}

impl EgressPolicy {
    fn new(config: &HttpConfig) -> Self {
        Self {
            allowed_hosts: config
                .allowed_hosts
                .iter()
                .map(|host| host.trim().trim_end_matches('.').to_ascii_lowercase())
                .filter(|host| !host.is_empty())
                .collect(),
            allow_private_addresses: config.allow_private_addresses,
        }
    }

    /// Synchronous checks: scheme, host allowlist and literal-address policy.
    fn check_url(&self, url: &Url) -> Result<()> {
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(ActError::Package(format!(
                "http egress policy: unsupported url scheme '{scheme}'"
            )));
        }
        let host = url_host(url)?;
        if !self.host_allowed(&host) {
            return Err(ActError::Package(format!(
                "http egress policy: host '{host}' is not in allowed-hosts"
            )));
        }
        if let Some(ip) = host_ip(url)
            && !self.address_allowed(ip)
        {
            return Err(ActError::Package(format!(
                "http egress policy: address {ip} is blocked"
            )));
        }
        Ok(())
    }

    /// Resolve-time check: a hostname must resolve to at least one permitted
    /// address. The connector resolver applies the same rule to the addresses
    /// it actually dials, so a rebinding answer is still caught.
    async fn check_resolved(&self, url: &Url) -> Result<()> {
        let Some(Host::Domain(domain)) = url.host() else {
            return Ok(());
        };
        let addrs = tokio::net::lookup_host((domain, 0)).await.map_err(|err| {
            ActError::Runtime(format!(
                "http egress policy: failed to resolve '{domain}': {err}"
            ))
        })?;
        self.filter_addrs(domain, addrs).map(|_| ())
    }

    fn host_allowed(&self, host: &str) -> bool {
        if self.allowed_hosts.is_empty() {
            return true;
        }
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.allowed_hosts
            .iter()
            .any(|pattern| match pattern.strip_prefix("*.") {
                Some(suffix) => host.ends_with(&format!(".{suffix}")),
                None => host == *pattern,
            })
    }

    fn address_allowed(&self, ip: IpAddr) -> bool {
        if is_metadata(ip) || is_never_allowed(ip) {
            return false;
        }
        self.allow_private_addresses || !is_private(ip)
    }

    /// Drop blocked addresses; error only when nothing permitted remains.
    fn filter_addrs(
        &self,
        host: &str,
        addrs: impl Iterator<Item = SocketAddr>,
    ) -> Result<Vec<SocketAddr>> {
        let mut permitted = Vec::new();
        let mut blocked = Vec::new();
        for addr in addrs {
            if self.address_allowed(addr.ip()) {
                permitted.push(addr);
            } else {
                blocked.push(addr.ip());
            }
        }
        if permitted.is_empty() {
            return Err(ActError::Package(format!(
                "http egress policy: '{host}' resolves only to blocked addresses {blocked:?}"
            )));
        }
        Ok(permitted)
    }
}

fn url_host(url: &Url) -> Result<String> {
    match url.host() {
        Some(Host::Ipv4(ip)) => Ok(ip.to_string()),
        Some(Host::Ipv6(ip)) => Ok(ip.to_string()),
        Some(Host::Domain(domain)) => Ok(domain.to_string()),
        None => Err(ActError::Package(
            "http egress policy: url has no host".to_string(),
        )),
    }
}

fn host_ip(url: &Url) -> Option<IpAddr> {
    match url.host()? {
        Host::Ipv4(ip) => Some(IpAddr::V4(ip)),
        Host::Ipv6(ip) => Some(IpAddr::V6(ip)),
        Host::Domain(_) => None,
    }
}

/// Address ranges rejected even when `allow-private-addresses` is set:
/// unspecified, multicast, broadcast, reserved and documentation space.
fn is_never_allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_documentation()
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 240
        }
        IpAddr::V6(ip) => match ip.to_ipv4() {
            Some(ip) => is_never_allowed(IpAddr::V4(ip)),
            None => ip.is_unspecified() || ip.is_multicast(),
        },
    }
}

/// Loopback, RFC1918, link-local, carrier-grade NAT and IPv6 unique-local
/// ranges. IPv4-mapped IPv6 addresses are judged by their IPv4 value.
fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local() || is_cgnat(ip),
        IpAddr::V6(ip) => match ip.to_ipv4() {
            Some(ip) => is_private(IpAddr::V4(ip)),
            None => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
        },
    }
}

/// 100.64.0.0/10 carrier-grade NAT, which includes the Alibaba Cloud
/// metadata endpoint 100.100.100.200.
fn is_cgnat(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..128).contains(&b)
}

/// Cloud instance metadata endpoints: AWS/GCP/Azure/DigitalOcean share
/// 169.254.169.254, Alibaba Cloud uses 100.100.100.200 and the AWS IPv6
/// endpoint is fd00:ec2::254.
fn is_metadata(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            octets == [169, 254, 169, 254] || octets == [100, 100, 100, 200]
        }
        IpAddr::V6(ip) => match ip.to_ipv4() {
            Some(ip) => is_metadata(IpAddr::V4(ip)),
            None => ip == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254),
        },
    }
}

/// Resolver that filters blocked addresses before the connector dials them.
#[derive(Debug)]
struct SafeResolver {
    policy: Arc<EgressPolicy>,
}

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        let policy = self.policy.clone();
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|err| boxed_err(format!("failed to resolve '{host}': {err}")))?;
            let permitted = policy
                .filter_addrs(&host, addrs)
                .map_err(|err| boxed_err(err.to_string()))?;
            Ok(Box::new(permitted.into_iter()) as Addrs)
        })
    }
}

fn boxed_err(message: String) -> Box<dyn std::error::Error + Send + Sync> {
    message.into()
}

#[derive(Debug, Clone)]
pub struct HttpPackage {
    client: Client,
    policy: Arc<EgressPolicy>,
    max_response_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpPackageParams {
    pub url: String,
    pub method: String,
    #[serde(default)]
    #[serde(rename(deserialize = "content-type"))]
    pub content_type: ContentType,
    #[serde(default)]
    pub headers: Vec<Pair>,
    #[serde(default)]
    pub params: Vec<Pair>,
    pub body: Option<JsonValue>,
    /// Total request timeout in milliseconds. Overrides the client-wide
    /// `[http].timeout-ms` default; when unset the config default applies.
    #[serde(default, rename(deserialize = "timeout-ms"))]
    pub timeout_ms: Option<u64>,
}

#[async_trait::async_trait]
impl ActPackage for HttpPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.core.http",
            name: "Http",
            desc: "do a http request",
            version: "0.1.0",
            icon: r#"<svg viewBox="0 0 1024 1024" version="1.1" xmlns="http://www.w3.org/2000/svg" width="24" height="24" fill="currentColor"><path d="M53.312 512a458.688 458.688 0 1 1 917.376 0A458.688 458.688 0 0 1 53.312 512z m339.52-376.32a394.048 394.048 0 0 0-138.88 77.632c25.6 22.08 53.952 40.96 84.48 55.936 6.784-25.408 14.528-49.152 23.168-70.848 9.216-22.912 19.584-44.16 31.232-62.72zM209.024 258.944A392.896 392.896 0 0 0 118.592 480h191.232c1.472-51.584 6.528-100.992 14.656-146.688a459.136 459.136 0 0 1-115.456-74.24z m422.144 629.376a394.176 394.176 0 0 0 138.88-77.696 395.328 395.328 0 0 0-84.48-55.936 610.752 610.752 0 0 1-23.168 70.848c-9.152 22.912-19.584 44.096-31.232 62.72z m183.808-123.456A392.832 392.832 0 0 0 905.408 544H714.24a1008.704 1008.704 0 0 1-14.72 146.624c42.24 19.008 81.152 44.16 115.456 74.304zM905.408 480a392.96 392.96 0 0 0-90.432-220.928 459.2 459.2 0 0 1-115.392 74.24c8.064 45.696 13.12 95.104 14.656 146.688h191.168zM769.92 213.312a394.112 394.112 0 0 0-138.88-77.696c11.712 18.688 22.144 39.872 31.296 62.784 8.704 21.76 16.448 45.44 23.104 70.848a395.328 395.328 0 0 0 84.48-55.936zM392.832 888.384a399.04 399.04 0 0 1-31.232-62.784 610.624 610.624 0 0 1-23.104-70.848 395.264 395.264 0 0 0-84.48 55.936 394.048 394.048 0 0 0 138.88 77.696z m-183.744-123.456a459.136 459.136 0 0 1 115.392-74.24A1008.448 1008.448 0 0 1 309.76 544H118.656a392.896 392.896 0 0 0 90.496 220.928zM512 117.312c-11.904 0-26.496 5.952-43.2 23.552-16.64 17.664-33.216 44.928-47.744 81.28-8.448 21.12-16 44.8-22.528 70.656A394.752 394.752 0 0 0 512 309.312c39.488 0 77.568-5.76 113.536-16.512a557.824 557.824 0 0 0-22.528-70.592c-14.592-36.416-31.104-63.68-47.808-81.344-16.64-17.6-31.232-23.552-43.2-23.552zM373.824 480h276.352a953.28 953.28 0 0 0-11.712-124.352A458.88 458.88 0 0 1 512 373.312a458.88 458.88 0 0 1-126.4-17.664A953.216 953.216 0 0 0 373.76 480z m11.776 188.352A458.944 458.944 0 0 1 512 650.688c43.84 0 86.272 6.144 126.464 17.664 6.272-38.656 10.368-80.448 11.712-124.352H373.824c1.344 43.904 5.44 85.76 11.776 124.352zM512 714.688c-39.424 0-77.568 5.76-113.472 16.512 6.464 25.792 14.08 49.472 22.528 70.592 14.528 36.416 31.04 63.68 47.744 81.344 16.64 17.6 31.296 23.552 43.2 23.552 11.968 0 26.56-5.952 43.2-23.552 16.704-17.664 33.28-44.928 47.808-81.28 8.448-21.184 16-44.8 22.464-70.656A394.752 394.752 0 0 0 512 714.688z" ></path></svg>"#,
            doc: "",
            schema: include_json!("./in-schema.json"),
            options: Some(include_json!("./ui-schema.json")),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }

    fn new(config: &acts::Config) -> Result<Self>
    where
        Self: Sized,
    {
        let config = if config.has("http") {
            config.get::<HttpConfig>("http")?
        } else {
            HttpConfig::default()
        };
        Self::from_config(&config)
    }

    async fn execute(
        &self,
        _ctx: &acts::Context,
        params: &serde_json::Value,
    ) -> Result<Option<Vars>> {
        let params = serde_json::from_value::<HttpPackageParams>(params.clone()).map_err(|e| {
            ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;

        self.request(&params).await
    }
}

impl HttpPackage {
    /// Build the package from an explicit `[http]` config, bypassing the
    /// engine config lookup.
    pub fn from_config(config: &HttpConfig) -> Result<Self> {
        if config.max_response_bytes == 0 {
            return Err(ActError::Config(
                "http.max-response-bytes must be greater than zero".to_string(),
            ));
        }
        let policy = Arc::new(EgressPolicy::new(config));
        let mut builder = Client::builder()
            .dns_resolver(Arc::new(SafeResolver {
                policy: policy.clone(),
            }))
            .redirect(redirect_policy(policy.clone()));
        if config.connect_timeout_ms > 0 {
            builder = builder.connect_timeout(Duration::from_millis(config.connect_timeout_ms));
        }
        if let Some(timeout_ms) = config.timeout_ms.filter(|timeout| *timeout > 0) {
            builder = builder.timeout(Duration::from_millis(timeout_ms));
        }
        let client = builder
            .build()
            .map_err(|err| ActError::Config(format!("failed to build http client: {err}")))?;

        Ok(Self {
            client,
            policy,
            max_response_bytes: config.max_response_bytes,
        })
    }

    async fn request(&self, params: &HttpPackageParams) -> Result<Option<Vars>> {
        let mut ret = Vars::new();
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("accept"),
            HeaderValue::from_static("*/*"),
        );

        for Pair { key, value } in &params.headers {
            headers.insert(
                key.parse::<HeaderName>()
                    .map_err(|err| ActError::Runtime(err.to_string()))?,
                value
                    .to_string()
                    .parse()
                    .map_err(|err: InvalidHeaderValue| ActError::Runtime(err.to_string()))?,
            );
        }
        let mut query = Vec::new();
        for Pair { key, value } in &params.params {
            query.push((key.clone(), value.clone()));
        }

        let url = Url::parse(&params.url)
            .map_err(|err| ActError::Package(format!("invalid url '{}': {err}", params.url)))?;
        self.policy.check_url(&url)?;
        self.policy.check_resolved(&url).await?;

        let method: reqwest::Method = params
            .method
            .parse()
            .map_err(|_| ActError::Runtime(format!("invalid method '{}'", params.method)))?;
        let mut request = self
            .client
            .request(method, url)
            .headers(headers)
            .query(&query);
        if let Some(timeout_ms) = params.timeout_ms {
            if timeout_ms == 0 {
                return Err(ActError::Package(
                    "timeout-ms must be greater than zero".to_string(),
                ));
            }
            request = request.timeout(Duration::from_millis(timeout_ms));
        }

        match params.content_type {
            ContentType::Text | ContentType::Html => {
                if let Some(text) = &params.body {
                    let data = text.as_str().ok_or(ActError::Package(
                        "content-type did not match the body content".to_string(),
                    ))?;
                    request = request.body::<String>(data.to_string());
                }
            }
            ContentType::Json => {
                if let Some(json) = &params.body {
                    let body = serde_json::to_vec(json)?;
                    request = request.body(body);
                }
            }
            ContentType::FormData | ContentType::UrlEncoded => {
                if let Some(form) = &params.body {
                    let data = form.as_object().ok_or(ActError::Package(
                        "content-type did not match the body content".to_string(),
                    ))?;
                    request = request.form(data);
                }
            }
            ContentType::Binary | ContentType::Image | ContentType::Video | ContentType::Audio => {
                if let Some(value) = &params.body {
                    let data = value.as_str().ok_or(ActError::Package(
                        "content-type did not match the body content".to_string(),
                    ))?;
                    let data = STANDARD
                        .decode(data)
                        .map_err(|err| ActError::Package(err.to_string()))?;
                    request = request.body(data);
                }
            }
            _ => {}
        }

        let res = request.send().await.map_err(map_send_err)?;

        let status = res.status();
        let content_type = match res.headers().get(CONTENT_TYPE) {
            Some(value) => Some(
                value
                    .to_str()
                    .map_err(|err| ActError::Package(err.to_string()))?
                    .to_string(),
            ),
            None => None,
        };
        let response_type = get_content_type(content_type.as_deref().unwrap_or("application/json"));
        if !matches!(response_type, ContentType::None) {
            let body = read_body_capped(res, self.max_response_bytes).await?;
            match response_type {
                ContentType::Text | ContentType::Html => {
                    ret.insert(
                        DATA_KEY.to_string(),
                        decode_text(&body, content_type.as_deref()).into(),
                    );
                }
                ContentType::Json => {
                    let data = serde_json::from_slice::<JsonValue>(&body).map_err(|err| {
                        ActError::Package(format!("failed to parse json response: {err}"))
                    })?;
                    ret.insert(DATA_KEY.to_string(), data);
                }
                ContentType::Binary
                | ContentType::Image
                | ContentType::Video
                | ContentType::Audio => {
                    ret.insert(DATA_KEY.to_string(), STANDARD.encode(&body).into());
                }
                _ => {}
            }
        }
        if !status.is_success() {
            return Err(ActError::Exception {
                ecode: status.as_u16().to_string(),
                message: ret.get(DATA_KEY).unwrap_or(status.to_string()),
            });
        }

        Ok(Some(ret))
    }
}

fn map_package_err(err: reqwest::Error) -> ActError {
    ActError::Package(err.to_string())
}

fn get_content_type(mime_type: &str) -> ContentType {
    let mut ret = ContentType::None;
    if mime_type.starts_with("application/json") {
        ret = ContentType::Json;
    } else if mime_type.starts_with("text/html") {
        ret = ContentType::Html;
    } else if mime_type.starts_with("application/x-www-form-urlencoded") {
        ret = ContentType::UrlEncoded;
    } else if mime_type.starts_with("multipart/form-data") {
        ret = ContentType::FormData;
    } else if mime_type.starts_with("image/") {
        ret = ContentType::Image;
    } else if mime_type.starts_with("audio/") {
        ret = ContentType::Audio;
    } else if mime_type.starts_with("video/") {
        ret = ContentType::Video;
    } else if mime_type.starts_with("text/") || mime_type.starts_with("application/javascript") {
        ret = ContentType::Text;
    }

    ret
}

/// Read a body into memory, failing once `max_bytes` is exceeded. Streaming
/// keeps the cap enforced for chunked responses without `Content-Length`.
async fn read_body_capped(res: Response, max_bytes: u64) -> Result<Vec<u8>> {
    if let Some(length) = res.content_length()
        && length > max_bytes
    {
        return Err(over_limit(max_bytes));
    }
    let mut body = Vec::new();
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(map_package_err)?;
        if body.len() as u64 + chunk.len() as u64 > max_bytes {
            return Err(over_limit(max_bytes));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn over_limit(max_bytes: u64) -> ActError {
    ActError::Package(format!(
        "http response body exceeds max-response-bytes ({max_bytes})"
    ))
}

/// Decode a text/html body honoring the response charset, mirroring
/// `reqwest::Response::text` which the capped reader replaced.
fn decode_text(body: &[u8], content_type: Option<&str>) -> String {
    let charset = content_type
        .and_then(|value| {
            value.split(';').skip(1).find_map(|part| {
                let (name, value) = part.split_once('=')?;
                name.trim()
                    .eq_ignore_ascii_case("charset")
                    .then(|| value.trim().trim_matches('"'))
            })
        })
        .unwrap_or("utf-8");
    let encoding =
        encoding_rs::Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(body);
    text.into_owned()
}

/// Redirects are re-validated against the egress policy so a permitted URL
/// cannot bounce the client to a loopback/private/metadata target.
fn redirect_policy(policy: Arc<EgressPolicy>) -> Policy {
    Policy::custom(move |attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        match policy.check_url(attempt.url()) {
            Ok(()) => attempt.follow(),
            Err(err) => attempt.error(err.to_string()),
        }
    })
}

/// Include the error source chain: reqwest's `Display` prints only a generic
/// prefix, and the policy/resolver reason lives further down the chain.
fn map_send_err(err: reqwest::Error) -> ActError {
    let mut message = err.to_string();
    let mut source = std::error::Error::source(&err);
    while let Some(err) = source {
        message.push_str(": ");
        message.push_str(&err.to_string());
        source = err.source();
    }
    ActError::Runtime(format!("Http error: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn params(url: String) -> HttpPackageParams {
        HttpPackageParams {
            url,
            method: "GET".to_string(),
            content_type: ContentType::Json,
            headers: Vec::new(),
            params: Vec::new(),
            body: None,
            timeout_ms: None,
        }
    }

    fn package(config: HttpConfig) -> HttpPackage {
        HttpPackage::from_config(&config).expect("build http package")
    }

    /// Serve one HTTP/1.1 response on an ephemeral loopback port.
    async fn serve(response: Vec<u8>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let _ = socket.write_all(&response).await;
            let _ = socket.shutdown().await;
        });
        addr
    }

    #[test]
    fn blocks_loopback_private_link_local_and_metadata() {
        let policy = EgressPolicy::new(&HttpConfig::default());
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.1",
            "169.254.169.254",
            "100.100.100.200",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(!policy.address_allowed(ip), "{ip} must be blocked");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(policy.address_allowed(ip), "{ip} must be allowed");
        }
    }

    #[test]
    fn private_opt_in_keeps_metadata_and_reserved_blocked() {
        let policy = EgressPolicy::new(&HttpConfig {
            allow_private_addresses: true,
            ..Default::default()
        });
        assert!(policy.address_allowed("192.168.1.1".parse().unwrap()));
        assert!(policy.address_allowed("127.0.0.1".parse().unwrap()));
        assert!(!policy.address_allowed("169.254.169.254".parse().unwrap()));
        assert!(!policy.address_allowed("100.100.100.200".parse().unwrap()));
        assert!(!policy.address_allowed("0.0.0.0".parse().unwrap()));
    }

    #[test]
    fn allowed_hosts_match_exact_and_wildcard() {
        let policy = EgressPolicy::new(&HttpConfig {
            allowed_hosts: vec!["api.example.com".into(), "*.example.org".into()],
            ..Default::default()
        });
        assert!(policy.host_allowed("api.example.com"));
        assert!(policy.host_allowed("API.EXAMPLE.COM"));
        assert!(policy.host_allowed("a.example.org"));
        assert!(!policy.host_allowed("example.org"));
        assert!(!policy.host_allowed("api.example.com.evil.com"));
        assert!(!policy.host_allowed("evil.com"));
    }

    #[tokio::test]
    async fn rejects_loopback_ip_literal() {
        let err = package(HttpConfig::default())
            .request(&params("http://127.0.0.1:9/".into()))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("address 127.0.0.1 is blocked"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn rejects_hostname_resolving_to_loopback() {
        let err = package(HttpConfig::default())
            .request(&params("http://localhost:9/".into()))
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("blocked addresses") || message.contains("failed to resolve"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn rejects_host_outside_allowlist() {
        let config = HttpConfig {
            allowed_hosts: vec!["allowed.example".into()],
            ..Default::default()
        };
        let err = package(config)
            .request(&params("http://blocked.example/".into()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not in allowed-hosts"), "{err}");
    }

    #[tokio::test]
    async fn reads_body_within_limit() {
        let addr = serve(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello"
                .to_vec(),
        )
        .await;
        let config = HttpConfig {
            allow_private_addresses: true,
            ..Default::default()
        };
        let out = package(config)
            .request(&params(format!("http://{addr}/")))
            .await
            .unwrap()
            .unwrap();
        let data: String = out.get(DATA_KEY).unwrap();
        assert_eq!(data, "hello");
    }

    #[tokio::test]
    async fn rejects_body_over_content_length() {
        let addr = serve(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 1000\r\n\r\n".to_vec(),
        )
        .await;
        let config = HttpConfig {
            allow_private_addresses: true,
            max_response_bytes: 16,
            ..Default::default()
        };
        let err = package(config)
            .request(&params(format!("http://{addr}/")))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("max-response-bytes"), "{err}");
    }

    #[tokio::test]
    async fn rejects_chunked_body_over_limit() {
        let chunk = "x".repeat(64);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{chunk}\r\n0\r\n\r\n",
            chunk.len()
        );
        let addr = serve(response.into_bytes()).await;
        let config = HttpConfig {
            allow_private_addresses: true,
            max_response_bytes: 16,
            ..Default::default()
        };
        let err = package(config)
            .request(&params(format!("http://{addr}/")))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("max-response-bytes"), "{err}");
    }

    #[tokio::test]
    async fn request_timeout_is_enforced() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await;
        });
        let config = HttpConfig {
            allow_private_addresses: true,
            timeout_ms: Some(150),
            ..Default::default()
        };
        let err = package(config)
            .request(&params(format!("http://{addr}/")))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Http error"), "{err}");
    }

    #[tokio::test]
    async fn blocks_redirect_outside_allowlist() {
        let response =
            b"HTTP/1.1 302 Found\r\nLocation: http://10.0.0.1:9/secret\r\nContent-Length: 0\r\n\r\n"
                .to_vec();
        let addr = serve(response).await;
        let config = HttpConfig {
            allowed_hosts: vec!["127.0.0.1".into()],
            allow_private_addresses: true,
            ..Default::default()
        };
        let err = package(config)
            .request(&params(format!("http://{addr}/")))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not in allowed-hosts"), "{err}");
    }
}
