//! まとめて1回だけ動かすジョブ（保存後のフック）の結合テスト（本物のPostgreSQL）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::server::*;
use common::with_scratch;
use yomi_server::jobs;

async fn pending(server: &Server) -> Vec<(i32, f64)> {
    sqlx::query_as(
        "SELECT attempts, EXTRACT(EPOCH FROM run_at - now())::float8 FROM yomi.jobs WHERE dedupe_key = 'k' ORDER BY created_at",
    )
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn debounced_jobs_are_merged_until_claimed() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let keys = &server.state.keys;
        let enqueue = |delay: f64| {
            jobs::enqueue_debounced(&server.pool, keys, None, "test", b"x", 3, "k", delay)
        };
        // 取り出される前に積み直すと、1つのまま実行の時刻だけが延びる
        enqueue(5.0).await.unwrap();
        enqueue(60.0).await.unwrap();
        let rows = pending(&server).await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].1 > 30.0, "{rows:?}");

        // 取り出された後に積むと、次の1回として別に積まれる
        enqueue(0.0).await.unwrap();
        let claimed = jobs::claim(&server.pool, keys)
            .await
            .unwrap()
            .expect("a job");
        assert_eq!(claimed.payload.as_deref(), Some(&b"x"[..]));
        enqueue(0.0).await.unwrap();
        let rows = pending(&server).await;
        assert_eq!(
            rows.iter().map(|r| r.0).collect::<Vec<_>>(),
            vec![1, 0],
            "{rows:?}"
        );
    })
    .await;
}
