//! 管理API: テンプレートのデプロイと、ワークスペースのスキーマ情報。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/templates/{name}/deploy | テンプレートをデプロイする（デプロイの範囲の個人トークンだけ）。`dryRun` なら検査だけ |
//! | GET | /api/admin/workspaces/{ws}/schema | ワークスペースに適用済みのスキーマ（`devNote` は除く） |
//! | GET | /api/admin/bundled-templates | 同梱のテンプレートの一覧と、デプロイ済みの版との違い（サーバー管理者） |
//! | POST | /api/admin/bundled-templates/{name}/deploy | 同梱のテンプレートをデプロイする（サーバー管理者のセッションと再認証）。`dryRun` なら検査だけ |
//!
//! デプロイはサーバー管理者だけが、デプロイの範囲を持つ個人トークンで行う（13-security.md「デプロイ」、Q102）。
//! デプロイの範囲のトークンは、サーバー管理者が多要素認証で再認証してから発行する。

use axum::Json;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody, require_cookie_session};
use crate::access;
use crate::auth::{personal_tokens, session};
use crate::schema::deploy::{self, Outcome, Rejected};
use crate::schema::manifest::{Manifest, valid_template_name};

/// デプロイの範囲を持つ、サーバー管理者の個人トークン。Cookieのセッションでは受け付けない。
pub struct DeployToken(pub session::Current);

fn deploy_forbidden() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "deploy_forbidden",
        "Deploying requires a personal access token with the deploy scope of a server administrator with multi-factor authentication.",
    )
}

impl FromRequestParts<AppState> for DeployToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let Some(value) = parts.headers.get(header::AUTHORIZATION) else {
            return Err(ApiError::unauthenticated());
        };
        let presented = value
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .ok_or_else(ApiError::unauthenticated)?;
        let current = personal_tokens::resolve(&state.pool, presented)
            .await?
            .ok_or_else(ApiError::unauthenticated)?;
        let allowed = current
            .grant
            .as_ref()
            .is_some_and(|g| g.has_scope("deploy"))
            && current.is_server_admin
            && current.mfa_enabled;
        if !allowed {
            return Err(deploy_forbidden());
        }
        Ok(Self(current))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeployRequest {
    manifest: Manifest,
    /// 破壊的な変更（フィールドの削除など）を許す（Q53）。
    #[serde(default)]
    allow_destructive: bool,
    /// 登録済みのクエリを壊す変更を許す（Q54）。
    #[serde(default)]
    allow_broken_queries: bool,
    /// 試行: デプロイと同じ検査だけを行い、何も書き込まない（CLIがデプロイの前に結果を見せるため）。
    #[serde(default)]
    dry_run: bool,
}

/// デプロイの応答（試行なら検査の結果、そうでなければデプロイの結果）。
#[derive(Serialize)]
#[serde(untagged)]
pub enum DeployResponse {
    Applied(Outcome),
    Checked(deploy::Check),
}

/// 応答に載せる問題の数の上限。
const MAX_REPORTED: usize = 50;

fn report(items: impl Iterator<Item = String>) -> String {
    items.take(MAX_REPORTED).collect::<Vec<_>>().join("; ")
}

pub async fn deploy_template(
    State(state): State<AppState>,
    DeployToken(current): DeployToken,
    Path(name): Path<String>,
    JsonBody(req): JsonBody<DeployRequest>,
) -> Result<Json<DeployResponse>, ApiError> {
    if !valid_template_name(&name) {
        return Err(ApiError::bad_request(
            "The template name must be 1 to 64 characters of a-z, 0-9 and -.",
        ));
    }
    let flags = req.flags();
    run_deploy(&state, &name, req.manifest, flags, current.user_id).await
}

/// デプロイの指定（マニフェスト以外）。
#[derive(Debug, Clone, Copy, Default)]
pub struct DeployFlags {
    allow_destructive: bool,
    allow_broken_queries: bool,
    dry_run: bool,
}

