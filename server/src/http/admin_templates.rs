//! 管理API: テンプレートのデプロイと、ワークスペースのスキーマ情報。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | POST | /api/admin/templates/{name}/deploy | テンプレートをデプロイする（デプロイの範囲の個人トークンだけ）。`dryRun` なら検査だけ |
//! | GET | /api/admin/workspaces/{ws}/schema | ワークスペースに適用済みのスキーマ（`devNote` は除く） |
//! | GET | /api/admin/bundled-templates | 同梱のテンプレートの一覧と、デプロイ済みの版との違い（サーバー管理者） |
//! | POST | /api/admin/bundled-templates/{name}/deploy | 同梱のテンプレートをデプロイする（サーバー管理者のセッションと再認証）。`dryRun` なら検査だけ |
//! | POST | /api/admin/workspaces/{ws}/template | テンプレートを切り替える（サーバー管理者と、権限を付与された管理者。再認証）。`dryRun` なら計画だけ |
//! | POST | /api/admin/workspaces/{ws}/template/export | 切り替えで消えるデータを JSON で書き出す（再認証） |
//! | GET | /api/admin/workspaces/{ws}/template-options | 切り替え先に選べるテンプレート |
//! | GET | /api/admin/templates | テンプレートの一覧（ワークスペースの作成で選ぶため。サーバー管理者） |
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
            // URLの決め方が変わったコンテンツタイプは、移行したワークスペースの公開中の記事のURLを再計算する（Q39）
            for &workspace_id in &outcome.workspaces {
                // 変換結果のない・古い変換器で作った公開中の版を作り直す（Q58）
                if let Err(e) =
                    crate::content::rerender::schedule(&state.pool, &state.keys, workspace_id).await
                {
                    tracing::error!(workspace_id = %workspace_id, "cannot queue re-rendering: {e:#}");
                }
                // 検索の対象のフィールドが変わったかもしれないので、検索用データを見直す
                super::admin_publishing::schedule_search(state, workspace_id).await;
                for ct in &outcome.url_rule_changes {
                    if let Err(e) = crate::content::urls::schedule_content_type(
                        &state.pool,
                        &state.keys,
                        workspace_id,
                        ct,
                    )
                    .await
                    {
                        tracing::error!(workspace_id = %workspace_id, "cannot queue URL recomputation: {e:#}");
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
                report(changes.iter().map(crate::schema::diff::Breaking::describe))
            ),
        )),
        Err(Rejected::BrokenQueries(broken)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "breaks_registered_queries",
            format!(
                "The deployment would break registered queries, so it was not applied to any workspace. Update the queries, or allow breaking them explicitly: {}",
                report(
                    broken
                        .iter()
                        .map(|b| format!("{} {}", b.workspace_id, b.query_id))
                )
            ),
        )),
        Err(Rejected::MigrationFailed { workspace_id, code }) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "migration_failed",
            format!(
                "The migration failed in workspace {workspace_id} ({code}), so the deployment was not applied to any workspace."
            ),
        )),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    template: String,
    /// none: まだ適用していない、ok: 適用済み
    state: String,
    /// テンプレートをデプロイした日時（まだ適用していなければ null）。
    deployed_at: Option<String>,
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
    let (template, schema_state): (String, String) =
        sqlx::query_as("SELECT template, schema_state FROM yomi.workspaces WHERE id = $1")
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .map_err(anyhow::Error::from)?;
    let deployed = if schema_state == "ok" {
        deploy::load(&state.pool, &template).await?
    } else {
        None
    };
    Ok(Json(SchemaInfo {
        template,
        state: schema_state,
        deployed_at: deployed.as_ref().map(|d| d.deployed_at.clone()),
        manifest: deployed.map(|d| d.manifest.for_members()),
    }))
}

