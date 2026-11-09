//! 質問ログの管理API（`/api/admin/workspaces/{ws}/question-log`。M4）の結合テスト。
//! 回答から質問ログへの記録と保存期間（INV-LOG-01, 02）は、約束のテスト `tests/invariants/question-log.test.ts` が確かめる。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};
use yomi_server::answer::log::{self, Outcome};
use yomi_server::workspace_db;

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

#[tokio::test]
async fn question_log_api() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        for (email, role) in [("admin@example.test", "admin"), ("editor@example.test", "editor")] {
            create_user(&server.pool, email, false).await;
            add_member(&server, ws, email, role).await;
        }
        // 管理者の操作には多要素認証が要る
        let admin = login(&server, "admin@example.test", PASSWORD).await;
        let (secret, _) = enroll(&server, &admin).await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let settings = format!("/api/admin/workspaces/{ws}/question-log/settings");
        let view = format!("/api/admin/workspaces/{ws}/question-log");
        let db = workspace_db::lookup(&server.pool, ws).await.unwrap().unwrap();
        let record = |outcome: Outcome, text: Option<&'static str>| {
            let (pool, db) = (&server.pool, &db);
            async move { log::record(pool, db, outcome, "ja", text).await.unwrap() }
        };
        let put = |s: &Login, body: Value| {
            let (server, settings, s) = (&server, settings.clone(), s.clone());
            async move { send(server, "PUT", &settings, &s, Some(&body.to_string())).await }
        };

        // 既定は無効・90日。設定は管理者だけ
        assert_eq!(parse(&get(&server, &settings, &admin).await), json!({"enabled": false, "retentionDays": 90}));
        assert_eq!(get(&server, &settings, &editor).await.status, 403);
        assert_eq!(put(&editor, json!({"enabled": true})).await.status, 403);
        // 無効のあいだは、答えられなかった質問も文を残さない（件数だけ）
        record(Outcome::NoInformation, Some("無効のときの質問")).await;

        // 有効にする（再認証が要る。保存期間は7〜365日）
        expire_reauth(&server.pool, "admin@example.test").await;
        let res = put(&admin, json!({"enabled": true})).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (403, Some("reauth_required")));
        forget_used_steps(&server.pool, "admin@example.test").await;
        let res = send(&server, "POST", "/api/admin/session/reauth", &admin, Some(&format!("{{\"totpCode\":\"{}\"}}", code(&secret, 0)))).await;
        assert_eq!(res.status, 204, "{}", res.body);
        for days in [6, 366] {
            let res = put(&admin, json!({"retentionDays": days})).await;
            assert_eq!((res.status, res.json_str("error").as_deref()), (422, Some("invalid_question_log_settings")), "{days}");
        }
        assert_eq!(parse(&put(&admin, json!({"enabled": true, "retentionDays": 30})).await), json!({"enabled": true, "retentionDays": 30}));

        // 答えられなかった質問だけが、同じ質問ごとにまとまって候補になる。答えた質問・エラーは件数だけ
        record(Outcome::NoInformation, Some("ＡＢＣ とは何ですか")).await;
        record(Outcome::NoInformation, Some("abc  とは何ですか")).await;
        record(Outcome::NoInformation, Some("別の質問")).await;
        record(Outcome::Answered, None).await;
        record(Outcome::Error, None).await;

        // 閲覧は重い権限 viewQuestionLog（管理者にも既定では付かない。INV-PERM-02）と再認証が要り、見るたびに監査ログに残る
        assert_eq!(get(&server, &view, &admin).await.status, 403);
        let admin_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM yomi.users WHERE email = 'admin@example.test'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
        let res = send(&server, "PATCH", &format!("/api/admin/workspaces/{ws}/members/{admin_id}"), &admin, Some(&json!({"grants": ["viewQuestionLog"]}).to_string())).await;
        assert!(res.status < 300, "{}", res.body);
        let v = parse(&get(&server, &view, &admin).await);
        let candidates = v["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 2, "{v}");
        assert_eq!(candidates[0]["count"], 2);
        assert_eq!(candidates[0]["question"], "abc  とは何ですか");
        assert_eq!(candidates[1]["question"], "別の質問");
        assert!(!v.to_string().contains("無効のときの質問"));
        let today = &v["daily"][0];
        assert_eq!((today["answered"].as_i64(), today["noInformation"].as_i64(), today["error"].as_i64()), (Some(1), Some(4), Some(1)));
        let viewed: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'question_log.viewed'")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(viewed, 1);

        // 候補を消す（存在しない候補は 404）
        let id = candidates[1]["id"].as_str().unwrap().to_owned();
        let del = |id: String| {
            let (server, admin) = (&server, &admin);
            async move { send(server, "DELETE", &format!("/api/admin/workspaces/{ws}/question-log/candidates/{id}"), admin, None).await.status }
        };
        assert_eq!(del(id.clone()).await, 204);
        assert_eq!(del(id).await, 404);
        assert_eq!(del("not-a-candidate".into()).await, 404);
        assert_eq!(parse(&get(&server, &view, &admin).await)["candidates"].as_array().unwrap().len(), 1);

        // 無効にしたら、残っている質問文はすぐに消える
        assert_eq!(parse(&put(&admin, json!({"enabled": false})).await)["enabled"], false);
        let left: i64 = {
            let mut tx = workspace_db::begin(&server.pool, &db).await.unwrap();
            let n = sqlx::query_scalar("SELECT count(*) FROM question_log").fetch_one(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
            n
        };
        assert_eq!(left, 0);
    })
    .await;
}

/// 質問の文の件数には上限がある。超えても、日ごとの件数は数える（R31）。
#[tokio::test]
async fn question_text_stops_being_stored_at_the_limit() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let db = workspace_db::lookup(&server.pool, ws).await.unwrap().unwrap();
        let mut tx = workspace_db::begin(&server.pool, &db).await.unwrap();
        log::change_settings(&mut tx, &log::SettingsChange { enabled: Some(true), retention_days: None }).await.unwrap().unwrap();
        sqlx::query("INSERT INTO question_log (id, key, question, locale) SELECT gen_random_uuid(), 'k', 'q', 'ja' FROM generate_series(1, $1)")
            .bind(log::MAX_ROWS)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        log::record(&server.pool, &db, log::Outcome::NoInformation, "ja", Some("もう1件")).await.unwrap();
        let mut tx = workspace_db::begin(&server.pool, &db).await.unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM question_log").fetch_one(&mut *tx).await.unwrap();
        let counted: i64 = sqlx::query_scalar("SELECT COALESCE(sum(count), 0)::bigint FROM question_counts").fetch_one(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!((rows, counted), (log::MAX_ROWS, 1));
    })
    .await;
}
