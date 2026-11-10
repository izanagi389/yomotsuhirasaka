//! 組み込みJSの時間の上限が、ホスト関数（`yomi.http.fetch`）の応答を待っている間も効くことの結合テスト
//! （侵入テストの資料の「動かさないと分からないこと」）。手元で待ち受ける、わざと遅い相手に送る。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use std::time::{Duration, Instant};

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use yomi_server::config::EgressConfig;
use yomi_server::egress::Egress;
use yomi_server::script::{Engine, Host, Limits, Permission, Script};
use yomi_server::workspace_db;

/// リクエストを読んだ後、30秒待ってから応答する相手。
async fn slow_peer() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = s.read(&mut buf).await;
                tokio::time::sleep(Duration::from_secs(30)).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    )
                    .await;
            });
        }
    });
    port
}

#[tokio::test]
async fn the_time_limit_covers_waiting_for_a_host_function() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let db = workspace_db::lookup(&server.pool, ws).await.unwrap().unwrap();
        let port = slow_peer().await;
        let egress = Egress::from_config(&EgressConfig {
            internal_destination_exceptions: vec!["127.0.0.1".to_owned()],
            ..Default::default()
        })
        .unwrap();
        let engine = Engine::new(Limits {
            memory_bytes: 16 * 1024 * 1024,
            time: Duration::from_secs(1),
            async_time: Duration::from_secs(1),
        });
        let script = Script {
            source: format!(
                "export default function () {{ yomi.http.fetch('http://127.0.0.1:{port}/slow', {{ method: 'POST', body: '{{}}' }}); }}"
            ),
            permissions: vec![Permission::HttpFetch],
            allowed_hosts: vec!["127.0.0.1".to_owned()],
            secrets: vec![],
        };
        let host = Host::new(server.pool.clone(), db).with_fetch(egress, &script.allowed_hosts);
        let started = Instant::now();
        let result = engine.after(&script, host, &json!({})).await;
        let elapsed = started.elapsed();
        assert!(result.is_err(), "the hook finished although the peer never answered in time");
        // 上限（1秒）で止まる。相手の応答（30秒）までは待たない
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    })
    .await;
}