/// 同梱のテンプレートの状態。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundledTemplate {
    name: &'static str,
    /// 最後にデプロイした日時（まだなら null）。
    deployed_at: Option<String>,
    /// デプロイ済みのものが、同梱の中身と同じか（サイトのオリジンを除いて比べる）。
    up_to_date: bool,
    /// デプロイ済みのもののサイトのオリジン（まだなら null）。
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
        let deployed = deploy::load(&mut *conn, name).await?;
        let workspaces: i64 =
            sqlx::query_scalar("SELECT count(*) FROM yomi.workspaces WHERE template = $1")
                .bind(name)
                .fetch_one(&mut *conn)
                .await
                .map_err(anyhow::Error::from)?;
        out.push(BundledTemplate {
            name,
            up_to_date: deployed
                .as_ref()
                .is_some_and(|d| crate::schema::bundled::same_as_bundled(name, &d.manifest)),
            site_url: deployed
                .as_ref()
                .and_then(|d| crate::schema::bundled::origin_of(&d.manifest)),
            deployed_at: deployed.map(|d| d.deployed_at),
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

// ---- テンプレートの切り替え（02-schema-and-delivery-api.md「テンプレートの切り替え」、INV-SCH-08・09） ----

/// 切り替えられる人: サーバー管理者と、重い権限「テンプレートの切り替え」を付与されたワークスペースの管理者（INV-SCH-09）。
/// Cookie のセッションだけ（個人トークンでは行えない）。管理者とサーバー管理者の多要素認証は `member` が求める。
async fn switcher(state: &AppState, current: &session::Current, ws: Uuid) -> Result<(), ApiError> {
    require_cookie_session(current)?;
    let access = access::member(&state.pool, current, ws).await??;
    let granted_admin =
        access.role == Some(access::Role::Admin) && access.has(access::Permission::SwitchTemplate);
    if !(access::acts_as_server_admin(current) || granted_admin) {
        return Err(access::Denied::Forbidden.into());
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SwitchRequest {
    template: String,
    /// 古いコンテンツタイプとフィールドを、新しいテンプレートのどれに引き継ぐか（書いていないものは名前の同じもの）。
    #[serde(default)]
    mapping: crate::schema::switch::MappingInput,
    /// データが消える切り替えを許す（INV-SCH-08）。
    #[serde(default)]
    allow_destructive: bool,
    #[serde(default)]
    allow_broken_queries: bool,
    /// 試行: 対応づけと消えるもの、壊れるクエリを返すだけで、何も変えない。
    #[serde(default)]
    dry_run: bool,
}

fn valid_target(name: &str) -> Result<(), ApiError> {
    if valid_template_name(name) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "The template name must be 1 to 64 characters of a-z, 0-9 and -.",
        ))
    }
}

fn switch_rejected(r: crate::schema::switch::Rejected) -> ApiError {
    use crate::schema::switch::Rejected as R;
    match r {
        R::NotDeployed => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "template_not_deployed",
            "The template has not been deployed yet.",
        ),
        R::SameTemplate => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "same_template",
            "The workspace already uses this template.",
        ),
        R::Mapping(problems) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_mapping",
            format!("The mapping is not valid: {}", report(problems.into_iter())),
        ),
        R::Destructive(plan) => {
            let lost = plan.content_types.iter().flat_map(|t| match &t.to {
                None => vec![t.from.clone()],
                Some(_) => t
                    .fields
                    .iter()
                    .filter(|f| f.to.is_none())
                    .map(|f| format!("{}.{}", t.from, f.from))
                    .collect(),
            });
            ApiError::new(
                StatusCode::CONFLICT,
                "destructive_change",
                format!(
                    "Switching removes data that is not mapped to the new template. Map it, or allow destructive changes explicitly: {}",
                    report(lost)
                ),
            )
        }
        R::BrokenQueries(broken) => ApiError::new(
            StatusCode::CONFLICT,
            "breaks_registered_queries",
            format!(
                "Registered queries do not work with the new template. Fix them, or allow it explicitly: {}",
                report(broken.into_iter().map(|q| q.query_id))
            ),
        ),
        R::Failed(code) => ApiError::new(
            StatusCode::CONFLICT,
            "switch_failed",
            format!("The existing data does not fit the new template: {code}"),
        ),
    }
}

