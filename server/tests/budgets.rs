//! LLMの予算の確保と精算の結合テスト（本物のPostgreSQL）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::server::*;
use common::with_scratch;
use uuid::Uuid;
use yomi_server::budget::{self, Direction, Kind, Reserved};
use yomi_server::config::LlmConfig;

async fn workspace(server: &Server, root: &Login, name: &str) -> Uuid {
    let res = send(
        server,
        "POST",
        "/api/admin/workspaces",
        root,
        Some(&format!("{{\"name\":\"{name}\"}}")),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("id").unwrap().parse().unwrap()
}

fn llm(per_month: Option<u64>, per_day: Option<u64>) -> LlmConfig {
    LlmConfig {
        base_url: "https://llm.example.test/v1".into(),
        api_key: "test-key".into(),
        embedding_model: "m".into(),
        embedding_dimensions: 8,
        embedding_price_per_million_tokens: None,
        default_workspace_embedding_tokens_per_month: per_month,
        default_workspace_embedding_tokens_per_day: per_day,
        chat_model: None,
        chat_input_price_per_million_tokens: None,
        chat_output_price_per_million_tokens: None,
        default_workspace_answer_input_tokens_per_month: None,
        default_workspace_answer_input_tokens_per_day: None,
        default_workspace_answer_output_tokens_per_month: None,
        default_workspace_answer_output_tokens_per_day: None,
        default_workspace_assist_input_tokens_per_month: None,
        default_workspace_assist_input_tokens_per_day: None,
        default_workspace_assist_output_tokens_per_month: None,
        default_workspace_assist_output_tokens_per_day: None,
        next_embedding: None,
        allowed_endpoints: Vec::new(),
        allowed_embedding_models: Vec::new(),
    }
}

#[tokio::test]
async fn reservations_never_exceed_the_effective_limit() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.llm = Some(llm(Some(1000), Some(1000)));
        let server = start(cfg.clone()).await;
        create_user(&server.pool, "root@example.test", true).await;
        let root = login(&server, "root@example.test", PASSWORD).await;
        enroll(&server, &root).await;
        let ws = workspace(&server, &root, "budget").await;
        let admin_id: Uuid = sqlx::query_scalar("SELECT id FROM yomi.users WHERE email = 'root@example.test'")
            .fetch_one(&server.pool)
            .await
            .unwrap();

        // ワークスペースの上限（100）が天井（1000）より小さければ、ワークスペースの上限が効く
        budget::set_limits(&server.pool, ws, Kind::Embedding, Direction::Input, Some(100), None, admin_id)
            .await
            .unwrap();
        let limits = budget::limits(&server.pool, &cfg, ws, Kind::Embedding, Direction::Input).await.unwrap();
        assert_eq!(limits.effective().per_month, 100);
        assert_eq!(limits.effective().per_day, 1000);

        // 同時に 30 トークンずつ 10 回確保しても、通るのは 3 回まで
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..10 {
            let (pool, cfg) = (server.pool.clone(), cfg.clone());
            set.spawn(async move { budget::reserve(&pool, &cfg, ws, Kind::Embedding, Direction::Input, 30).await });
        }
        let results = set.join_all().await;
        let ok: Vec<_> = results
            .iter()
            .filter_map(|r| match r.as_ref().unwrap() {
                Reserved::Ok(r) => Some(r.clone()),
                Reserved::Exhausted { .. } => None,
            })
            .collect();
        assert_eq!(ok.len(), 3, "{results:?}");
        // 「初めて断った」は、この期間で1回だけ
        let firsts = results
            .iter()
            .filter(|r| matches!(r.as_ref().unwrap(), Reserved::Exhausted { first: true, .. }))
            .count();
        assert_eq!(firsts, 1, "{results:?}");
        let usage = budget::usage(&server.pool, ws, Kind::Embedding, Direction::Input).await.unwrap();
        assert_eq!((usage.month, usage.day), (90, 90));
        assert!(usage.exhausted_at.is_some());

        // 精算で余りを返すと、その分また確保できる
        budget::settle(&server.pool, &ok[0], 5).await.unwrap();
        let usage = budget::usage(&server.pool, ws, Kind::Embedding, Direction::Input).await.unwrap();
        assert_eq!(usage.month, 65);
        assert!(matches!(
            budget::reserve(&server.pool, &cfg, ws, Kind::Embedding, Direction::Input, 35).await.unwrap(),
            Reserved::Ok(_)
        ));
        assert!(matches!(
            budget::reserve(&server.pool, &cfg, ws, Kind::Embedding, Direction::Input, 1).await.unwrap(),
            Reserved::Exhausted { first: false, .. }
        ));

        // 上限を変えると「断った」の印が消え、上げた分だけ再開する
        budget::set_limits(&server.pool, ws, Kind::Embedding, Direction::Input, Some(150), None, admin_id)
            .await
            .unwrap();
        assert_eq!(
            budget::usage(&server.pool, ws, Kind::Embedding, Direction::Input).await.unwrap().exhausted_at,
            None
        );
        assert!(matches!(
            budget::reserve(&server.pool, &cfg, ws, Kind::Embedding, Direction::Input, 50).await.unwrap(),
            Reserved::Ok(_)
        ));

        // 天井を下げると、ワークスペースの上限より先に天井が効く（使う瞬間に照合する）
        sqlx::query(
            "INSERT INTO yomi.workspace_limits (workspace_id, embedding_tokens_per_month) VALUES ($1, 10)
             ON CONFLICT (workspace_id) DO UPDATE SET embedding_tokens_per_month = 10",
        )
        .bind(ws)
        .execute(&server.pool)
        .await
        .unwrap();
        assert!(matches!(
            budget::reserve(&server.pool, &cfg, ws, Kind::Embedding, Direction::Input, 1).await.unwrap(),
            Reserved::Exhausted { .. }
        ));
    })
    .await;
}

