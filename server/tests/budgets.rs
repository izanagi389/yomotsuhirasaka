//! LLMの予算の確保と精算の結合テスト（本物のPostgreSQL）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use cms_server::budget::{self, Direction, Kind, Reserved};
use cms_server::config::LlmConfig;
use common::server::*;
use common::with_scratch;
use uuid::Uuid;

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
        next_embedding: None,
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
        let admin_id: Uuid = sqlx::query_scalar("SELECT id FROM cms.users WHERE email = 'root@example.test'")
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
            .filter(|r| matches!(r.as_ref().unwrap(), Reserved::Exhausted { first: true }))
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
            Reserved::Exhausted { first: false }
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
            "INSERT INTO cms.workspace_limits (workspace_id, embedding_tokens_per_month) VALUES ($1, 10)
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
            Reserved::Exhausted { first: true }
        ));
    })
    .await;
}
