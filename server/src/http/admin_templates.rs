//! 管理API: テンプレートのデプロイと、ワークスペースのスキーマ情報。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/templates/{name}/deploy | テンプレートをデプロイする（デプロイの範囲の個人トークンだけ）。`dryRun` なら検査だけ |
//! | GET | /api/admin/workspaces/{ws}/schema | ワークスペースに適用済みのスキーマ（`devNote` は除く） |
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
use super::extract::{AuthSession, JsonBody};
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
    let queries = deploy::QueryCheck {
        allow_broken: req.allow_broken_queries,
        limits: crate::delivery::schema::QueryLimits {
            depth: state.config.delivery.max_depth,
            complexity: state.config.delivery.max_complexity,
        },
        unused_days: state.config.delivery.query_check_unused_days,
    };
    if req.dry_run {
        let checked = deploy::check(
            &state.scripts,
            &state.migration,
            &name,
            &req.manifest,
            queries,
        )
        .await?;
        return Ok(Json(DeployResponse::Checked(checked)));
    }
    match deploy::deploy(
        &state.scripts,
        &state.migration,
        &state.pool,
        &name,
        req.manifest,
        req.allow_destructive,
        queries,
        current.user_id,
    )
    .await?
    {
        Ok(outcome) => {
            // URLの決め方が変わったコンテンツタイプは、移行が済んだワークスペースの公開中の記事のURLを再計算する（Q39）
            for ws in outcome.workspaces.iter().filter(|w| w.state == "ok") {
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
        Err(Rejected::BreaksQueries(broken)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "breaks_registered_queries",
            format!(
                "The deployment breaks registered queries. Update the site's queries and register them again, deprecate the field first, or allow broken queries explicitly: {}",
                report(broken.iter().map(|b| format!(
                    "workspace {} query {}: {}",
                    b.workspace_id,
                    b.query_id,
                    b.problems.join(", ")
                )))
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
         FROM cms.workspaces WHERE id = $1",
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