/// ワークスペースのテンプレートを切り替える。`dryRun` なら計画だけを返す。本当の切り替えは再認証が要る。
pub async fn switch_template(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<SwitchRequest>,
) -> Result<Json<crate::schema::switch::Outcome>, ApiError> {
    let id = Uuid::parse_str(&ws).map_err(|_| ApiError::not_found())?;
    switcher(&state, &current, id).await?;
    valid_target(&req.template)?;
    if !req.dry_run && !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let limits = crate::delivery::schema::QueryLimits {
        depth: state.config.delivery.max_depth,
        complexity: state.config.delivery.max_complexity,
    };
    let outcome = crate::schema::switch::switch(
        &state.migration,
        id,
        crate::schema::switch::Request {
            template: req.template,
            mapping: req.mapping,
            allow_destructive: req.allow_destructive,
            allow_broken_queries: req.allow_broken_queries,
            dry_run: req.dry_run,
        },
        limits,
        state.config.delivery.query_check_unused_days,
        current.user_id,
    )
    .await?
    .map_err(switch_rejected)?;
    if !outcome.dry_run {
        // 版の変換結果を作り直し、検索用データと公開URLを見直す（デプロイの後と同じ）
        if let Err(e) = crate::content::rerender::schedule(&state.pool, &state.keys, id).await {
            tracing::error!(workspace_id = %id, "cannot queue re-rendering: {e:#}");
        }
        super::admin_publishing::schedule_search(&state, id).await;
        for ct in &outcome.content_types {
            if let Err(e) =
                crate::content::urls::schedule_content_type(&state.pool, &state.keys, id, ct).await
            {
                tracing::error!(workspace_id = %id, "cannot queue URL recomputation: {e:#}");
            }
        }
    }
    Ok(Json(outcome))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SwitchExportRequest {
    template: String,
    #[serde(default)]
    mapping: crate::schema::switch::MappingInput,
}

/// 切り替えで消えるデータを書き出す（JSON）。記事の本文を含むので、再認証が要る。何も変えない。
pub async fn switch_template_export(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<SwitchExportRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = Uuid::parse_str(&ws).map_err(|_| ApiError::not_found())?;
    switcher(&state, &current, id).await?;
    valid_target(&req.template)?;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let export = crate::schema::switch::export(&state.migration, id, &req.template, &req.mapping)
        .await?
        .map_err(switch_rejected)?;
    let mut data = export.data;
    if let Some(obj) = data.as_object_mut() {
        obj.insert("workspaceId".into(), serde_json::json!(id));
        obj.insert("template".into(), serde_json::json!(req.template));
        obj.insert(
            "plan".into(),
            serde_json::to_value(&export.plan).map_err(anyhow::Error::from)?,
        );
    }
    Ok(Json(data))
}

/// デプロイ済みのテンプレート。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateOption {
    name: String,
    /// 最後にデプロイした日時（同梱でまだデプロイしていなければ null）。
    deployed_at: Option<String>,
    /// 同梱のテンプレートか。
    bundled: bool,
}

async fn deployed_templates(state: &AppState) -> Result<Vec<(String, String)>, ApiError> {
    Ok(deploy::list(&state.pool).await?)
}

/// テンプレートの一覧（ワークスペースの作成で選ぶため。サーバー管理者）。デプロイ済みのものと、まだデプロイしていない同梱のもの。
pub async fn list_templates(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
) -> Result<Json<Vec<TemplateOption>>, ApiError> {
    server_admin_session(&current)?;
    let mut out: Vec<TemplateOption> = deployed_templates(&state)
        .await?
        .into_iter()
        .map(|(name, deployed_at)| TemplateOption {
            bundled: crate::schema::bundled::names().any(|b| b == name),
            name,
            deployed_at: Some(deployed_at),
        })
        .collect();
    for name in crate::schema::bundled::names() {
        if !out.iter().any(|t| t.name == name) {
            out.push(TemplateOption {
                name: name.to_owned(),
                deployed_at: None,
                bundled: true,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(out))
}

/// 切り替え先に選べるテンプレート（デプロイ済みで、今のもの以外）。切り替えられる人だけ。
pub async fn template_options(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<TemplateOption>>, ApiError> {
    let id = Uuid::parse_str(&ws).map_err(|_| ApiError::not_found())?;
    switcher(&state, &current, id).await?;
    let current_template: String =
        sqlx::query_scalar("SELECT template FROM yomi.workspaces WHERE id = $1")
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .map_err(anyhow::Error::from)?;
    Ok(Json(
        deployed_templates(&state)
            .await?
            .into_iter()
            .filter(|(name, _)| *name != current_template)
            .map(|(name, deployed_at)| TemplateOption {
                bundled: crate::schema::bundled::names().any(|b| b == name),
                name,
                deployed_at: Some(deployed_at),
            })
            .collect(),
    ))
}
