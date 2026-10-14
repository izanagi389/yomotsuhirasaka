//! 外部へのHTTPの送信の共通の仕組み（13-security.md「外部への送信」、Q95, Q112, Q128, Q139）。
//!
//! Webhook、監査ログのWebhookへの書き出し、フックからの通信、LLMの呼び出し、OIDCの設定の取得など、
//! 外部へのHTTPの送信は、すべてここを通す（CLAUDE.md の禁止事項）。
//!
//! - 送信のたびに名前解決し、得たアドレスを1つずつ検査する。内部向けのアドレスが1つでも含まれれば送らない。
//! - **検査したアドレスに固定して接続する。** 検査の後に名前解決をやり直さない（DNSリバインディング対策）。
//!   TLSの証明書は、本来のホスト名で確認する。
//! - 送信先は原則HTTPSに限る。サーバー管理者が例外として許した内部の宛先だけ、内部向けのアドレスとHTTPを許す。
//! - リダイレクトはたどらない（応答の状態コードを返すだけ）。
//! - プロキシの環境変数は使わない。出口のプロキシは設定で明示したときだけ使い、
//!   既定では中継の依頼（CONNECT）の宛先に、検査したIPアドレスを指定する（Q128）。
//! - 相手の応答の本文は、呼び出し側が上限を付けて求めたときだけ読む。管理APIの応答には出さない（Q112）。

pub mod address;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;
use url::{Host, Url};

use crate::config::{EgressConfig, ProxyConnectBy};

/// 送信できなかった理由。`code` はジョブの失敗の記録や管理APIの応答に使う（宛先や相手の応答の本文は含めない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressError {
    /// URLとして解釈できない、ホストがない、など。
    InvalidUrl,
    /// HTTPS以外（例外の宛先のHTTPを除く）。
    SchemeNotAllowed,
    /// 名前解決の結果に、内部向けのアドレスが含まれる。
    InternalDestination,
    /// 名前解決できない。
    DnsFailed,
    /// 接続できない。
    ConnectFailed,
    /// 出口のプロキシが中継を断った、またはプロキシに接続できない。
    ProxyFailed,
    /// TLSを確立できない（証明書の確認の失敗を含む）。
    TlsFailed,
    /// 上限の時間を超えた。
    Timeout,
    /// HTTPのやり取りに失敗した。
    HttpFailed,
    /// 応答の本文が上限を超えた。
    ResponseTooLarge,
}

impl EgressError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid_url",
            Self::SchemeNotAllowed => "scheme_not_allowed",
            Self::InternalDestination => "internal_destination",
            Self::DnsFailed => "dns_failed",
            Self::ConnectFailed => "connect_failed",
            Self::ProxyFailed => "proxy_failed",
            Self::TlsFailed => "tls_failed",
            Self::Timeout => "timeout",
            Self::HttpFailed => "http_failed",
            Self::ResponseTooLarge => "response_too_large",
        }
    }
}

impl std::fmt::Display for EgressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for EgressError {}

/// 送るリクエスト。
pub struct EgressRequest {
    pub method: hyper::Method,
    pub url: String,
    /// 付けるヘッダー（`Host` と `Content-Length` は自動で付ける）。
    pub headers: Vec<(hyper::header::HeaderName, hyper::header::HeaderValue)>,
    pub body: Vec<u8>,
    /// 応答の本文を読むときの上限（バイト）。None なら本文を読まない。
    pub read_body_limit: Option<usize>,
}

/// 受け取った応答。リダイレクトの応答（3xx）もそのまま返す。
#[derive(Debug)]
pub struct EgressResponse {
    pub status: u16,
    /// `read_body_limit` を指定したときだけ。
    pub body: Option<Vec<u8>>,
}

/// 検査を通った送信先。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub https: bool,
    /// URLのホスト（TLSの証明書の確認と `Host` ヘッダーに使う）。
    pub host: Host<String>,
    pub port: u16,
    /// 名前解決して検査したアドレス。この中のどれかにだけ接続する。
    pub addrs: Vec<SocketAddr>,
}

