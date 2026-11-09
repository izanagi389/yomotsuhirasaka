//! 接続の受け付け（13-security.md「入口の上限」）。
//!
//! `axum::serve` はヘッダーの読み取りの時間の上限を設定できないので、自分で受け付ける。ヘッダーをゆっくり送り続ける接続が、
//! いつまでも残らないようにする。リバースプロキシとは別に、サーバー自身でも上限を設ける。
//!
//! 止めるときは、新しい接続を受け付けず、処理中のリクエストが終わるのを待つ。

use std::future::Future;
use std::net::SocketAddr;
use std::pin::pin;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::{Service as _, service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

/// ヘッダーを読み終えるまでの時間の上限。
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 接続を受け付けられなかったとき（ファイル記述子が足りないなど）に待つ時間。
const ACCEPT_RETRY: Duration = Duration::from_secs(1);

/// `shutdown` が終わるまで、接続を受け付けて `app` で処理する。
pub async fn run(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()>,
    header_read_timeout: Duration,
) -> std::io::Result<()> {
    let (stop_tx, stop_rx) = watch::channel(());
    // すべての接続のタスクが送信側を手放したら、終わったと分かる
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);
    let mut shutdown = pin!(shutdown);
    loop {
        let accepted = tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => accepted,
        };
        let (stream, addr) = match accepted {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!("cannot accept a connection: {e}");
                tokio::time::sleep(ACCEPT_RETRY).await;
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        tokio::spawn(connection(
            stream,
            addr,
            app.clone(),
            header_read_timeout,
            stop_rx.clone(),
            done_tx.clone(),
        ));
    }
    drop(listener);
    drop(stop_rx);
    drop(done_tx);
    let _ = stop_tx.send(());
    let _ = done_rx.recv().await;
    Ok(())
}

async fn connection(
    stream: tokio::net::TcpStream,
    addr: SocketAddr,
    app: Router,
    header_read_timeout: Duration,
    mut stop: watch::Receiver<()>,
    _done: mpsc::Sender<()>,
) {
    let router = TowerToHyperService::new(app);
    let service = service_fn(move |mut req: hyper::Request<Incoming>| {
        // 接続元のIPアドレス（`ClientIp`）に使う
        req.extensions_mut().insert(ConnectInfo(addr));
        router.call(req)
    });
    let conn = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    let mut conn = pin!(conn);
    tokio::select! {
        _ = conn.as_mut() => {}
        _ = stop.changed() => {
            conn.as_mut().graceful_shutdown();
            let _ = conn.as_mut().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::routing::get;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    async fn server(
        timeout: Duration,
    ) -> (
        SocketAddr,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let app = Router::new().route(
            "/ip",
            get(|ConnectInfo(a): ConnectInfo<SocketAddr>| async move { a.ip().to_string() }),
        );
        let task = tokio::spawn(async move {
            run(
                listener,
                app,
                async {
                    let _ = rx.await;
                },
                timeout,
            )
            .await
            .unwrap();
        });
        (addr, tx, task)
    }

    const REQUEST: &[u8] = b"GET /ip HTTP/1.1\r\nHost: x\r\n\r\n";

    async fn read_some(s: &mut tokio::net::TcpStream) -> String {
        let mut buf = vec![0; 1024];
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    #[tokio::test]
    async fn serves_requests_with_the_client_address_over_one_connection() {
        let (addr, _stop, _task) = server(Duration::from_secs(5)).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        for _ in 0..2 {
            s.write_all(REQUEST).await.unwrap();
            let res = read_some(&mut s).await;
            assert!(res.starts_with("HTTP/1.1 200"), "{res}");
            assert!(res.ends_with("127.0.0.1"), "{res}");
        }
    }

    #[tokio::test]
    async fn a_client_that_never_finishes_its_headers_is_cut_off() {
        let (addr, _stop, _task) = server(Duration::from_millis(200)).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /ip HTTP/1.1\r\nHost:").await.unwrap();
        // 上限を過ぎると、応答（408 か切断）のあとで接続が閉じる
        let mut all = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut all))
            .await
            .expect("the connection must be closed")
            .ok();
    }

    #[tokio::test]
    async fn stopping_waits_for_open_connections_and_refuses_new_ones() {
        let (addr, stop, task) = server(Duration::from_secs(5)).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(REQUEST).await.unwrap();
        assert!(read_some(&mut s).await.starts_with("HTTP/1.1 200"));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("idle connections must not block the stop")
            .unwrap();
        assert!(tokio::net::TcpStream::connect(addr).await.is_err());
    }
}
