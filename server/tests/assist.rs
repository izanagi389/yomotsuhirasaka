//! 編集者向けのAI支援の管理API（矛盾の検知・見直しの手がかり・構成案。M4）の入口の結合テスト。LLMの接続先は設定しない
//! （予算と公開の関係 INV-BUD-02 は、約束のテスト `tests/invariants/assist.test.ts` が擬似LLMで確かめる）。
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
async fn assist_api_entrance() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        for (email, role) in [("editor@example.test", "editor"), ("viewer@example.test", "viewer")] {
            create_user(&server.pool, email, false).await;
            add_member(&server, ws, email, role).await;
        }
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let viewer = login(&server, "viewer@example.test", PASSWORD).await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "料金は月額 1,200 円です。"}]}]});
        let res = send(&server, "POST", &entries, &editor, Some(&json!({"contentType": "post", "fields": {"title": "t", "slug": "s", "body": body}}).to_string())).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let id = parse(&res)["id"].as_str().unwrap().to_owned();

        // 矛盾の検知と見直しの手がかり: 記事を編集できる人だけ。まだ始めていなければ notStarted、支援が無効なら disabled
        for kind in ["conflicts", "stale"] {
            let path = format!("{entries}/{id}/assist/{kind}");
            assert_eq!(parse(&get(&server, &path, &editor).await)["state"], "notStarted", "{kind}");
            assert_eq!(get(&server, &path, &viewer).await.status, 403, "{kind}");
            assert_eq!(send(&server, "POST", &path, &viewer, None).await.status, 403, "{kind}");
            let res = send(&server, "POST", &path, &editor, None).await;
            assert_eq!((res.status, parse(&res)["state"].clone()), (202, json!("disabled")), "{kind}");
            assert_eq!(parse(&get(&server, &path, &editor).await)["state"], "disabled", "{kind}");
            // ない記事は 404
            let missing = format!("{entries}/{}/assist/{kind}", uuid::Uuid::new_v4());
            assert_eq!(send(&server, "POST", &missing, &editor, None).await.status, 404, "{kind}");
        }
        // 支援を有効にすると、始めた結果は running になる（LLM の接続先がないので、ジョブは失敗で終わる）
        let res = send(&server, "PUT", &format!("/api/admin/workspaces/{ws}/assist-settings"), &root, Some(r#"{"enabled":true}"#)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let stale = format!("{entries}/{id}/assist/stale");
        let res = send(&server, "POST", &stale, &editor, None).await;
        assert_eq!((res.status, parse(&res)["state"].clone()), (202, json!("running")));

        // 構成案: 重い権限 viewQuestionLog（再認証）。候補がなければ 404。候補を消したら、構成案も消える
        let db = workspace_db::lookup(&server.pool, ws).await.unwrap().unwrap();
        let mut tx = workspace_db::begin(&server.pool, &db).await.unwrap();
        log::change_settings(&mut tx, &log::SettingsChange { enabled: Some(true), retention_days: None }).await.unwrap().unwrap();
        tx.commit().await.unwrap();
        log::record(&server.pool, &db, Outcome::NoInformation, "ja", Some("答えられなかった質問")).await.unwrap();
        // 閲覧の重い権限 viewQuestionLog は、管理者にも既定では付かない。管理者が自分に付ける
        create_user(&server.pool, "admin@example.test", false).await;
        add_member(&server, ws, "admin@example.test", "admin").await;
        let admin = login(&server, "admin@example.test", PASSWORD).await;
        enroll(&server, &admin).await;
        let admin_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM yomi.users WHERE email = 'admin@example.test'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
        let res = send(&server, "PATCH", &format!("/api/admin/workspaces/{ws}/members/{admin_id}"), &admin, Some(&json!({"grants": ["viewQuestionLog"]}).to_string())).await;
        assert!(res.status < 300, "{}", res.body);
        let candidates = parse(&get(&server, &format!("/api/admin/workspaces/{ws}/question-log"), &admin).await);
        let key = candidates["candidates"][0]["id"].as_str().unwrap_or_else(|| panic!("{candidates}")).to_owned();
        let outline = format!("/api/admin/workspaces/{ws}/question-log/candidates/{key}/outline");
        assert_eq!(send(&server, "POST", &outline, &editor, None).await.status, 403);
        let unknown = format!("/api/admin/workspaces/{ws}/question-log/candidates/{}/outline", "0".repeat(64));
        assert_eq!(send(&server, "POST", &unknown, &admin, None).await.status, 404);
        assert_eq!(send(&server, "POST", &format!("/api/admin/workspaces/{ws}/question-log/candidates/x/outline"), &admin, None).await.status, 404);
        let res = send(&server, "POST", &outline, &admin, None).await;
        assert_eq!((res.status, parse(&res)["state"].clone()), (202, json!("running")));
        let rows = || async {
            let mut tx = workspace_db::begin(&server.pool, &db).await.unwrap();
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM assist_tasks WHERE kind = 'outline'")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            n
        };
        assert_eq!(rows().await, 1);
        let res = send(&server, "DELETE", &format!("/api/admin/workspaces/{ws}/question-log/candidates/{key}"), &admin, None).await;
        assert_eq!(res.status, 204, "{}", res.body);
        assert_eq!(rows().await, 0);
        assert_eq!(get(&server, &outline, &admin).await.status, 404);
        // 構成案を見るのは質問ログの閲覧と同じく監査ログに残る
        let viewed: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'question_log.viewed'")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert!(viewed >= 2, "{viewed}");
    })
    .await;
}