/// 外部への送信の共通の仕組み。
#[derive(Clone)]
pub struct Egress {
    inner: Arc<Inner>,
}

struct Inner {
    exceptions: Exceptions,
    tls: tokio_rustls::TlsConnector,
    connect_timeout: Duration,
    request_timeout: Duration,
    proxy: Option<Proxy>,
}

struct Proxy {
    host: String,
    port: u16,
    connect_by: ProxyConnectBy,
}

/// 例外として許す内部の宛先。
#[derive(Debug, Default)]
struct Exceptions {
    hosts: Vec<String>,
    ips: Vec<IpAddr>,
}

impl Exceptions {
    fn new(entries: &[String]) -> Self {
        let mut out = Self::default();
        for e in entries {
            let e = e.trim();
            let bare = e.trim_start_matches('[').trim_end_matches(']');
            match bare.parse::<IpAddr>() {
                Ok(ip) => out.ips.push(canonical_ip(ip)),
                Err(_) => out.hosts.push(e.trim_end_matches('.').to_ascii_lowercase()),
            }
        }
        out
    }

    /// 宛先が例外か。ホスト名が例外にあるか、名前解決したアドレスがすべて例外にあるとき。
    fn covers(&self, host: &Host<String>, addrs: &[SocketAddr]) -> bool {
        if let Host::Domain(d) = host
            && self
                .hosts
                .iter()
                .any(|h| h == &d.trim_end_matches('.').to_ascii_lowercase())
        {
            return true;
        }
        !addrs.is_empty()
            && addrs
                .iter()
                .all(|a| self.ips.contains(&canonical_ip(a.ip())))
    }
}

/// IPv4射影のIPv6アドレスはIPv4に直して比べる。
fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// URLを解釈し、スキームとホストとポートを取り出す（名前解決の前の検査）。
fn parse_url(url: &str) -> Result<(Url, bool, Host<String>, u16), EgressError> {
    let parsed = Url::parse(url).map_err(|_| EgressError::InvalidUrl)?;
    let https = match parsed.scheme() {
        "https" => true,
        "http" => false,
        _ => return Err(EgressError::SchemeNotAllowed),
    };
    // 認証情報をURLに書く形は受け付けない（ログや画面に残りやすい）
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(EgressError::InvalidUrl);
    }
    let host = parsed.host().ok_or(EgressError::InvalidUrl)?.to_owned();
    let port = parsed
        .port_or_known_default()
        .ok_or(EgressError::InvalidUrl)?;
    Ok((parsed, https, host, port))
}

