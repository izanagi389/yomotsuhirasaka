//! 関連記事の固定・除外の管理APIの結合テスト（応答の今の一覧）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。LLMの接続先は設定しないので、関連記事の候補は計算されず、
//! 今の一覧には固定した記事だけが出る（候補の計算と照合は、約束のテスト INV-REL-01・INV-REL-02 で確かめる）。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};
use uuid::Uuid;

async fn published(server: &Server, editor: &Login, ws: Uuid, slug: &str) -> (String, String) {
    let entries = format!("/api/admin/workspaces/{ws}/entries");
    let create = json!({"contentType": "post", "fields": {"title": slug, "slug": slug,
        "body": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": slug}]}]}}})
    .to_string();
    let res = send(server, "POST", &entries, editor, Some(&create)).await;
    assert_eq!(res.status, 201, "{}", res.body);
    let id = res.json_str("id").unwrap();
    let publish = json!({"revision": 1, "correction": false}).to_string();
    let res = send(
        server,
        "POST",
        &format!("{entries}/{id}/publish"),
        editor,
        Some(&publish),
    )
    .await;
    assert_eq!(res.status, 200, "{}", res.body);
    let group = get(server, &format!("{entries}/{id}"), editor)
        .await
        .json_str("translationGroupId")
        .unwrap();
    (id, group)
}

#[tokio::test]
async fn the_response_shows_the_list_returned_now() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let (a, _) = published(&server, &editor, ws, "a").await;
        let (b, group_b) = published(&server, &editor, ws, "b").await;
        let (c, group_c) = published(&server, &editor, ws, "c").await;
        let path = format!("/api/admin/workspaces/{ws}/entries/{a}/related");
        let parse = |body: &str| serde_json::from_str::<Value>(body).unwrap();

        let res = get(&server, &path, &editor).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res.body), json!({"pinned": [], "excluded": [], "current": []}));

        // 固定した記事は、固定の順に今の一覧に出る（候補にないので点数は 0）
        let body = json!({"pinned": [group_c, group_b], "excluded": []}).to_string();
        let res = send(&server, "PUT", &path, &editor, Some(&body)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let expected = json!({"pinned": [group_c, group_b], "excluded": [],
            "current": [{"entryId": c, "pinned": true, "score": 0.0}, {"entryId": b, "pinned": true, "score": 0.0}]});
        assert_eq!(parse(&res.body), expected);
        assert_eq!(parse(&get(&server, &path, &editor).await.body), expected);

        // 非公開にした記事は、固定していても今の一覧から落ちる（設定は残す）
        let res = send(&server, "POST", &format!("/api/admin/workspaces/{ws}/entries/{c}/unpublish"), &editor, None).await;
        assert_eq!(res.status, 204, "{}", res.body);
        let now = parse(&get(&server, &path, &editor).await.body);
        assert_eq!(now["pinned"], json!([group_c, group_b]));
        assert_eq!(now["current"], json!([{"entryId": b, "pinned": true, "score": 0.0}]));
    })
    .await;
}
