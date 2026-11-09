//! メディアのアップロード・配信・削除と、ワークスペースの容量の上限の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use std::io::Cursor;

use common::content::*;
use common::server::*;
use common::with_scratch;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use serde_json::{Value, json};

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

fn error(res: &Resp) -> (u16, Option<String>) {
    (res.status, res.json_str("error"))
}

fn png(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let img = RgbImage::from_fn(w, h, |x, y| {
        Rgb([
            (x as u8).wrapping_mul(31) ^ seed,
            (y as u8).wrapping_mul(17),
            seed,
        ])
    });
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn path_of(url: &str) -> &str {
    url.strip_prefix("https://yomi.example.test").unwrap()
}

#[tokio::test]
async fn uploading_serving_and_deleting_media() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let other_ws = workspace_with_template(&server, &root, "B", "blog").await;
        for (email, role) in [("editor@example.test", "editor"), ("author@example.test", "author"), ("viewer@example.test", "viewer")] {
            create_user(&server.pool, email, false).await;
            add_member(&server, ws, email, role).await;
        }
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let author = login(&server, "author@example.test", PASSWORD).await;
        let viewer = login(&server, "viewer@example.test", PASSWORD).await;
        let media = format!("/api/admin/workspaces/{ws}/media");

        // 権限: 閲覧者・他のワークスペースの人はアップロードできない
        assert_eq!(upload(&server, &media, &viewer, "image/png", &png(8, 8, 1)).await.status, 403);
        assert_eq!(upload(&server, &format!("/api/admin/workspaces/{other_ws}/media"), &editor, "image/png", &png(8, 8, 1)).await.status, 404);

        // 中身で形式を判定する。申告の形式は見ない
        let res = upload(&server, &media, &author, "text/plain", &png(20, 10, 1)).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let uploaded = parse(&res);
        assert_eq!((uploaded["contentType"].as_str(), uploaded["width"].as_i64(), uploaded["height"].as_i64()), (Some("image/png"), Some(20), Some(10)));
        let url = uploaded["url"].as_str().unwrap().to_owned();
        let id = uploaded["id"].as_str().unwrap().to_owned();
        assert!(url.starts_with(&format!("https://yomi.example.test/media/{}/", ws.simple())), "{url}");
        for (bad, ct) in [
            (b"<html><script>alert(1)</script></html>".to_vec(), "image/png"),
            (b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>".to_vec(), "image/svg+xml"),
            (b"%PDF-1.4\n".to_vec(), "image/jpeg"),
            (Vec::new(), "image/png"),
        ] {
            let res = upload(&server, &media, &editor, ct, &bad).await;
            assert_eq!(error(&res), (415, Some("unsupported_media_type".into())), "{}", res.body);
        }
        // 画素数の上限（ヘッダーで巨大な大きさを宣言する画像）
        let mut huge = png(1, 1, 0);
        huge[16..24].copy_from_slice(&[0, 1, 0x86, 0xa0, 0, 1, 0x86, 0xa0]);
        let res = upload(&server, &media, &editor, "image/png", &huge).await;
        assert!(res.status == 422, "{}", res.body);
        // ファイルの大きさの上限
        let res = upload(&server, &media, &editor, "image/png", &vec![0u8; 21 * 1024 * 1024]).await;
        assert_eq!(res.status, 413, "{}", res.body);

        // 配信: 認証なしで取得できる。スクリプトの実行を禁じるヘッダーが付く（INV-MED-03）
        let res = request_bytes(&server, "GET", path_of(&url), &[], None).await;
        assert_eq!(res.status, 200);
        assert_eq!(res.header("content-type"), Some("image/png"));
        assert_eq!(res.header("x-content-type-options"), Some("nosniff"));
        assert!(res.header("content-security-policy").unwrap().contains("sandbox"));
        assert_eq!(image::guess_format(&res.body).unwrap(), ImageFormat::Png);
        // 少しずつ送っても、大きさは先に伝える
        assert_eq!(res.header("content-length"), Some(res.body.len().to_string().as_str()));
        // 決まった形でないパスは見つからない
        let file = url.rsplit('/').next().unwrap().to_owned();
        for bad in [
            format!("/media/{}/..%2F..%2Fetc%2Fpasswd", ws.simple()),
            format!("/media/{}/{}", ws.simple(), file.replace(".png", ".svg")),
            format!("/media/{}/{}", other_ws.simple(), file),
            format!("/media/x/{file}"),
        ] {
            assert_eq!(request_bytes(&server, "GET", &bad, &[], None).await.status, 404, "{bad}");
        }

        // 一覧と1件: 閲覧者も読める。ファイル名は利用者が送ったものを使わない
        let list = get(&server, &media, &viewer).await;
        assert_eq!(list.status, 200);
        assert!(list.body.contains(&id));
        let detail = parse(&get(&server, &format!("{media}/{id}"), &viewer).await);
        assert_eq!(detail["usage"], json!([]));

        // 記事で使う: 下書きと版で使われていることが分かる
        let body = json!({"type": "doc", "content": [{"type": "image", "attrs": {"mediaId": id.to_uppercase(), "alt": "図"}}]});
        let create = json!({"contentType": "post", "fields": {"title": "画像", "slug": "with-image", "body": body}}).to_string();
        let res = send(&server, "POST", &format!("/api/admin/workspaces/{ws}/entries"), &editor, Some(&create)).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let entry = parse(&res)["id"].as_str().unwrap().to_owned();
        assert_eq!(parse(&res)["fields"]["body"]["content"][0]["attrs"]["mediaId"], id.as_str(), "media IDs are normalized");
        let res = send(&server, "POST", &format!("/api/admin/workspaces/{ws}/entries/{entry}/publish"), &editor, Some("{\"revision\":1}")).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let detail = parse(&get(&server, &format!("{media}/{id}"), &viewer).await);
        assert_eq!(detail["usage"], json!([{"entryId": entry, "locale": "ja", "inDraft": true, "versions": [1]}]));

        // 削除: 執筆者・閲覧者はできない。使われていれば確認を求め、force で削除する
        let item = format!("{media}/{id}");
        assert_eq!(send(&server, "DELETE", &item, &author, None).await.status, 403);
        assert_eq!(send(&server, "DELETE", &item, &viewer, None).await.status, 403);
        let res = send(&server, "DELETE", &item, &editor, None).await;
        assert_eq!(error(&res), (409, Some("media_in_use".into())));
        assert!(res.body.contains(&entry));
        assert_eq!(request_bytes(&server, "GET", path_of(&url), &[], None).await.status, 200, "not deleted yet");
        assert_eq!(send(&server, "DELETE", &format!("{item}?force=true"), &editor, None).await.status, 204);
        assert_eq!(request_bytes(&server, "GET", path_of(&url), &[], None).await.status, 404);
        assert!(!server.state.config.media.root.join(ws.simple().to_string()).join(&file).exists());
        assert_eq!(get(&server, &item, &viewer).await.status, 404);
        assert!(!get(&server, &media, &viewer).await.body.contains(&id));
        assert_eq!(send(&server, "DELETE", &format!("{item}?force=true"), &editor, None).await.status, 404);
        // 使われていないメディアは確認なしで削除できる
        let res = upload(&server, &media, &editor, "image/png", &png(8, 8, 2)).await;
        let unused = parse(&res)["id"].as_str().unwrap().to_owned();
        assert_eq!(send(&server, "DELETE", &format!("{media}/{unused}"), &editor, None).await.status, 204);

        // 容量の上限: サーバー管理者だけが変えられる（INV-MED-05）
        let limits = format!("/api/admin/workspaces/{ws}/limits");
        assert_eq!(parse(&get(&server, &limits, &viewer).await)["mediaBytes"], 5_i64 * 1024 * 1024 * 1024);
        assert_eq!(send(&server, "PUT", &limits, &editor, Some("{\"mediaBytes\":1000}")).await.status, 403);
        assert_eq!(send(&server, "PUT", &limits, &root, Some("{\"mediaBytes\":-1}")).await.status, 400);
        let first = upload(&server, &media, &editor, "image/png", &png(60, 60, 3)).await;
        assert_eq!(first.status, 201);
        let size = parse(&first)["bytes"].as_i64().unwrap();
        let res = send(&server, "PUT", &limits, &root, Some(&json!({"mediaBytes": size * 2 + 10}).to_string())).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res)["mediaBytesUsed"], size);
        // 天井だけを変えても、容量の上限は変わらない（書いた項目だけを変える）
        let res = send(&server, "PUT", &limits, &root, Some("{\"embeddingTokensPerMonth\":10}")).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res)["mediaBytes"], size * 2 + 10);
        assert_eq!(upload(&server, &media, &editor, "image/png", &png(60, 60, 3)).await.status, 201);
        let res = upload(&server, &media, &editor, "image/png", &png(60, 60, 3)).await;
        assert_eq!(error(&res), (413, Some("media_quota_exceeded".into())), "{}", res.body);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'workspace.limits_changed'")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        // 容量の上限の変更と、天井だけの変更の2回（変更のたびに残る）
        assert_eq!(n, 2);
        // 既定値に戻す
        let res = send(&server, "PUT", &limits, &root, Some("{\"mediaBytes\":null}")).await;
        assert_eq!(parse(&res)["mediaBytes"], 5_i64 * 1024 * 1024 * 1024);
        assert_eq!(upload(&server, &media, &editor, "image/png", &png(60, 60, 3)).await.status, 201);
        // 拒否されたアップロードのファイルは残らない
        let files = std::fs::read_dir(server.state.config.media.root.join(ws.simple().to_string())).unwrap().count();
        assert_eq!(files, 3);
    })
    .await;
}

/// 同時配信の枠が空いていなければ、ファイルを開かずに 503 を返す。
#[tokio::test]
async fn serving_is_refused_when_no_slot_is_free() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.media.max_concurrent_serves = 0;
        let server = start(cfg).await;
        let path = format!(
            "/media/{}/{}.png",
            uuid::Uuid::new_v4().simple(),
            "a".repeat(32)
        );
        assert_eq!(
            request_bytes(&server, "GET", &path, &[], None).await.status,
            503
        );
    })
    .await;
}
