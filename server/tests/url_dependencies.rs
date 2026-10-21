//! URL生成関数の依存の記録と再計算の結合テスト（本物のPostgreSQL、組み込みJS、ジョブ）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use cms_server::worker;
use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};

/// 子の記事のURLに、親の記事（subtitle にIDを書く）のスラッグを入れるURL生成関数。
fn manifest(prefix: &str) -> String {
    let source = format!(
        "export default function (e) {{ const pid = e.fields.subtitle; let parent = 'root'; if (pid) {{ const p = cms.content.get(pid); parent = p ? p.fields.slug : 'missing'; }} return 'https://site.example.test/{prefix}' + e.locale + '/' + parent + '/' + e.slug; }}"
    );
    let mut m: Value = serde_json::from_str(&blog_manifest()).unwrap();
    m["contentTypes"][0]["urlFunction"] =
        json!({"source": source, "permissions": ["content.read"]});
    m.to_string()
}

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

#[tokio::test]
async fn urls_follow_their_dependencies() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &manifest(""), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");

        let create = |fields: Value| json!({"contentType": "post", "fields": fields}).to_string();
        let publish = |rev: i64| json!({"revision": rev, "correction": false}).to_string();
        let res = send(
            &server,
            "POST",
            &entries,
            &editor,
            Some(&create(json!({"title": "親", "slug": "p1"}))),
        )
        .await;
        let parent = parse(&res)["id"].as_str().unwrap().to_owned();
        let res = send(
            &server,
            "POST",
            &format!("{entries}/{parent}/publish"),
            &editor,
            Some(&publish(1)),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        let res = send(
            &server,
            "POST",
            &entries,
            &editor,
            Some(&create(
                json!({"title": "子", "slug": "c", "subtitle": parent}),
            )),
        )
        .await;
        let child = parse(&res)["id"].as_str().unwrap().to_owned();
        let res = send(
            &server,
            "POST",
            &format!("{entries}/{child}/publish"),
            &editor,
            Some(&publish(1)),
        )
        .await;
        assert_eq!(parse(&res)["url"], "https://site.example.test/ja/p1/c");
        let drain = || async { while worker::run_once(&server.state).await.unwrap() {} };
        // 記事のいまのURLは、ワークスペースのロールに切り替えて読む（普段用のロールはワークスペースのスキーマを読めない）
        let child_url = || async {
            let db = cms_server::workspace_db::WorkspaceDb::names_for(ws);
            let mut tx = cms_server::workspace_db::begin(&server.pool, &db)
                .await
                .unwrap();
            let url: String = sqlx::query_scalar("SELECT url FROM entries WHERE id = $1")
                .bind(uuid::Uuid::parse_str(&child).unwrap())
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            url
        };
        drain().await;

        // 親のスラッグが変わると、子のURLも再計算され、変更履歴に残る
        let res = send(
            &server,
            "PUT",
            &format!("{entries}/{parent}/draft"),
            &editor,
            Some(&json!({"revision": 1, "fields": {"slug": "p2"}}).to_string()),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        let res = send(
            &server,
            "POST",
            &format!("{entries}/{parent}/publish"),
            &editor,
            Some(&publish(2)),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        drain().await;
        assert_eq!(child_url().await, "https://site.example.test/ja/p2/c");
        let history =
            parse(&get(&server, &format!("{entries}/{child}/url-history"), &editor).await);
        assert_eq!(history[0]["from"], "https://site.example.test/ja/p1/c");
        assert_eq!(history[0]["to"], "https://site.example.test/ja/p2/c");
        assert_eq!(
            history[0]["versionId"],
            Value::Null,
            "a recomputation does not make a version"
        );

        // 親を非公開にすると、関数は親を読めない（公開中の記事だけを読む）
        let res = send(
            &server,
            "POST",
            &format!("{entries}/{parent}/unpublish"),
            &editor,
            None,
        )
        .await;
        assert_eq!(res.status, 204, "{}", res.body);
        drain().await;
        assert_eq!(child_url().await, "https://site.example.test/ja/missing/c");

        // URL生成関数を変えてデプロイすると、そのタイプの公開中の記事が再計算される
        assert_eq!(
            deploy(&server, &token, "blog", &manifest("v2/"), "")
                .await
                .status,
            200
        );
        drain().await;
        assert_eq!(
            child_url().await,
            "https://site.example.test/v2/ja/missing/c"
        );
        // URLの変更のWebhookのジョブは、受け手がなければ積まれない。ジョブは残っていない
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.jobs WHERE failed_at IS NULL")
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    })
    .await;
}
