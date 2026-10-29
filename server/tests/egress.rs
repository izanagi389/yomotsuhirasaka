//! 外部への送信の共通の仕組み（13-security.md「外部への送信」）の結合テスト。
//! 手元で待ち受ける相手（127.0.0.1）を例外として許し、実際に送って確かめる。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cms_server::config::{EgressConfig, EgressProxyConfig, ProxyConnectBy};
use cms_server::egress::{Egress, EgressError, EgressRequest, StreamLimits};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

/// 届いたリクエストの先頭（リクエスト行とヘッダー）を記録し、決めた応答を返す相手。
async fn peer(response: &'static str) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&got).into_owned());
                let _ = s.write_all(response.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    (port, seen)
}

fn egress(config: EgressConfig) -> Egress {
    Egress::from_config(&config).unwrap()
}

fn allow_local() -> EgressConfig {
    EgressConfig {
        internal_destination_exceptions: vec!["127.0.0.1".to_owned()],
        ..Default::default()
    }
}

fn post(url: String, read_body_limit: Option<usize>) -> EgressRequest {
    EgressRequest {
        method: hyper::Method::POST,
        url,
        headers: vec![(
            hyper::header::CONTENT_TYPE,
            hyper::header::HeaderValue::from_static("application/json"),
        )],
        body: b"{\"a\":1}".to_vec(),
        read_body_limit,
    }
}

#[tokio::test]
async fn sends_to_excepted_destination_and_reads_status_only() {
    let (port, seen) =
        peer("HTTP/1.1 202 Accepted\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello").await;
    let res = egress(allow_local())
        .send(post(format!("http://127.0.0.1:{port}/hook?x=1"), None))
        .await
        .unwrap();
    assert_eq!(res.status, 202);
    assert!(res.body.is_none());
    let req = seen.lock().unwrap()[0].clone();
    assert!(req.starts_with("POST /hook?x=1 HTTP/1.1\r\n"), "{req}");
    assert!(
        req.to_ascii_lowercase()
            .contains(&format!("host: 127.0.0.1:{port}")),
        "{req}"
    );
}

#[tokio::test]
async fn body_is_read_only_within_the_limit() {
    let (port, _) =
        peer("HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello").await;
    let e = egress(allow_local());
    let ok = e
        .send(post(format!("http://127.0.0.1:{port}/"), Some(5)))
        .await
        .unwrap();
    assert_eq!(ok.body.as_deref(), Some(&b"hello"[..]));
    let err = e
        .send(post(format!("http://127.0.0.1:{port}/"), Some(4)))
        .await
        .unwrap_err();
    assert_eq!(err, EgressError::ResponseTooLarge);
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let (target_port, target_seen) = peer("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await;
    let location: &'static str = Box::leak(
        format!("HTTP/1.1 307 Temporary Redirect\r\nlocation: http://127.0.0.1:{target_port}/final\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .into_boxed_str(),
    );
    let (port, _) = peer(location).await;
    let res = egress(allow_local())
        .send(post(format!("http://127.0.0.1:{port}/"), None))
        .await
        .unwrap();
    assert_eq!(res.status, 307);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(target_seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_destinations_are_not_contacted() {
    let (port, seen) = peer("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await;
    // 例外がなければ、HTTPSでもHTTPでも送らない
    let e = egress(EgressConfig::default());
    for url in [
        format!("https://127.0.0.1:{port}/"),
        format!("http://127.0.0.1:{port}/"),
        format!("https://localhost:{port}/"),
    ] {
        let err = e.send(post(url.clone(), None)).await.unwrap_err();
        assert!(
            matches!(
                err,
                EgressError::InternalDestination | EgressError::SchemeNotAllowed
            ),
            "{url}: {err}"
        );
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn proxy_is_asked_for_the_checked_ip_address() {
    // プロキシの代わり: 依頼を記録し、CONNECT には成功を返して閉じる（その後のTLSは失敗する）
    let (proxy_port, proxy_seen) = peer("HTTP/1.1 200 Connection Established\r\n\r\n").await;
    let config = |connect_by| EgressConfig {
        internal_destination_exceptions: vec!["localhost".to_owned(), "127.0.0.1".to_owned()],
        proxy: Some(EgressProxyConfig {
            url: format!("http://127.0.0.1:{proxy_port}"),
            connect_by,
        }),
        ..Default::default()
    };
    let e = egress(config(ProxyConnectBy::Ip));
    let err = e
        .send(post("https://localhost:4443/x".to_owned(), None))
        .await
        .unwrap_err();
    assert_eq!(err, EgressError::TlsFailed);
    let first = proxy_seen.lock().unwrap()[0].clone();
    assert!(
        first.starts_with("CONNECT 127.0.0.1:4443 HTTP/1.1")
            || first.starts_with("CONNECT [::1]:4443 HTTP/1.1"),
        "{first}"
    );

    // HTTP は絶対形式で、宛先は検査したIPアドレス
    let _ = e
        .send(post("http://localhost:8080/y".to_owned(), None))
        .await;
    let second = proxy_seen.lock().unwrap()[1].clone();
    assert!(
        second.starts_with("POST http://127.0.0.1:8080/y HTTP/1.1")
            || second.starts_with("POST http://[::1]:8080/y HTTP/1.1"),
        "{second}"
    );

    // ホスト名の方式に切り替えたときだけ、ホスト名で依頼する
    let e = egress(config(ProxyConnectBy::Hostname));
    let _ = e
        .send(post("https://localhost:4443/x".to_owned(), None))
        .await;
    let third = proxy_seen.lock().unwrap()[2].clone();
    assert!(
        third.starts_with("CONNECT localhost:4443 HTTP/1.1"),
        "{third}"
    );
}

#[tokio::test]
async fn internal_destinations_are_checked_before_the_proxy() {
    let (proxy_port, proxy_seen) = peer("HTTP/1.1 200 Connection Established\r\n\r\n").await;
    let e = egress(EgressConfig {
        proxy: Some(EgressProxyConfig {
            url: format!("http://127.0.0.1:{proxy_port}"),
            connect_by: ProxyConnectBy::Ip,
        }),
        ..Default::default()
    });
    let err = e
        .send(post("https://localhost:4443/x".to_owned(), None))
        .await
        .unwrap_err();
    assert_eq!(err, EgressError::InternalDestination);
    assert!(proxy_seen.lock().unwrap().is_empty());
}

#[test]
fn unreadable_ca_file_is_an_error() {
    let err = Egress::from_config(&EgressConfig {
        extra_ca_files: vec!["/nonexistent/ca.pem".into()],
        ..Default::default()
    });
    assert!(err.is_err());
}

// --- 本文を少しずつ受け取る送り方（send_streaming）---

/// 応答の先頭を `head_delay` 待ってから返し、本文を chunked で、決めた間隔をおいて1つずつ返す相手。
async fn streaming_peer(head_delay: Duration, chunks: Vec<(Duration, &'static str)>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let chunks = chunks.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                tokio::time::sleep(head_delay).await;
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
                if s.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for (delay, data) in chunks {
                    tokio::time::sleep(delay).await;
                    let frame = format!("{:x}\r\n{data}\r\n", data.len());
                    if s.write_all(frame.as_bytes()).await.is_err() {
                        return;
                    }
                }
                let _ = s.write_all(b"0\r\n\r\n").await;
                let _ = s.shutdown().await;
            });
        }
    });
    port
}

fn limits() -> StreamLimits {
    StreamLimits {
        first_byte: Duration::from_secs(5),
        idle: Duration::from_secs(5),
        total: Duration::from_secs(10),
        max_body_bytes: 1024,
    }
}

/// 本文を終わりまで読む。途中で失敗したら、それまでに読んだものと失敗の理由を返す。
async fn read_all(
    stream: &mut cms_server::egress::EgressStream,
) -> (Vec<String>, Option<EgressError>) {
    let mut out = Vec::new();
    loop {
        match stream.next_chunk().await {
            Ok(Some(b)) => out.push(String::from_utf8(b.to_vec()).unwrap()),
            Ok(None) => return (out, None),
            Err(e) => return (out, Some(e)),
        }
    }
}

#[tokio::test]
async fn streaming_returns_chunks_as_they_arrive() {
    let ms = Duration::from_millis;
    let port = streaming_peer(
        ms(0),
        vec![
            (ms(0), "data: a\n\n"),
            (ms(100), "data: b\n\n"),
            (ms(100), "data: c\n\n"),
        ],
    )
    .await;
    let mut stream = egress(allow_local())
        .send_streaming(
            post(format!("http://127.0.0.1:{port}/v1/chat"), None),
            limits(),
        )
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    let (chunks, err) = read_all(&mut stream).await;
    assert_eq!(err, None);
    assert_eq!(chunks.concat(), "data: a\n\ndata: b\n\ndata: c\n\n");
    // 終わった後は、何度読んでも None
    assert!(stream.next_chunk().await.unwrap().is_none());
}

#[tokio::test]
async fn streaming_stops_when_the_first_byte_is_late() {
    let ms = Duration::from_millis;
    let port = streaming_peer(ms(800), vec![(ms(0), "data: a\n\n")]).await;
    let err = egress(allow_local())
        .send_streaming(
            post(format!("http://127.0.0.1:{port}/"), None),
            StreamLimits {
                first_byte: ms(200),
                ..limits()
            },
        )
        .await
        .err()
        .unwrap();
    assert_eq!(err, EgressError::Timeout);
}

#[tokio::test]
async fn streaming_stops_when_data_stops_arriving() {
    let ms = Duration::from_millis;
    let port = streaming_peer(
        ms(0),
        vec![(ms(0), "data: a\n\n"), (ms(800), "data: b\n\n")],
    )
    .await;
    let mut stream = egress(allow_local())
        .send_streaming(
            post(format!("http://127.0.0.1:{port}/"), None),
            StreamLimits {
                idle: ms(200),
                ..limits()
            },
        )
        .await
        .unwrap();
    let (chunks, err) = read_all(&mut stream).await;
    assert_eq!(chunks, vec!["data: a\n\n"]);
    assert_eq!(err, Some(EgressError::Timeout));
    // 止めた後は読まない
    assert!(stream.next_chunk().await.unwrap().is_none());
}

#[tokio::test]
async fn streaming_stops_at_the_total_time_limit() {
    let ms = Duration::from_millis;
    // データは途切れずに来続けるが、全体の時間の上限で止まる
    let port = streaming_peer(ms(0), (0..30).map(|_| (ms(100), "data: x\n\n")).collect()).await;
    let mut stream = egress(allow_local())
        .send_streaming(
            post(format!("http://127.0.0.1:{port}/"), None),
            StreamLimits {
                idle: ms(1000),
                total: ms(600),
                ..limits()
            },
        )
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let (chunks, err) = read_all(&mut stream).await;
    assert_eq!(err, Some(EgressError::Timeout));
    assert!(chunks.len() < 30, "{}", chunks.len());
    assert!(started.elapsed() < ms(1500), "{:?}", started.elapsed());
}

#[tokio::test]
async fn streaming_stops_when_the_body_is_too_large() {
    let ms = Duration::from_millis;
    let port = streaming_peer(ms(0), vec![(ms(0), "abc"), (ms(50), "def")]).await;
    let mut stream = egress(allow_local())
        .send_streaming(
            post(format!("http://127.0.0.1:{port}/"), None),
            StreamLimits {
                max_body_bytes: 5,
                ..limits()
            },
        )
        .await
        .unwrap();
    let (chunks, err) = read_all(&mut stream).await;
    assert_eq!(chunks, vec!["abc"]);
    assert_eq!(err, Some(EgressError::ResponseTooLarge));
}

#[tokio::test]
async fn streaming_checks_the_destination_like_send() {
    let ms = Duration::from_millis;
    let port = streaming_peer(ms(0), vec![(ms(0), "data: a\n\n")]).await;
    // 例外がなければ、内部向けの宛先にもHTTPにも送らない
    let e = egress(EgressConfig::default());
    for url in [
        format!("http://127.0.0.1:{port}/"),
        format!("https://127.0.0.1:{port}/"),
    ] {
        let err = e
            .send_streaming(post(url.clone(), None), limits())
            .await
            .err()
            .unwrap();
        assert!(
            matches!(
                err,
                EgressError::InternalDestination | EgressError::SchemeNotAllowed
            ),
            "{url}: {err}"
        );
    }
}

#[tokio::test]
async fn streaming_does_not_follow_redirects() {
    let (target_port, target_seen) = peer("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await;
    let location: &'static str = Box::leak(
        format!("HTTP/1.1 307 Temporary Redirect\r\nlocation: http://127.0.0.1:{target_port}/final\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .into_boxed_str(),
    );
    let (port, _) = peer(location).await;
    let mut stream = egress(allow_local())
        .send_streaming(post(format!("http://127.0.0.1:{port}/"), None), limits())
        .await
        .unwrap();
    assert_eq!(stream.status(), 307);
    let (chunks, err) = read_all(&mut stream).await;
    assert_eq!((chunks.len(), err), (0, None));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(target_seen.lock().unwrap().is_empty());
}