impl DeployRequest {
    fn flags(&self) -> DeployFlags {
        DeployFlags {
            allow_destructive: self.allow_destructive,
            allow_broken_queries: self.allow_broken_queries,
            dry_run: self.dry_run,
        }
    }
}

/// デプロイ（トークンの経路と、同梱のテンプレートの経路で共通）。検査はどちらも同じに行う。
async fn run_deploy(
    state: &AppState,
    name: &str,
    manifest: Manifest,
    req: DeployFlags,
    actor: Uuid,
) -> Result<Json<DeployResponse>, ApiError> {
    let queries = deploy::QueryCheck {
        allow_broken: req.allow_broken_queries,
        limits: crate::delivery::schema::QueryLimits {
            depth: state.config.delivery.max_depth,
            complexity: state.config.delivery.max_complexity,
        },
        unused_days: state.config.delivery.query_check_unused_days,
    };
    if req.dry_run {
        let checked =
            deploy::check(&state.scripts, &state.migration, name, &manifest, queries).await?;
        return Ok(Json(DeployResponse::Checked(checked)));
    }
    match deploy::deploy(
        &state.scripts,
        &state.migration,
        name,
        manifest,
        req.allow_destructive,
        queries,
        actor,
    )
    .await?
    {
        Ok(outcome) => {
            // URLの決め方が変わったコンテンツタイプは、移行が済んだワークスペースの公開中の記事のURLを再計算する（Q39）
            for ws in outcome.workspaces.iter().filter(|w| w.state == "ok") {
                // 変換結果のない・古い変換器で作った公開中の版を作り直す（Q58。移行待ちだった版もここで拾う）
                if let Err(e) =
                    crate::content::rerender::schedule(&state.pool, &state.keys, ws.workspace_id)
                        .await
                {
                    tracing::error!(workspace_id = %ws.workspace_id, "cannot queue re-rendering: {e:#}");
                }
                // 検索の対象のフィールドが変わったかもしれないので、検索用データを見直す
                super::admin_publishing::schedule_search(state, ws.workspace_id).await;
                for ct in &outcome.url_rule_changes {
                    if let Err(e) = crate::content::urls::schedule_content_type(
                        &state.pool,
                        &state.keys,
                        ws.workspace_id,
                        ct,
                    )
                    .await
                    {
                        tracing::error!(workspace_id = %ws.workspace_id, "cannot queue URL recomputation: {e:#}");
                    }
                }
            }
            Ok(Json(DeployResponse::Applied(outcome)))
        }
        Err(Rejected::Invalid(problems)) => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_manifest",
            format!(
                "The template is not valid: {}",
                report(problems.into_iter())
            ),
        )),
        Err(Rejected::Destructive(changes)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "destructive_change",
            format!(
                "The deployment contains destructive changes. Add a field, migrate the data and remove the old one in separate deployments, or allow destructive changes explicitly: {}",
                report(changes.iter().map(|c| c.describe()))
            ),
        )),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    template: String,
    /// 適用済みの版（まだなければ null）。
    version: Option<i32>,
    /// none: まだ適用していない、ok: 最新、pending: 移行待ち
    state: String,
    /// 移行待ちのとき、適用しようとした版。
    #[serde(skip_serializing_if = "Option::is_none")]
    target_version: Option<i32>,
    /// 移行待ちの理由のコード。
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// 適用済みのスキーマ（`devNote` は除く。INV-SCH-01）。
    manifest: Option<Manifest>,
}

