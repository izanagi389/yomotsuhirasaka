//! 埋め込みの HNSW の索引の結合テスト（本物のPostgreSQL と pgvector）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する（使い捨てのDBに拡張 vector を作る）。

mod common;

use cms_server::config::LlmConfig;
use common::content::*;
use common::server::*;
use common::with_scratch_at;

/// 引用符を含むモデル名（索引の条件に文字列の定数として書くため、引用が正しいことも確かめる）。
const MODEL: &str = "o'brien/embed-small";

fn llm() -> LlmConfig {
    LlmConfig {
        base_url: "https://llm.example.test/v1".into(),
        api_key: "test-key".into(),
        embedding_model: MODEL.into(),
        embedding_dimensions: 8,
        embedding_price_per_million_tokens: None,
        default_workspace_embedding_tokens_per_month: None,
        default_workspace_embedding_tokens_per_day: None,
        next_embedding: None,
    }
}

#[tokio::test]
async fn each_workspace_gets_an_hnsw_index_for_the_configured_model() {
    with_scratch_at("CMS_TEST_PG_SUPERUSER_URL", &["vector"], |app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.llm = Some(llm());
        let server = start(cfg.clone()).await;
        let (root, _) = root_with_deploy_token(&server).await;
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let schema = format!("ws_{}", ws.simple());
        let indexes = || async {
            sqlx::query_scalar::<_, String>(
                "SELECT indexdef FROM pg_indexes WHERE schemaname = $1 AND tablename = 'search_embeddings'
                 AND indexdef LIKE '%USING hnsw%'",
            )
            .bind(&schema)
            .fetch_all(&server.pool)
            .await
            .unwrap()
        };
        // ワークスペースを作ったときに、設定のモデルの部分索引ができる
        let defs = indexes().await;
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert!(defs[0].contains("vector_cosine_ops"), "{}", defs[0]);
        assert!(defs[0].contains("'o''brien/embed-small'"), "the model name is quoted: {}", defs[0]);
        assert!(defs[0].contains("vector(8)"), "{}", defs[0]);
        // 起動のたびに呼んでも、増えない
        cms_server::search::ensure_vector_indexes(&server.state.migration, &cfg)
            .await
            .unwrap();
        assert_eq!(indexes().await.len(), 1);
        // 切り替え先のモデルを足せば、そのモデルの索引も作る
        let mut with_next = cfg.clone();
        with_next.llm.as_mut().unwrap().next_embedding = Some(cms_server::config::NextEmbeddingConfig {
            model: "embed-large".into(),
            dimensions: 16,
            base_url: None,
            api_key: None,
            embedding_price_per_million_tokens: None,
        });
        cms_server::search::ensure_vector_indexes(&server.state.migration, &with_next)
            .await
            .unwrap();
        let defs = indexes().await;
        assert_eq!(defs.len(), 2, "{defs:?}");
        assert!(defs.iter().any(|d| d.contains("vector(16)") && d.contains("'embed-large'")));
        // 設定からなくなったモデルの索引は消す
        let mut only_next = cfg.clone();
        only_next.llm.as_mut().unwrap().embedding_model = "embed-large".into();
        only_next.llm.as_mut().unwrap().embedding_dimensions = 16;
        cms_server::search::ensure_vector_indexes(&server.state.migration, &only_next)
            .await
            .unwrap();
        let defs = indexes().await;
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert!(defs[0].contains("'embed-large'"));
    })
    .await;
}
