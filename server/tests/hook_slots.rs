//! フックを動かす処理の枠の結合テスト。枠が空かなければ、接続を取らずに断る。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::json;

#[tokio::test]
async fn publishing_is_refused_when_no_hook_slot_is_free() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "本文"}]}]});
        let res = send(&server, "POST", &entries, &root, Some(&json!({"contentType": "post", "fields": {"title": "t", "slug": "s", "body": body}}).to_string())).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let id = res.json_str("id").unwrap();
        let publish = format!("{entries}/{id}/publish");

        let all = server.state.hook_slots.acquire_many(4).await.unwrap();
        let res = send(&server, "POST", &publish, &root, Some(r#"{"revision":1}"#)).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (503, Some("server_busy")), "{}", res.body);
        drop(all);
        let res = send(&server, "POST", &publish, &root, Some(r#"{"revision":1}"#)).await;
        assert!(res.status < 300, "{}", res.body);
    })
    .await;
}