/// ワークスペースに適用済みのスキーマ。所属している人なら見られる。
pub async fn schema_info(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<SchemaInfo>, ApiError> {
    let id = Uuid::parse_str(&ws).map_err(|_| ApiError::not_found())?;
    access::member(&state.pool, &current, id).await??;
    let (template, version, schema_state, target_version, error): (
        String,
        Option<i32>,
        String,
        Option<i32>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT template, schema_version, schema_state, schema_target_version, schema_error
         FROM yomi.workspaces WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    let manifest = match version {
        Some(v) => deploy::load_version(&state.pool, &template, v)
            .await?
            .map(|m| m.for_members()),
        None => None,
    };
    Ok(Json(SchemaInfo {
        template,
        version,
        state: schema_state,
        target_version,
        error,
        manifest,
    }))
}

/// 同梱のテンプレートの状態。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundledTemplate {
    name: &'static str,
    /// デプロイ済みの最新の版（まだなら null）。
    deployed_version: Option<i32>,
    /// デプロイ済みの最新の版が、同梱の中身と同じか（サイトのオリジンを除いて比べる）。
    up_to_date: bool,
    /// デプロイ済みの版のサイトのオリジン（まだなら null）。
    site_url: Option<String>,
    /// このテンプレートを使うワークスペースの数。
    workspaces: i64,
}

fn server_admin_session(current: &session::Current) -> Result<(), ApiError> {
    require_cookie_session(current)?;
    access::server_admin(current)?;
    Ok(())
}

/// 同梱のテンプレートの一覧（サーバー管理者）。
pub async fn bundled_templates(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<Vec<BundledTemplate>>, ApiError> {
    server_admin_session(&current)?;
    // 読むだけなので普段用の接続を使う（移行用の接続は、デプロイとワークスペースの作成だけに使う。Q179）
    let mut conn = state.pool.acquire().await.map_err(anyhow::Error::from)?;
    let mut out = Vec::new();
    for name in crate::schema::bundled::names() {
        let latest = deploy::load_latest(&mut conn, name).await?;
        let workspaces: i64 =
            sqlx::query_scalar("SELECT count(*) FROM yomi.workspaces WHERE template = $1")
                .bind(name)
                .fetch_one(&mut *conn)
                .await
                .map_err(anyhow::Error::from)?;
        out.push(BundledTemplate {
            name,
            deployed_version: latest.as_ref().map(|(v, _)| *v),
            up_to_date: latest
                .as_ref()
                .is_some_and(|(_, m)| crate::schema::bundled::same_as_bundled(name, m)),
            site_url: latest
                .as_ref()
                .and_then(|(_, m)| crate::schema::bundled::origin_of(m)),
            workspaces,
        });
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundledDeployRequest {
    /// 記事の公開URLのサイトのオリジン（`https://example.com` の形）。見本のひな形の先頭を、これに置き換える。
    site_url: String,
    #[serde(default)]
    allow_destructive: bool,
    #[serde(default)]
    allow_broken_queries: bool,
    #[serde(default)]
    dry_run: bool,
}

/// 同梱のテンプレートをデプロイする（サーバー管理者のセッションと、多要素認証での再認証）。
/// 送れるのはサイトのオリジンと指定（試行・破壊的な変更・壊れるクエリ）だけで、マニフェストはサーバーに埋め込んだものを使う。
pub async fn deploy_bundled_template(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(name): Path<String>,
    JsonBody(req): JsonBody<BundledDeployRequest>,
) -> Result<Json<DeployResponse>, ApiError> {
    server_admin_session(&current)?;
    if !crate::schema::bundled::names().any(|n| n == name) {
        return Err(ApiError::not_found());
    }
    let origin = crate::schema::bundled::site_origin(&req.site_url).ok_or_else(|| {
        ApiError::bad_request(
            "`siteUrl` must be the origin of the site, such as https://example.com (http only for localhost).",
        )
    })?;
    let manifest =
        crate::schema::bundled::manifest_for(&name, &origin).ok_or_else(ApiError::not_found)?;
    let flags = DeployFlags {
        allow_destructive: req.allow_destructive,
        allow_broken_queries: req.allow_broken_queries,
        dry_run: req.dry_run,
    };
    // 試行は書き込まないので、再認証を求めない（画面がデプロイの前に結果を見せるため）
    if !flags.dry_run && !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    run_deploy(&state, &name, manifest, flags, current.user_id).await
}
