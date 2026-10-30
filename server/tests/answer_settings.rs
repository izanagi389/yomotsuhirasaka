//! 回答の語り口と定型文の管理API（`/api/admin/workspaces/{ws}/answer-settings`。M4）の結合テスト。
//! 自由記述があっても引用の検証が効くこと（INV-QA-03）は、約束のテスト `tests/invariants/answer.test.ts` が擬似LLMで確かめる。
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
async fn answer_settings_api() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        // ワークスペースの管理者（重い権限 manageLlm を持つ）が操作する
        create_user(&server.pool, "admin@example.test", false).await;
        add_member(&server, ws, "admin@example.test", "admin").await;
        let admin = login(&server, "admin@example.test", PASSWORD).await;
        let path = format!("/api/admin/workspaces/{ws}/answer-settings");
        let put = |s: &Login, body: Value| {
            let (server, path, s) = (&server, path.clone(), s.clone());
            async move { send(server, "PUT", &path, &s, Some(&body.to_string())).await }
        };

        // 既定: 丁寧・標準の長さ・自由記述は使わない。「情報なし」の文はテンプレートのロケールごと
        let v = parse(&get(&server, &path, &admin).await);
        assert_eq!(v["politeness"], "polite");
        assert_eq!(v["length"], "standard");
        assert_eq!(v["freePrompt"], json!({"enabled": false, "text": ""}));
        assert_eq!(v["phrases"]["ja"]["noInformation"], "該当する情報がありません。");
        assert_eq!(v["phrases"]["en"]["noInformation"], "No relevant information was found.");

        // 重い権限 manageLlm のない編集者は、読めも変えられもしない
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        assert_eq!(get(&server, &path, &editor).await.status, 403);
        assert_eq!(put(&editor, json!({"politeness": "casual"})).await.status, 403);

        // 変えるには再認証が要る
        expire_reauth(&server.pool, "admin@example.test").await;
        let res = put(&admin, json!({"politeness": "casual"})).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (403, Some("reauth_required")));
        let admin = login(&server, "admin@example.test", PASSWORD).await;

        // 誤りは、何も変えずに拒否する
        let secret_tone = "SECRET-TONE-0001 be a pirate";
        for (body, what) in [
            (json!({"freePrompt": {"enabled": true}}), "空の自由記述を使う"),
            (json!({"freePrompt": {"enabled": true, "text": "x".repeat(2001)}}), "長すぎる自由記述"),
            (json!({"freePrompt": {"text": "a\u{0007}b"}}), "制御文字"),
            (json!({"phrases": {"fr": {"noInformation": "rien"}}}), "テンプレートにないロケール"),
            (json!({"phrases": {"ja": {"noInformation": "  "}}}), "空の定型文"),
            (json!({"phrases": {"ja": {"noInformation": "あ".repeat(501)}}}), "長すぎる定型文"),
            (json!({"politeness": "casual", "phrases": {"fr": {"noInformation": "rien"}}}), "一部だけ正しい"),
        ] {
            let res = put(&admin, body).await;
            assert_eq!((res.status, res.json_str("error").as_deref()), (422, Some("invalid_answer_settings")), "{what}: {}", res.body);
        }
        assert_eq!(parse(&get(&server, &path, &admin).await), v);

        // 書いた項目だけが変わる
        let res = put(&admin, json!({
            "politeness": "casual",
            "length": "short",
            "freePrompt": {"enabled": true, "text": secret_tone},
            "phrases": {"ja": {"noInformation": "  見つかりませんでした。 "}},
        }))
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        let v = parse(&res);
        assert_eq!((v["politeness"].as_str(), v["length"].as_str()), (Some("casual"), Some("short")));
        assert_eq!(v["freePrompt"], json!({"enabled": true, "text": secret_tone}));
        assert_eq!(v["phrases"]["ja"]["noInformation"], "見つかりませんでした。");
        assert_eq!(v["phrases"]["en"]["noInformation"], "No relevant information was found.");
        // 使わない設定にしても、文は残る
        let v = parse(&put(&admin, json!({"freePrompt": {"enabled": false}})).await);
        assert_eq!(v["freePrompt"], json!({"enabled": false, "text": secret_tone}));
        assert_eq!(v["politeness"], "casual");

        // 答えない質問の決まり: 拒否する語は前後の空白を除き、空と重複を除いて、一覧ごと置き換える
        let v = parse(&put(&admin, json!({"refusal": {"words": [" SECRET-WORD-0001 ", "", "SECRET-WORD-0001", "社外秘"], "creditCard": true}})).await);
        assert_eq!(v["refusal"], json!({"words": ["SECRET-WORD-0001", "社外秘"], "creditCard": true, "myNumber": false}));
        let too_many: Vec<String> = (0..101).map(|i| format!("w{i}")).collect();
        for (body, what) in [
            (json!({"refusal": {"words": too_many}}), "多すぎる語"),
            (json!({"refusal": {"words": ["x".repeat(101)]}}), "長すぎる語"),
            (json!({"refusal": {"words": ["a\u{0007}"]}}), "制御文字"),
            (json!({"refusal": {"other": true}}), "知らない項目"),
        ] {
            let res = put(&admin, body).await;
            assert!(res.status == 422 || res.status == 400, "{what}: {} {}", res.status, res.body);
        }
        assert_eq!(parse(&get(&server, &path, &admin).await)["refusal"]["words"], json!(["SECRET-WORD-0001", "社外秘"]));

        // 監査ログには、変えた項目の名前だけが残る（文の中身は残さない）
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT action, details::text FROM cms.audit_log WHERE workspace_id = $1 AND action = 'answer.settings_changed' ORDER BY created_at",
        )
        .bind(ws)
        .fetch_all(&server.pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 3);
        let details: Value = serde_json::from_str(rows[0].1.as_deref().unwrap()).unwrap();
        assert_eq!(
            details["fields"],
            json!(["politeness", "length", "freePrompt.enabled", "freePrompt.text", "phrases.ja"])
        );
        for (_, d) in &rows {
            let d = d.as_deref().unwrap();
            assert!(!d.contains("SECRET-TONE") && !d.contains("見つかりませんでした") && !d.contains("SECRET-WORD") && !d.contains("社外秘"), "{d}");
        }
    })
    .await;
}