#[tokio::test]
async fn the_ceiling_defaults_to_zero() {
    with_scratch(|app, migration| async move {
        // 設定ファイルに既定値がなければ、天井は 0（埋め込みを作らない）
        let mut cfg = config(&app, &migration);
        cfg.llm = Some(llm(None, None));
        let server = start(cfg.clone()).await;
        create_user(&server.pool, "root@example.test", true).await;
        let root = login(&server, "root@example.test", PASSWORD).await;
        enroll(&server, &root).await;
        let ws = workspace(&server, &root, "zero").await;
        assert!(matches!(
            budget::reserve(&server.pool, &cfg, ws, Kind::Embedding, Direction::Input, 1)
                .await
                .unwrap(),
            Reserved::Exhausted { first: true, .. }
        ));
    })
    .await;
}

#[tokio::test]
async fn answer_budget_api() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        let mut l = llm(None, None);
        l.chat_model = Some("chat".into());
        l.chat_output_price_per_million_tokens = Some(2.0);
        cfg.llm = Some(l);
        let server = start(cfg.clone()).await;
        // やることの一覧は、テンプレートを適用したワークスペースで読む
        let (root, token) = common::content::root_with_deploy_token(&server).await;
        let manifest = common::content::blog_manifest();
        assert_eq!(common::content::deploy(&server, &token, "blog", &manifest, "").await.status, 200);
        let ws = common::content::workspace_with_template(&server, &root, "answer", "blog").await;
        let path = format!("/api/admin/workspaces/{ws}/budgets/answer");
        let get = |s: &Login| {
            let (server, s, path) = (&server, s.clone(), path.clone());
            async move {
                let res = send(server, "GET", &path, &s, None).await;
                assert_eq!(res.status, 200, "{}", res.body);
                serde_json::from_str::<serde_json::Value>(&res.body).unwrap()
            }
        };

        // 重い権限 manageLlm のない人は見られない
        create_user(&server.pool, "editor@example.test", false).await;
        common::content::add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        assert_eq!(send(&server, "GET", &path, &editor, None).await.status, 403);

        // 天井は、設定ファイルに既定値がなければ 0。サーバー管理者が入力・出力ごとに決める
        let v = get(&root).await;
        assert_eq!(v["chatConfigured"], true);
        assert_eq!(v["input"]["ceilingTokensPerMonth"], 0);
        let res = send(
            &server,
            "PUT",
            &format!("/api/admin/workspaces/{ws}/limits"),
            &root,
            Some(r#"{"answerInputTokensPerMonth":1000,"answerInputTokensPerDay":100,"answerOutputTokensPerMonth":500,"answerOutputTokensPerDay":50}"#),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        let v = get(&root).await;
        assert_eq!((v["input"]["ceilingTokensPerMonth"].clone(), v["output"]["ceilingTokensPerDay"].clone()), (1000.into(), 50.into()));
        assert_eq!(v["output"]["pricePerMillionTokens"], 2.0);
        assert_eq!(v["input"]["pricePerMillionTokens"], serde_json::Value::Null);

        // 天井を超える上限は 400。片方が超えていれば、どちらも変えない
        let res = send(
            &server,
            "PUT",
            &path,
            &root,
            Some(r#"{"input":{"tokensPerMonth":10,"tokensPerDay":null},"output":{"tokensPerMonth":501,"tokensPerDay":null}}"#),
        )
        .await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (400, Some("budget_exceeds_ceiling")));
        assert_eq!(get(&root).await["input"]["tokensPerMonth"], serde_json::Value::Null);
        // 送った向きだけが変わる
        let res = send(&server, "PUT", &path, &root, Some(r#"{"output":{"tokensPerMonth":400,"tokensPerDay":null}}"#)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let v: serde_json::Value = serde_json::from_str(&res.body).unwrap();
        assert_eq!((v["output"]["tokensPerMonth"].clone(), v["output"]["effectiveTokensPerMonth"].clone()), (400.into(), 400.into()));
        assert_eq!(v["input"]["tokensPerMonth"], serde_json::Value::Null);
        let changed: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'budget.changed'")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(changed, 1);

        // 予算が尽きたら、使用量の画面とやることの一覧に出る（どの予算かを示す）
        assert!(matches!(
            budget::reserve(&server.pool, &cfg, ws, Kind::VisitorAnswers, Direction::Output, 401).await.unwrap(),
            Reserved::Exhausted { first: true, .. }
        ));
        assert!(get(&root).await["output"]["exhaustedAt"].is_string());
        let res = send(&server, "GET", &format!("/api/admin/workspaces/{ws}/todo"), &root, None).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let todo: serde_json::Value = serde_json::from_str(&res.body).unwrap();
        let budgets: Vec<&str> = todo["items"]
            .as_array()
            .or_else(|| todo.as_array())
            .unwrap()
            .iter()
            .filter(|i| i["kind"] == "budgetExhausted")
            .filter_map(|i| i["budget"].as_str())
            .collect();
        assert_eq!(budgets, vec!["visitor_answers"]);
    })
    .await;
}

#[tokio::test]
async fn assist_budget_api_and_settings() {
    with_scratch(|app, migration| async move {
        // 編集者向けの支援の予算（M4、Q70）: 天井は設定ファイルの既定値、なければ 0。尽きた期間を返す
        let mut cfg = config(&app, &migration);
        let mut l = llm(None, None);
        l.chat_model = Some("chat".into());
        l.default_workspace_assist_output_tokens_per_month = Some(300);
        cfg.llm = Some(l);
        let server = start(cfg.clone()).await;
        let (root, token) = common::content::root_with_deploy_token(&server).await;
        let manifest = common::content::blog_manifest();
        assert_eq!(common::content::deploy(&server, &token, "blog", &manifest, "").await.status, 200);
        let ws = common::content::workspace_with_template(&server, &root, "assist", "blog").await;
        let path = format!("/api/admin/workspaces/{ws}/budgets/assist");
        let get = || async {
            let res = send(&server, "GET", &path, &root, None).await;
            assert_eq!(res.status, 200, "{}", res.body);
            serde_json::from_str::<serde_json::Value>(&res.body).unwrap()
        };

        let v = get().await;
        assert_eq!(v["input"]["ceilingTokensPerMonth"], 0);
        assert_eq!(v["output"]["ceilingTokensPerMonth"], 300);
        // サーバー管理者が天井を決める
        let res = send(
            &server,
            "PUT",
            &format!("/api/admin/workspaces/{ws}/limits"),
            &root,
            Some(r#"{"assistInputTokensPerMonth":1000,"assistInputTokensPerDay":100}"#),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        let v = get().await;
        assert_eq!((v["input"]["ceilingTokensPerMonth"].clone(), v["input"]["ceilingTokensPerDay"].clone()), (1000.into(), 100.into()));
        // 天井を超える上限は 400
        let res = send(&server, "PUT", &path, &root, Some(r#"{"input":{"tokensPerMonth":1001,"tokensPerDay":null}}"#)).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (400, Some("budget_exceeds_ceiling")));
        let res = send(&server, "PUT", &path, &root, Some(r#"{"input":{"tokensPerMonth":500,"tokensPerDay":50}}"#)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(get().await["input"]["effectiveTokensPerMonth"], 500);
        // 他の予算とは別に数える
        assert_eq!(
            budget::limits(&server.pool, &cfg, ws, Kind::VisitorAnswers, Direction::Input).await.unwrap().effective().per_month,
            0
        );

        // 尽きた期間: 日の上限（50）だけを超える → day、月の上限（500）を超える → month
        let reserve = |n| budget::reserve(&server.pool, &cfg, ws, Kind::EditorAssist, Direction::Input, n);
        assert!(matches!(
            reserve(51).await.unwrap(),
            Reserved::Exhausted { first: true, period: budget::Period::Day }
        ));
        assert!(matches!(
            reserve(501).await.unwrap(),
            Reserved::Exhausted { first: true, period: budget::Period::Month }
        ));
        assert!(matches!(reserve(50).await.unwrap(), Reserved::Ok(_)));

        // 支援の予算が尽きたことも、やることの一覧に出る
        let res = send(&server, "GET", &format!("/api/admin/workspaces/{ws}/todo"), &root, None).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let todo: serde_json::Value = serde_json::from_str(&res.body).unwrap();
        let budgets: Vec<&str> = todo["items"]
            .as_array()
            .or_else(|| todo.as_array())
            .unwrap()
            .iter()
            .filter(|i| i["kind"] == "budgetExhausted")
            .filter_map(|i| i["budget"].as_str())
            .collect();
        assert_eq!(budgets, vec!["editor_assist"]);

        // 支援のスイッチ: 既定は無効。重い権限 manageLlm のない人は見られない。変えたら監査ログに残る
        let settings = format!("/api/admin/workspaces/{ws}/assist-settings");
        let res = send(&server, "GET", &settings, &root, None).await;
        assert_eq!((res.status, res.body.as_str()), (200, r#"{"enabled":false}"#));
        create_user(&server.pool, "editor@example.test", false).await;
        common::content::add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        assert_eq!(send(&server, "GET", &settings, &editor, None).await.status, 403);
        assert_eq!(send(&server, "PUT", &settings, &editor, Some(r#"{"enabled":true}"#)).await.status, 403);
        assert_eq!(send(&server, "PUT", &settings, &root, Some(r#"{"enabled":true,"x":1}"#)).await.status, 400);
        let res = send(&server, "PUT", &settings, &root, Some(r#"{"enabled":true}"#)).await;
        assert_eq!((res.status, res.body.as_str()), (200, r#"{"enabled":true}"#));
        assert_eq!(send(&server, "GET", &settings, &root, None).await.body, r#"{"enabled":true}"#);
        let changed: Vec<String> = sqlx::query_scalar(
            "SELECT details::text FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'assist.settings_changed'",
        )
        .bind(ws)
        .fetch_all(&server.pool)
        .await
        .unwrap();
        assert_eq!(changed, vec![r#"{"enabled": true}"#.to_owned()]);
    })
    .await;
}
