//! テンプレートのデプロイと記事のテストの補助。
#![allow(dead_code)]

use super::server::*;
use uuid::Uuid;

/// テストのテンプレート（ブログ）。
pub fn blog_manifest() -> String {
    r#"{
      "locales": ["ja", "en"],
      "defaultLocale": "ja",
      "body": {
        "nodes": [
          {"name": "doc", "children": ["paragraph", "heading", "bulletList", "image", "embed"]},
          {"name": "paragraph", "children": ["text"], "marks": true},
          {"name": "heading", "children": ["text"], "attrs": ["level"], "marks": true},
          {"name": "bulletList", "children": ["listItem"]},
          {"name": "listItem", "children": ["paragraph", "bulletList"]},
          {"name": "text"},
          {"name": "image", "attrs": ["mediaId", "alt", "decorative"]},
          {"name": "embed", "attrs": ["src"]}
        ],
        "marks": [{"name": "bold"}, {"name": "link", "attrs": ["href", "target", "rel"]}],
        "embedProviders": ["www.youtube.com"]
      },
      "contentTypes": [{
        "name": "post",
        "urlTemplate": "https://site.example.test/{locale}/blog/{slug}",
        "listLimit": 5,
        "fields": [
          {"name": "title", "type": "text", "required": true},
          {"name": "slug", "type": "slug", "required": true},
          {"name": "subtitle", "type": "text"},
          {"name": "body", "type": "richtext"},
          {"name": "related", "type": "reference", "referenceTo": "post", "multiple": true}
        ]
      }]
    }"#
    .to_owned()
}

/// サーバー管理者（多要素認証あり）を作り、デプロイの範囲のトークンを発行する。
pub async fn root_with_deploy_token(server: &Server) -> (Login, String) {
    create_user(&server.pool, "root@example.test", true).await;
    let root = login(server, "root@example.test", PASSWORD).await;
    enroll(server, &root).await;
    let res = send(
        server,
        "POST",
        "/api/admin/me/tokens",
        &root,
        Some("{\"name\":\"deploy\",\"scopes\":[\"deploy\"],\"workspaceIds\":[]}"),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    (root, res.json_str("token").unwrap())
}

pub async fn deploy(
    server: &Server,
    token: &str,
    template: &str,
    manifest: &str,
    flags: &str,
) -> Resp {
    request(
        server,
        "POST",
        &format!("/api/admin/templates/{template}/deploy"),
        &[("Authorization", &format!("Bearer {token}"))],
        Some(&format!("{{\"manifest\":{manifest}{flags}}}")),
    )
    .await
}

pub async fn workspace_with_template(
    server: &Server,
    root: &Login,
    name: &str,
    template: &str,
) -> Uuid {
    let res = send(
        server,
        "POST",
        "/api/admin/workspaces",
        root,
        Some(&format!(
            "{{\"name\":\"{name}\",\"template\":\"{template}\"}}"
        )),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("id").unwrap().parse().unwrap()
}

pub async fn add_member(server: &Server, ws: Uuid, email: &str, role: &str) {
    sqlx::query(
        "INSERT INTO yomi.memberships (workspace_id, user_id, role) SELECT $1, id, $3 FROM yomi.users WHERE email = $2",
    )
    .bind(ws)
    .bind(email)
    .bind(role)
    .execute(&server.pool)
    .await
    .unwrap();
}
