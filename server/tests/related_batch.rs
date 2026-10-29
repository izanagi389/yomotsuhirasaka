//! 関連記事の一括計算の結合テスト（毎日1回だけ積むこと、今すぐの再計算の管理API、Webhookの分け方）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。関連記事の中身の計算（pgvector）は、
//! 約束のテスト（INV-REL-01・INV-REL-02）で確かめる。

mod common;

use std::sync::{Arc, Mutex};

use cms_server::search::batch;
use cms_server::webhooks::{self, EntryRef};
use cms_server::worker;
use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use uuid::Uuid;

/// 届いたリクエストの本文を記録し、200 を返す受け手。
async fn receiver() -> (u16, Arc<Mutex<Vec<String>>>) {
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
                let mut got = Vec::new();
                let mut buf = vec![0u8; 65536];
                loop {
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got);
                    if let Some(at) = text.find("\r\n\r\n") {
                        let len = text[..at]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if got.len() >= at + 4 + len {
                            break;
                        }
                    }
                }
                let raw = String::from_utf8_lossy(&got).into_owned();
                let body = raw
                    .split_once("\r\n\r\n")
                    .map(|(_, b)| b.to_owned())
                    .unwrap_or_default();
                log.lock().unwrap().push(body);
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    )
                    .await;
            });
        }
    });
    (port, seen)
}

async fn queued_batches(pool: &PgPool, ws: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM cms.jobs WHERE dedupe_key = $1")
        .bind(format!("{}:{ws}", batch::JOB_KIND))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_daily_batch_is_queued_once_per_day_and_catches_up() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let keys = &server.state.keys;

        // 時刻 0 時（UTC）なら、今日の分はもう予定の時刻を過ぎている。1回目は積み、同じ日の2回目は積まない
        assert_eq!(
            batch::schedule_due(&server.pool, keys, 0).await.unwrap(),
            vec![ws]
        );
        assert_eq!(
            batch::schedule_due(&server.pool, keys, 0).await.unwrap(),
            Vec::<Uuid>::new()
        );
        assert_eq!(queued_batches(&server.pool, ws).await, 1);
        // ジョブを実行する（LLMの接続先がないので、計算するものはなく、そのまま終わる）
        while worker::run_once(&server.state).await.unwrap() {}
        assert_eq!(queued_batches(&server.pool, ws).await, 0);
        assert_eq!(
            batch::schedule_due(&server.pool, keys, 0).await.unwrap(),
            Vec::<Uuid>::new()
        );

        // サーバーが止まっていて逃した日があっても、次に見たときに1回だけ積む
        sqlx::query(
            "UPDATE cms.related_batches SET last_run_on = last_run_on - 3 WHERE workspace_id = $1",
        )
        .bind(ws)
        .execute(&server.pool)
        .await
        .unwrap();
        assert_eq!(
            batch::schedule_due(&server.pool, keys, 0).await.unwrap(),
            vec![ws]
        );
        assert_eq!(
            batch::schedule_due(&server.pool, keys, 0).await.unwrap(),
            Vec::<Uuid>::new()
        );
        assert_eq!(queued_batches(&server.pool, ws).await, 1);

        // まだ積んだジョブが残っているときに日が変わっても、ジョブは増えない
        sqlx::query(
            "UPDATE cms.related_batches SET last_run_on = last_run_on - 1 WHERE workspace_id = $1",
        )
        .bind(ws)
        .execute(&server.pool)
        .await
        .unwrap();
        assert_eq!(
            batch::schedule_due(&server.pool, keys, 0).await.unwrap(),
            vec![ws]
        );
        assert_eq!(queued_batches(&server.pool, ws).await, 1);
    })
    .await;
}