/// TLSの設定。標準のCAの一覧（Mozilla）に、設定で足したCAを加える。
fn tls_connector(extra_ca_files: &[std::path::PathBuf]) -> Result<tokio_rustls::TlsConnector> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for path in extra_ca_files {
        let mut added = 0;
        for cert in CertificateDer::pem_file_iter(path)
            .with_context(|| format!("cannot read CA file {}", path.display()))?
        {
            let cert =
                cert.with_context(|| format!("invalid certificate in {}", path.display()))?;
            roots
                .add(cert)
                .with_context(|| format!("unusable CA certificate in {}", path.display()))?;
            added += 1;
        }
        anyhow::ensure!(added > 0, "no certificate in CA file {}", path.display());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("TLS configuration")?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

fn parse_proxy(url: &str) -> Result<(String, u16)> {
    let parsed = Url::parse(url).context("`egress.proxy.url` is not a valid URL")?;
    anyhow::ensure!(
        parsed.scheme() == "http",
        "`egress.proxy.url` must be an http:// URL"
    );
    anyhow::ensure!(
        parsed.username().is_empty() && parsed.password().is_none(),
        "`egress.proxy.url` must not contain credentials"
    );
    anyhow::ensure!(
        matches!(parsed.path(), "" | "/") && parsed.query().is_none(),
        "`egress.proxy.url` must not have a path or query"
    );
    let host = match parsed.host().context("`egress.proxy.url` has no host")? {
        Host::Domain(d) => d.to_owned(),
        Host::Ipv4(ip) => ip.to_string(),
        Host::Ipv6(ip) => ip.to_string(),
    };
    let port = parsed
        .port_or_known_default()
        .context("`egress.proxy.url` has no port")?;
    Ok((host, port))
}

/// `host:port` の書式（IPv6は角括弧で囲む）。
fn authority(host: &Host<String>, port: u16) -> String {
    match host {
        Host::Domain(d) => format!("{d}:{port}"),
        Host::Ipv4(ip) => format!("{ip}:{port}"),
        Host::Ipv6(ip) => format!("[{ip}]:{port}"),
    }
}

fn socket_authority(addr: SocketAddr) -> String {
    match addr {
        SocketAddr::V4(a) => a.to_string(),
        SocketAddr::V6(a) => format!("[{}]:{}", a.ip(), a.port()),
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

impl Egress {
    /// 設定から作る。CAのファイルを読めない、プロキシのURLが不正、などは起動時の検査で止める。
    pub fn from_config(config: &EgressConfig) -> Result<Self> {
        let proxy = match &config.proxy {
            None => None,
            Some(p) => {
                let (host, port) = parse_proxy(&p.url)?;
                Some(Proxy {
                    host,
                    port,
                    connect_by: p.connect_by,
                })
            }
        };
        Ok(Self {
            inner: Arc::new(Inner {
                exceptions: Exceptions::new(&config.internal_destination_exceptions),
                tls: tls_connector(&config.extra_ca_files)?,
                connect_timeout: Duration::from_secs(config.connect_timeout_seconds.max(1).into()),
                request_timeout: Duration::from_secs(config.request_timeout_seconds.max(1).into()),
                proxy,
            }),
        })
    }

    /// 送信先を名前解決して検査する（登録時の確認にも使う）。送信のたびにやり直す。
    pub async fn check(&self, url: &str) -> Result<Destination, EgressError> {
        let (_, https, host, port) = parse_url(url)?;
        tokio::time::timeout(
            self.inner.connect_timeout,
            self.resolve_and_check(https, host, port),
        )
        .await
        .map_err(|_| EgressError::Timeout)?
    }

    async fn resolve_and_check(
        &self,
        https: bool,
        host: Host<String>,
        port: u16,
    ) -> Result<Destination, EgressError> {
        let addrs: Vec<SocketAddr> = match &host {
            Host::Ipv4(ip) => vec![SocketAddr::new(IpAddr::V4(*ip), port)],
            Host::Ipv6(ip) => vec![SocketAddr::new(IpAddr::V6(*ip), port)],
            Host::Domain(d) => {
                let mut out: Vec<SocketAddr> = tokio::net::lookup_host((d.as_str(), port))
                    .await
                    .map_err(|_| EgressError::DnsFailed)?
                    .collect();
                out.dedup();
                out
            }
        };
        if addrs.is_empty() {
            return Err(EgressError::DnsFailed);
        }
        let excepted = self.inner.exceptions.covers(&host, &addrs);
        if !excepted {
            if !https {
                return Err(EgressError::SchemeNotAllowed);
            }
            if addrs.iter().any(|a| address::is_internal(a.ip())) {
                return Err(EgressError::InternalDestination);
            }
        }
        Ok(Destination {
            https,
            host,
            port,
            addrs,
        })
    }

    /// 送る。名前解決と検査を行い、検査したアドレスに固定して接続する。リダイレクトはたどらない。
    pub async fn send(&self, request: EgressRequest) -> Result<EgressResponse, EgressError> {
        let (url, ..) = parse_url(&request.url)?;
        tokio::time::timeout(self.inner.request_timeout, async {
            let dest = self.check(&request.url).await?;
            let io = self.open(&dest).await?;
            // プロキシ経由のHTTP（CONNECTを使わない中継）では、リクエストの宛先を絶対形式で書く
            let absolute_target = match &self.inner.proxy {
                Some(proxy) if !dest.https => Some(match proxy.connect_by {
                    ProxyConnectBy::Ip => socket_authority(dest.addrs[0]),
                    ProxyConnectBy::Hostname => authority(&dest.host, dest.port),
                }),
                _ => None,
            };
            exchange(io, &dest, &url, request, absolute_target).await
        })
        .await
        .map_err(|_| EgressError::Timeout)?
    }

    /// 検査したアドレスへの接続（必要ならプロキシ経由、TLS）を作る。
    async fn open(&self, dest: &Destination) -> Result<Box<dyn Io>, EgressError> {
        let timeout = self.inner.connect_timeout;
        let tcp: Box<dyn Io> = match &self.inner.proxy {
            None => Box::new(connect_any(&dest.addrs, timeout).await?),
            Some(proxy) => {
                let mut stream = connect_proxy(proxy, timeout).await?;
                if dest.https {
                    // HTTPSは中継の依頼（CONNECT）で通す。宛先は既定で、検査したIPアドレス
                    let target = match proxy.connect_by {
                        ProxyConnectBy::Ip => socket_authority(dest.addrs[0]),
                        ProxyConnectBy::Hostname => authority(&dest.host, dest.port),
                    };
                    tokio::time::timeout(timeout, proxy_connect(&mut stream, &target))
                        .await
                        .map_err(|_| EgressError::Timeout)??;
                }
                Box::new(stream)
            }
        };
        if !dest.https {
            return Ok(tcp);
        }
        let name = match &dest.host {
            Host::Domain(d) => {
                ServerName::try_from(d.clone()).map_err(|_| EgressError::InvalidUrl)?
            }
            Host::Ipv4(ip) => ServerName::IpAddress(IpAddr::V4(*ip).into()),
            Host::Ipv6(ip) => ServerName::IpAddress(IpAddr::V6(*ip).into()),
        };
        let tls = tokio::time::timeout(timeout, self.inner.tls.connect(name, tcp))
            .await
            .map_err(|_| EgressError::Timeout)?
            .map_err(|_| EgressError::TlsFailed)?;
        Ok(Box::new(tls))
    }
}

/// 検査したアドレスに順に接続を試みる。
async fn connect_any(addrs: &[SocketAddr], timeout: Duration) -> Result<TcpStream, EgressError> {
    for addr in addrs {
        if let Ok(Ok(stream)) = tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
            return Ok(stream);
        }
    }
    Err(EgressError::ConnectFailed)
}

/// 出口のプロキシに接続する（プロキシ自体はサーバー管理者が指定した宛先なので、内部向けかどうかは検査しない）。
async fn connect_proxy(proxy: &Proxy, timeout: Duration) -> Result<TcpStream, EgressError> {
    let addrs: Vec<SocketAddr> = tokio::time::timeout(
        timeout,
        tokio::net::lookup_host((proxy.host.as_str(), proxy.port)),
    )
    .await
    .map_err(|_| EgressError::Timeout)?
    .map_err(|_| EgressError::ProxyFailed)?
    .collect();
    connect_any(&addrs, timeout)
        .await
        .map_err(|_| EgressError::ProxyFailed)
}

/// 中継の依頼（CONNECT）を送り、成功の応答を待つ。応答の先は読みすぎないよう1バイトずつ読む。
async fn proxy_connect(stream: &mut TcpStream, target: &str) -> Result<(), EgressError> {
    let req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|_| EgressError::ProxyFailed)?;
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= 8192 {
            return Err(EgressError::ProxyFailed);
        }
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|_| EgressError::ProxyFailed)?;
        if n == 0 {
            return Err(EgressError::ProxyFailed);
        }
        head.push(byte[0]);
    }
    let status_line = head.split(|&b| b == b'\n').next().unwrap_or(&[]);
    let mut parts = status_line.split(|&b| b == b' ');
    let version = parts.next().unwrap_or(&[]);
    let status = parts.next().unwrap_or(&[]);
    if version.starts_with(b"HTTP/1.") && status.len() == 3 && status[0] == b'2' {
        Ok(())
    } else {
        Err(EgressError::ProxyFailed)
    }
}

