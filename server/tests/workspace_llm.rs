//! ワークスペースの LLM の接続設定の上書き（`/api/admin/workspaces/{ws}/llm`。M4、Q26）の結合テスト。
//! 回答がその接続先とキーで作られること（INV-LLM-01, 02）は、約束のテスト `tests/invariants/llm-override.test.ts` が確かめる。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use cms_server::config::{AllowedEmbeddingModel, AllowedEndpoint, LlmConfig};
use cms_server::workspace_llm;
use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

const SECRET: &str = "sk-WORKSPACE-SECRET-0001";

#[tokio::test]
async fn workspace_llm_api() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.llm = Some(LlmConfig {
            base_url: "https://llm.example.test/v1".into(),
            api_key: "instance-key".into(),
            embedding_model: "m".into(),
            embedding_dimensions: 8,
            embedding_price_per_million_tokens: None,
            default_workspace_embedding_tokens_per_month: None,
            default_workspace_embedding_tokens_per_day: None,
            chat_model: Some("instance-chat".into()),
            chat_input_price_per_million_tokens: Some(1.0),
            chat_output_price_per_million_tokens: Some(2.0),
            default_workspace_answer_input_tokens_per_month: None,
            default_workspace_answer_input_tokens_per_day: None,
            default_workspace_answer_output_tokens_per_month: None,
            default_workspace_answer_output_tokens_per_day: None,
            next_embedding: None,
            allowed_endpoints: vec![AllowedEndpoint {
                name: "primary".into(),
                base_url: "https://primary.example.test/v1".into(),
            }],
            allowed_embedding_models: vec![AllowedEmbeddingModel {
                model: "ws-embed".into(),
                dimensions: 16,
                embedding_price_per_million_tokens: Some(0.1),
            }],
        });
        let server = start(cfg.clone()).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let path = format!("/api/admin/workspaces/{ws}/llm");
        let put = |body: Value| {
            let (server, root, path) = (&server, &root, path.clone());
            async move { send(server, "PUT", &path, root, Some(&body.to_string())).await }
        };
        let error = |res: &Resp| (res.status, res.json_str("error"));
        let effective = || async {
            workspace_llm::effective(&server.pool, &server.state.keys, &cfg, ws).await.unwrap().unwrap()
        };

        // 既定: 上書きは許されていない。選べる接続先の名前だけが見える
        let v = parse(&get(&server, &path, &root).await);
        assert_eq!(v["allowed"], false);
        assert_eq!(v["endpoints"], json!(["primary"]));
        assert_eq!(v["embeddingModels"], json!(["ws-embed"]));
        assert_eq!(v["settings"], Value::Null);
        assert_eq!(v["embedding"]["state"], "switched");
        assert_eq!(v["embedding"]["activeModel"], "m");
        assert_eq!(get(&server, &path, &editor).await.status, 403);
        let res = put(json!({"endpoint": "primary", "apiKey": SECRET})).await;
        assert_eq!(error(&res), (403, Some("llm_override_not_allowed".into())));
        assert_eq!(effective().await.1, "");

        // サーバー管理者が上書きを許す
        let res = send(&server, "PUT", &format!("/api/admin/workspaces/{ws}/limits"), &root, Some(r#"{"llmOverride":true}"#)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res)["llmOverride"], true);

        // 許可の一覧にない名前・URL の項目は拒否する。初めての設定ではキーが要る
        assert_eq!(error(&put(json!({"endpoint": "other", "apiKey": SECRET})).await), (422, Some("unknown_llm_endpoint".into())));
        assert_eq!(error(&put(json!({"endpoint": "https://primary.example.test/v1", "apiKey": SECRET})).await), (422, Some("unknown_llm_endpoint".into())));
        let res = put(json!({"endpoint": "primary", "apiKey": SECRET, "baseUrl": "http://169.254.169.254"})).await;
        assert!(res.status == 400 || res.status == 422, "{}", res.body);
        assert_eq!(error(&put(json!({"endpoint": "primary"})).await), (422, Some("invalid_llm_settings".into())));
        assert_eq!(error(&put(json!({"endpoint": "primary", "apiKey": " "})).await), (422, Some("invalid_llm_settings".into())));
        assert_eq!(error(&put(json!({"endpoint": "primary", "apiKey": SECRET, "chatInputPrice": -1})).await), (422, Some("invalid_llm_settings".into())));

        // 保存する。応答にも読み取りにもキーは出ない。DBには暗号化して入る
        let res = put(json!({"endpoint": "primary", "apiKey": SECRET, "chatModel": "ws-chat", "chatInputPrice": 0.5})).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(!res.body.contains(SECRET), "{}", res.body);
        let v = parse(&get(&server, &path, &root).await);
        assert!(!v.to_string().contains(SECRET));
        assert_eq!(v["settings"]["endpoint"], "primary");
        assert_eq!(v["settings"]["apiKeySet"], true);
        let updated = v["settings"]["apiKeyUpdatedAt"].clone();
        let stored: Vec<u8> = sqlx::query_scalar("SELECT api_key FROM cms.workspace_llm WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&stored).contains(SECRET));
        // 回答には、許可の一覧の URL と、ワークスペースのキー・モデル・単価を使う（埋め込みはインスタンスのまま）
        let (c, endpoint) = effective().await;
        assert_eq!(endpoint, "primary");
        assert_eq!((c.base_url.as_str(), c.api_key.as_str()), ("https://primary.example.test/v1", SECRET));
        assert_eq!(c.chat_model.as_deref(), Some("ws-chat"));
        assert_eq!((c.chat_input_price_per_million_tokens, c.chat_output_price_per_million_tokens), (Some(0.5), None));
        assert_eq!(c.embedding_model, "m");

        // キーを送らなければ、保存してあるキーを使い続ける
        let res = put(json!({"endpoint": "primary", "chatModel": "ws-chat-2"})).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res)["settings"]["apiKeyUpdatedAt"], updated);
        assert_eq!(effective().await.0.api_key, SECRET);

        // 監査ログには変えた項目の名前だけ（キーは残さない）
        let details: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT details::text FROM cms.audit_log WHERE workspace_id = $1 AND action = 'llm.override_changed'",
        )
        .bind(ws)
        .fetch_all(&server.pool)
        .await
        .unwrap();
        assert_eq!(details.len(), 2);
        assert!(details.iter().flatten().all(|d| !d.contains(SECRET)));

        // サーバー管理者が許可を取り消したら、上書き（キーを含む）は消え、インスタンスの設定に戻る
        let res = send(&server, "PUT", &format!("/api/admin/workspaces/{ws}/limits"), &root, Some(r#"{"llmOverride":false}"#)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.workspace_llm WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
        let (c, endpoint) = effective().await;
        assert_eq!((endpoint.as_str(), c.api_key.as_str()), ("", "instance-key"));

        // 埋め込みのモデルの上書き: 許可の一覧の名前だけ。選んだだけでは切り替えず、見積もりを見て開始する
        send(&server, "PUT", &format!("/api/admin/workspaces/{ws}/limits"), &root, Some(r#"{"llmOverride":true}"#)).await;
        assert_eq!(put(json!({"endpoint": "primary", "apiKey": SECRET})).await.status, 200);
        assert_eq!(error(&put(json!({"endpoint": "primary", "embeddingModel": "other"})).await), (422, Some("unknown_embedding_model".into())));
        let v = parse(&put(json!({"endpoint": "primary", "embeddingModel": "ws-embed"})).await);
        assert_eq!(v["settings"]["embeddingModel"], "ws-embed");
        assert_eq!((v["embedding"]["state"].as_str(), v["embedding"]["desiredModel"].as_str()), (Some("notStarted"), Some("ws-embed")));
        assert_eq!(v["embedding"]["activeModel"], "m");
        // 埋め込みは、上書きの接続先とキーで作る（切り替え先として渡す）
        let setup = workspace_llm::embedding(&server.pool, &server.state.keys, &cfg, ws).await.unwrap().unwrap();
        let next = setup.llm.next().unwrap();
        assert_eq!((next.embedding_model.as_str(), next.embedding_dimensions), ("ws-embed", 16));
        assert_eq!((next.base_url.as_str(), next.api_key.as_str()), ("https://primary.example.test/v1", SECRET));
        assert!(setup.overridden && setup.started.is_empty());
        // 開始する（切り替え先の埋め込みを作っている間は、前のモデルで検索する）
        let start = format!("{path}/embedding-switch");
        let res = send(&server, "POST", &start, &root, None).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res)["embedding"]["state"], "inProgress");
        // 次元数が合わなければ、切り替えを止めて理由を見せる（開始し直せる）
        assert!(workspace_llm::embedding_switch_failed(&server.pool, ws, &("ws-embed".into(), 16), "dimension_mismatch").await.unwrap());
        let v = parse(&get(&server, &path, &root).await);
        assert_eq!((v["embedding"]["state"].as_str(), v["embedding"]["failedCode"].as_str()), (Some("notStarted"), Some("dimension_mismatch")));
        assert_eq!(send(&server, "POST", &start, &root, None).await.status, 200);
        assert_eq!(parse(&get(&server, &path, &root).await)["embedding"]["failedCode"], Value::Null);
        // 上書きのモデルを外すと、インスタンスのモデルのまま（切り替えるものはない）
        let v = parse(&put(json!({"endpoint": "primary", "embeddingModel": null})).await);
        assert_eq!(v["settings"]["embeddingModel"], Value::Null);
        assert_eq!(v["embedding"]["state"], "switched");
        assert_eq!(error(&send(&server, "POST", &start, &root, None).await), (409, Some("no_embedding_switch".into())));

        // 上書きをやめる（DELETE）
        assert_eq!(send(&server, "DELETE", &path, &root, None).await.status, 204);
        assert_eq!(parse(&get(&server, &path, &root).await)["settings"], Value::Null);
    })
    .await;
}
