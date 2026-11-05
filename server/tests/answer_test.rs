//! 管理画面の回答のテスト（`POST /api/admin/workspaces/{ws}/answer-test`。M4）の入口の結合テスト。LLMの接続先は設定しない
//! （回答の中身・断片・判定の宣言は、擬似LLMを使う約束のテストと管理画面の確認で見る）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

#[tokio::test]
async fn answer_test_entrance() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let path = format!("/api/admin/workspaces/{ws}/answer-test");
        let ask = |s: &Login, body: Value| {
            let (server, path, s) = (&server, path.clone(), s.clone());
            async move { send(server, "POST", &path, &s, Some(&body.to_string())).await }
        };

        // 重い権限 manageLlm を持つ人だけ
        assert_eq!(ask(&editor, json!({"question": "q", "locale": "ja"})).await.status, 403);
        // 知らない項目・不正な語り口は断る
        assert_eq!(ask(&root, json!({"question": "q", "locale": "ja", "x": 1})).await.status, 400);
        let bad = json!({"question": "q", "locale": "ja", "tone": {"politeness": "polite", "length": "short", "freePrompt": {"enabled": true, "text": " "}}});
        assert_eq!(ask(&root, bad).await.status, 422);

        // マスクした質問文を見せる。LLMの設定がなければ、配信APIと同じコードで答えられない理由を返す
        let res = ask(&root, json!({"question": "連絡先は a@example.com です", "locale": "ja"})).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let v = parse(&res);
        assert!(!v["maskedQuestion"].as_str().unwrap().contains("a@example.com"), "{v}");
        assert_eq!(v["error"]["code"], "ANSWER_UNAVAILABLE");
        // 拒否する語も本番と同じに効く
        let res = send(&server, "PUT", &format!("/api/admin/workspaces/{ws}/answer-settings"), &root, Some(r#"{"refusal":{"words":["社外秘"]}}"#)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let v = parse(&ask(&root, json!({"question": "社外秘の件", "locale": "ja"})).await);
        assert_eq!(v["error"]["code"], "QUESTION_REFUSED");

        // 質問ログには、件数も残さない
        let db = cms_server::workspace_db::lookup(&server.pool, ws).await.unwrap().unwrap();
        let mut tx = cms_server::workspace_db::begin(&server.pool, &db).await.unwrap();
        let counted: i64 = sqlx::query_scalar("SELECT COALESCE(sum(count), 0)::bigint FROM question_counts")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(counted, 0);

        // 人ごとに1分10回まで（数えるのは権限と本文の検査を通ったものだけ。ここまでで2回）
        for _ in 0..8 {
            assert_eq!(ask(&root, json!({"question": "q", "locale": "ja"})).await.status, 200);
        }
        let res = ask(&root, json!({"question": "q", "locale": "ja"})).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (429, Some("answer_test_rate_limited")));
    })
    .await;
}