/// 接続の上でHTTP/1.1のリクエストを1つ送り、応答を受け取る。
/// `absolute_target` はプロキシ経由のHTTP（CONNECTを使わない中継）のときの宛先（既定では検査したIPアドレス）。
async fn exchange(
    io: Box<dyn Io>,
    dest: &Destination,
    url: &Url,
    request: EgressRequest,
    absolute_target: Option<String>,
) -> Result<EgressResponse, EgressError> {
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .map_err(|_| EgressError::HttpFailed)?;
    let conn = tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut path = url.path().to_owned();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    let uri = match absolute_target {
        Some(target) => format!("http://{target}{path}"),
        None => path,
    };
    let default_port = if dest.https { 443 } else { 80 };
    let host_header = if dest.port == default_port {
        match &dest.host {
            Host::Ipv6(ip) => format!("[{ip}]"),
            h => h.to_string(),
        }
    } else {
        authority(&dest.host, dest.port)
    };
    let mut builder = hyper::Request::builder()
        .method(request.method)
        .uri(uri)
        .header(hyper::header::HOST, host_header);
    for (name, value) in request.headers {
        builder = builder.header(name, value);
    }
    let req = builder
        .body(Full::new(Bytes::from(request.body)))
        .map_err(|_| EgressError::InvalidUrl)?;
    let res = sender
        .send_request(req)
        .await
        .map_err(|_| EgressError::HttpFailed)?;
    let status = res.status().as_u16();
    let body = match request.read_body_limit {
        None => None,
        Some(limit) => {
            let collected = Limited::new(res.into_body(), limit)
                .collect()
                .await
                .map_err(|e| {
                    if e.is::<http_body_util::LengthLimitError>() {
                        EgressError::ResponseTooLarge
                    } else {
                        EgressError::HttpFailed
                    }
                })?;
            Some(collected.to_bytes().to_vec())
        }
    };
    conn.abort();
    Ok(EgressResponse { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn egress(exceptions: &[&str]) -> Egress {
        Egress::from_config(&EgressConfig {
            internal_destination_exceptions: exceptions.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        })
        .unwrap()
    }

    #[tokio::test]
    async fn rejects_internal_and_plain_http() {
        let e = egress(&[]);
        for (url, err) in [
            ("https://127.0.0.1/", EgressError::InternalDestination),
            ("https://[::1]/", EgressError::InternalDestination),
            (
                "https://[::ffff:127.0.0.1]/",
                EgressError::InternalDestination,
            ),
            ("https://0x7f.1/", EgressError::InternalDestination),
            (
                "https://169.254.169.254/latest/meta-data",
                EgressError::InternalDestination,
            ),
            ("https://localhost/", EgressError::InternalDestination),
            ("http://8.8.8.8/", EgressError::SchemeNotAllowed),
            ("ftp://8.8.8.8/", EgressError::SchemeNotAllowed),
            ("https://user:pw@8.8.8.8/", EgressError::InvalidUrl),
            ("not a url", EgressError::InvalidUrl),
        ] {
            assert_eq!(e.check(url).await.unwrap_err(), err, "{url}");
        }
        assert!(e.check("https://8.8.8.8/").await.is_ok());
    }

    #[tokio::test]
    async fn exceptions_allow_internal_and_http() {
        let e = egress(&["127.0.0.2", "LocalHost"]);
        assert!(e.check("http://127.0.0.2:8080/").await.is_ok());
        assert!(e.check("http://[::ffff:127.0.0.2]:8080/").await.is_ok());
        assert!(e.check("http://localhost:8080/").await.is_ok());
        assert_eq!(
            e.check("http://127.0.0.3/").await.unwrap_err(),
            EgressError::SchemeNotAllowed
        );
        assert_eq!(
            e.check("https://127.0.0.3/").await.unwrap_err(),
            EgressError::InternalDestination
        );
    }

    #[test]
    fn proxy_url_rules() {
        assert!(parse_proxy("http://proxy.example:3128").is_ok());
        assert!(parse_proxy("https://proxy.example:3128").is_err());
        assert!(parse_proxy("http://u:p@proxy.example:3128").is_err());
        assert!(parse_proxy("http://proxy.example:3128/x").is_err());
    }
}
