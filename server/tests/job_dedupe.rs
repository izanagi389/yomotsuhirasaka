//! まとめて1回だけ動かすジョブ（保存後のフック）の結合テスト（本物のPostgreSQL）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
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

/// 1つのワークスペースのジョブが積み上がっても、取り出し手を占めない（13-security.md「ワークスペースごとの上限」）。
#[tokio::test]
async fn one_workspace_cannot_take_every_worker() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let keys = &server.state.keys;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let busy = workspace_with_template(&server, &root, "busy", "blog").await;
        let other = workspace_with_template(&server, &root, "other", "blog").await;
        // 先に積んだ busy のジョブが並んでいても、busy は2つまでで、other とインスタンス全体のジョブに順番が回る
        for _ in 0..5 {
            jobs::enqueue(&server.pool, keys, Some(busy), "test", b"x", 3)
                .await
                .unwrap();
        }
        jobs::enqueue(&server.pool, keys, Some(other), "test", b"x", 3)
            .await
            .unwrap();
        jobs::enqueue(&server.pool, keys, None, "test", b"x", 3)
            .await
            .unwrap();
        let mut claimed = Vec::new();
        while let Some(job) = jobs::claim(&server.pool, keys).await.unwrap() {
            claimed.push(job);
        }
        let of = |ws: Option<uuid::Uuid>| claimed.iter().filter(|j| j.workspace_id == ws).count();
        assert_eq!((of(Some(busy)), of(Some(other)), of(None)), (2, 1, 1));
        // 終われば次を取り出せる
        let done = claimed
            .iter()
            .find(|j| j.workspace_id == Some(busy))
            .unwrap();
        jobs::finish(&server.pool, done, jobs::Outcome::Done)
            .await
            .unwrap();
        let next = jobs::claim(&server.pool, keys)
            .await
            .unwrap()
            .expect("a job");
        assert_eq!(next.workspace_id, Some(busy));
        assert!(jobs::claim(&server.pool, keys).await.unwrap().is_none());
    })
    .await;
}

/// 取り出し手が同時に動いても、ワークスペースごとの上限を超えない。
#[tokio::test]
async fn concurrent_claims_keep_the_limit() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let keys = &server.state.keys;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "busy", "blog").await;
        for _ in 0..10 {
            jobs::enqueue(&server.pool, keys, Some(ws), "test", b"x", 3)
                .await
                .unwrap();
        }
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let (pool, state) = (server.pool.clone(), server.state.clone());
            tasks.push(tokio::spawn(async move {
                jobs::claim(&pool, &state.keys).await.unwrap().is_some()
            }));
        }
        let mut got = 0;
        for t in tasks {
            got += usize::from(t.await.unwrap());
        }
        assert_eq!(got, 2);
    })
    .await;
}