#[tokio::test]
async fn admins_can_start_a_recomputation_now() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        for (email, role) in [
            ("admin@example.test", "admin"),
            ("editor@example.test", "editor"),
        ] {
            create_user(&server.pool, email, false).await;
            add_member(&server, ws, email, role).await;
        }
        let admin = login(&server, "admin@example.test", PASSWORD).await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let path = format!("/api/admin/workspaces/{ws}/related/recompute");
        let audits = || async {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM cms.audit_log WHERE action = 'related.recompute_started'",
            )
            .fetch_one(&server.pool)
            .await
            .unwrap()
        };

        // 編集者は始められない
        assert_eq!(
            send(&server, "POST", &path, &editor, None).await.status,
            403
        );
        assert_eq!(queued_batches(&server.pool, ws).await, 0);
        // 管理者は始められる。待っている一括計算があれば、新しく積まない
        let res = send(&server, "POST", &path, &admin, None).await;
        assert_eq!((res.status, res.body.as_str()), (202, r#"{"queued":true}"#));
        let res = send(&server, "POST", &path, &admin, None).await;
        assert_eq!(
            (res.status, res.body.as_str()),
            (202, r#"{"queued":false}"#)
        );
        assert_eq!(queued_batches(&server.pool, ws).await, 1);
        // 監査ログは、公開の操作と同じく、サーバー管理者が操作したときだけ残す
        assert_eq!(audits().await, 0);
        while worker::run_once(&server.state).await.unwrap() {}
        let res = send(&server, "POST", &path, &root, None).await;
        assert_eq!((res.status, res.body.as_str()), (202, r#"{"queued":true}"#));
        assert_eq!(audits().await, 1);
        // 今すぐの再計算は、毎日の分の記録を書き換えない
        let recorded: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.related_batches")
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(recorded, 0);
    })
    .await;
}

#[tokio::test]
async fn changed_entries_are_sent_in_parts_of_one_hundred() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.egress.internal_destination_exceptions = vec!["127.0.0.1".to_owned()];
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let quiet = workspace_with_template(&server, &root, "B", "blog").await;
        let (port, seen) = receiver().await;
        let hook = json!({"url": format!("http://127.0.0.1:{port}/hook")}).to_string();
        let res = send(
            &server,
            "POST",
            &format!("/api/admin/workspaces/{ws}/webhooks"),
            &root,
            Some(&hook),
        )
        .await;
        assert_eq!(res.status, 201, "{}", res.body);

        let refs: Vec<EntryRef> = (0..250)
            .map(|i| EntryRef {
                id: Uuid::new_v4(),
                translation_group_id: Uuid::new_v4(),
                locale: "ja".into(),
                url: (i % 2 == 0).then(|| format!("/posts/{i}")),
            })
            .collect();
        webhooks::emit_related_changed(&server.pool, &server.state.keys, ws, &refs)
            .await
            .unwrap();
        // 送信先のないワークスペースには、送信の記録もジョブも作らない
        webhooks::emit_related_changed(&server.pool, &server.state.keys, quiet, &refs)
            .await
            .unwrap();
        let quiet_deliveries: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM cms.webhook_deliveries WHERE workspace_id = $1",
        )
        .bind(quiet)
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(quiet_deliveries, 0);
        while worker::run_once(&server.state).await.unwrap() {}

        let mut bodies: Vec<Value> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|b| serde_json::from_str(b).unwrap())
            .collect();
        bodies.sort_by_key(|b| b["part"].as_u64());
        assert_eq!(bodies.len(), 3);
        let run = bodies[0]["runId"].as_str().unwrap().to_owned();
        let mut sent = Vec::new();
        for (i, (b, n)) in bodies.iter().zip([100, 100, 50]).enumerate() {
            assert_eq!(b["type"], "entries.related_changed");
            assert_eq!(b["workspaceId"], ws.to_string());
            assert_eq!(b["runId"], run.as_str());
            assert_eq!(
                (b["part"].as_u64(), b["parts"].as_u64()),
                (Some(i as u64 + 1), Some(3))
            );
            let entries = b["entries"].as_array().unwrap();
            assert_eq!(entries.len(), n);
            for e in entries {
                // 記事の参照だけを含める（INV-WH-07）
                let keys: Vec<&str> = e.as_object().unwrap().keys().map(String::as_str).collect();
                assert!(
                    keys.iter()
                        .all(|k| ["entryId", "translationGroupId", "locale", "url"].contains(k)),
                    "{keys:?}"
                );
                sent.push(e["entryId"].as_str().unwrap().to_owned());
            }
            let top: Vec<&str> = b.as_object().unwrap().keys().map(String::as_str).collect();
            assert!(
                top.iter().all(
                    |k| ["type", "workspaceId", "runId", "part", "parts", "entries"].contains(k)
                ),
                "{top:?}"
            );
        }
        let expected: Vec<String> = refs.iter().map(|r| r.id.to_string()).collect();
        assert_eq!(sent, expected);
    })
    .await;
}
