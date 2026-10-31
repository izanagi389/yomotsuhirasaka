//! ワークスペースの LLM の接続設定の上書き（10-llm-and-budget.md「接続設定の単位」、Q26、INV-LLM-01, 02）。
//!
//! - 上書きを許すかどうかは、サーバー管理者がワークスペースごとに決める（`workspace_limits.llm_override`。既定は許さない）。
//!   許可を取り消したら、上書きの設定（キーを含む）を消し、インスタンスの設定に戻る。
//! - 接続先は、サーバーの設定の許可の一覧（`[[llm.allowed_endpoints]]`）の名前で選ぶ。URL は受け付けない（SSRF対策）。
//!   送信は外部への送信の共通の仕組みを通る（IPアドレスの固定・内部の宛先の遮断）。
//! - APIキーは暗号化の鍵で暗号化して持つ（AAD はワークスペースIDと用途。別の行に付け替えても復号できない）。
//!   管理APIは「設定済みか」と「設定した日時」だけを返し、キーそのものは返さない。ログ・監査ログ・エラーにも出さない。
//! - 上書きしたのはチャット（回答の生成）。埋め込みはインスタンスの設定のまま（埋め込みの上書きは別の段階で足す）。
//! - 許可の一覧から名前が消えた接続先の上書きは使わず、インスタンスの設定で動く。

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::{Config, LlmConfig};
use crate::keys::KeyRing;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;
/// APIキーとモデル名の長さの上限（文字）。
const MAX_KEY_CHARS: usize = 1000;
const MAX_MODEL_CHARS: usize = 200;

/// APIキーの暗号化の AAD（ワークスペースと用途にひも付ける）。
pub fn key_aad(workspace: Uuid) -> Vec<u8> {
    let mut aad = b"workspace-llm-key:".to_vec();
    aad.extend_from_slice(workspace.as_bytes());
    aad
}

/// サーバー管理者がワークスペースに上書きを許しているか。
pub async fn allowed(pool: &PgPool, workspace: Uuid) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT llm_override FROM cms.workspace_limits WHERE workspace_id = $1")
            .bind(workspace)
            .fetch_optional(pool)
            .await?
            .unwrap_or(false),
    )
}

type Row = (
    String,
    Vec<u8>,
    String,
    Option<String>,
    Option<f64>,
    Option<f64>,
);

/// ワークスペースで使う LLM の設定（上書きがあれば、インスタンスの設定に重ねたもの）と、接続先の名前（インスタンスなら空）。
/// インスタンスに LLM の設定がなければ None。
pub async fn effective(
    pool: &PgPool,
    keys: &KeyRing,
    config: &Config,
    workspace: Uuid,
) -> Result<Option<(LlmConfig, String)>> {
    let Some(base) = config.llm.as_ref() else {
        return Ok(None);
    };
    let instance = Ok(Some((base.current(), String::new())));
    if !allowed(pool, workspace).await? {
        return instance;
    }
    let row: Option<Row> = sqlx::query_as(crate::sql!(
        "SELECT endpoint, api_key, to_char(api_key_updated_at AT TIME ZONE 'UTC', {RFC3339}),
                chat_model, chat_input_price, chat_output_price
         FROM cms.workspace_llm WHERE workspace_id = $1"
    ))
    .bind(workspace)
    .fetch_optional(pool)
    .await?;
    let Some((endpoint, sealed, _, chat_model, input_price, output_price)) = row else {
        return instance;
    };
    let Some(allowed) = base.allowed_endpoints.iter().find(|e| e.name == endpoint) else {
        tracing::warn!(workspace = %workspace, "the workspace LLM endpoint is no longer allowed; using the instance settings");
        return instance;
    };
    let key = keys
        .decrypt(&sealed, &key_aad(workspace))
        .map_err(|_| anyhow!("the workspace LLM API key could not be decrypted"))?;
    let key =
        String::from_utf8(key).map_err(|_| anyhow!("the workspace LLM API key is not text"))?;
    // 埋め込みはインスタンスの設定のまま（チャットだけを上書きする）。単価は上書きに書いたものだけを使う
    Ok(Some((
        LlmConfig {
            base_url: allowed.base_url.clone(),
            api_key: key,
            chat_model: chat_model.or_else(|| base.chat_model.clone()),
            chat_input_price_per_million_tokens: input_price,
            chat_output_price_per_million_tokens: output_price,
            ..base.current()
        },
        endpoint,
    )))
}

/// 回答の単価（100万トークンあたり、入力・出力）。上書きが効いていれば上書きに書いたもの（なければ費用を出さない）、
/// なければインスタンスの設定（キーは復号しない）。
pub async fn chat_prices(
    pool: &PgPool,
    config: &Config,
    workspace: Uuid,
) -> Result<(Option<f64>, Option<f64>)> {
    let Some(base) = config.llm.as_ref() else {
        return Ok((None, None));
    };
    let instance = (
        base.chat_input_price_per_million_tokens,
        base.chat_output_price_per_million_tokens,
    );
    if !allowed(pool, workspace).await? {
        return Ok(instance);
    }
    let row: Option<(String, Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT endpoint, chat_input_price, chat_output_price FROM cms.workspace_llm WHERE workspace_id = $1",
    )
    .bind(workspace)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some((endpoint, input, output))
            if base.allowed_endpoints.iter().any(|e| e.name == endpoint) =>
        {
            (input, output)
        }
        _ => instance,
    })
}

/// 管理APIで返す設定（キーそのものは返さない）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    /// サーバー管理者が上書きを許しているか。
    pub allowed: bool,
    /// 選べる接続先の名前（サーバーの設定の許可の一覧）。
    pub endpoints: Vec<String>,
    /// 今の上書き（なければ null。インスタンスの設定で動く）。
    pub settings: Option<Settings>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub endpoint: String,
    pub chat_model: Option<String>,
    pub chat_input_price: Option<f64>,
    pub chat_output_price: Option<f64>,
    pub api_key_set: bool,
    pub api_key_updated_at: String,
}

pub async fn view(pool: &PgPool, config: &Config, workspace: Uuid) -> Result<View> {
    let endpoints = config
        .llm
        .iter()
        .flat_map(|l| l.allowed_endpoints.iter().map(|e| e.name.clone()))
        .collect();
    let row: Option<Row> = sqlx::query_as(crate::sql!(
        "SELECT endpoint, api_key, to_char(api_key_updated_at AT TIME ZONE 'UTC', {RFC3339}),
                chat_model, chat_input_price, chat_output_price
         FROM cms.workspace_llm WHERE workspace_id = $1"
    ))
    .bind(workspace)
    .fetch_optional(pool)
    .await?;
    Ok(View {
        allowed: allowed(pool, workspace).await?,
        endpoints,
        settings: row.map(
            |(endpoint, sealed, updated, chat_model, input, output)| Settings {
                endpoint,
                chat_model,
                chat_input_price: input,
                chat_output_price: output,
                api_key_set: !sealed.is_empty(),
                api_key_updated_at: updated,
            },
        ),
    })
}

/// 管理APIの変更。接続先は名前だけ（URL の項目はない。知らない項目は受け付けない）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub endpoint: String,
    /// 新しいキー（なければ、保存してあるキーを使い続ける。初めての設定では必須）。
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub chat_model: Option<String>,
    #[serde(default)]
    pub chat_input_price: Option<f64>,
    #[serde(default)]
    pub chat_output_price: Option<f64>,
}

impl Change {
    /// 変えた項目の名前（監査ログに残す。キーの値は残さない）。
    pub fn fields(&self) -> Vec<&'static str> {
        let mut out = vec!["endpoint"];
        for (set, name) in [
            (self.api_key.is_some(), "apiKey"),
            (self.chat_model.is_some(), "chatModel"),
            (self.chat_input_price.is_some(), "chatInputPrice"),
            (self.chat_output_price.is_some(), "chatOutputPrice"),
        ] {
            if set {
                out.push(name);
            }
        }
        out
    }
}

/// 変更が拒否された理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// サーバー管理者が上書きを許していない。
    NotAllowed,
    /// 許可の一覧にない接続先。
    UnknownEndpoint,
    /// 値の誤り（説明。キーの値は含めない）。
    Invalid(String),
}

/// 上書きを保存する（なければ作る）。
pub async fn save(
    pool: &PgPool,
    keys: &KeyRing,
    config: &Config,
    workspace: Uuid,
    user: Uuid,
    change: &Change,
) -> Result<Result<(), Rejected>> {
    if !allowed(pool, workspace).await? {
        return Ok(Err(Rejected::NotAllowed));
    }
    let known = config
        .llm
        .iter()
        .flat_map(|l| l.allowed_endpoints.iter())
        .any(|e| e.name == change.endpoint);
    if !known {
        return Ok(Err(Rejected::UnknownEndpoint));
    }
    if let Some(k) = &change.api_key
        && (k.trim().is_empty()
            || k.chars().count() > MAX_KEY_CHARS
            || k.chars().any(char::is_control))
    {
        return Ok(Err(Rejected::Invalid(format!(
            "`apiKey` must be 1 to {MAX_KEY_CHARS} characters without control characters"
        ))));
    }
    if let Some(m) = &change.chat_model
        && (m.trim().is_empty()
            || m.chars().count() > MAX_MODEL_CHARS
            || m.chars().any(char::is_control))
    {
        return Ok(Err(Rejected::Invalid(format!(
            "`chatModel` must be 1 to {MAX_MODEL_CHARS} characters without control characters"
        ))));
    }
    if [change.chat_input_price, change.chat_output_price]
        .iter()
        .flatten()
        .any(|p| !p.is_finite() || *p < 0.0)
    {
        return Ok(Err(Rejected::Invalid(
            "`chatInputPrice` and `chatOutputPrice` must be non-negative numbers".to_owned(),
        )));
    }
    let sealed = match &change.api_key {
        Some(k) => Some(keys.encrypt(k.as_bytes(), &key_aad(workspace))?),
        None => None,
    };
    let mut tx = pool.begin().await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cms.workspace_llm WHERE workspace_id = $1)",
    )
    .bind(workspace)
    .fetch_one(&mut *tx)
    .await?;
    if !exists && sealed.is_none() {
        return Ok(Err(Rejected::Invalid(
            "`apiKey` is required the first time".to_owned(),
        )));
    }
    // 新しい行には、キーが必ずある（上で確かめた）。既にある行は、キーを送らなければ保存してあるキーを使い続ける
    let sql = if exists {
        "UPDATE cms.workspace_llm SET
           endpoint = $2,
           api_key = COALESCE($3, api_key),
           api_key_updated_at = CASE WHEN $3 IS NULL THEN api_key_updated_at ELSE now() END,
           chat_model = $4, chat_input_price = $5, chat_output_price = $6,
           updated_by = $7, updated_at = now()
         WHERE workspace_id = $1"
    } else {
        "INSERT INTO cms.workspace_llm (workspace_id, endpoint, api_key, chat_model, chat_input_price, chat_output_price, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)"
    };
    sqlx::query(sql)
        .bind(workspace)
        .bind(&change.endpoint)
        .bind(sealed.as_deref())
        .bind(change.chat_model.as_deref().map(str::trim))
        .bind(change.chat_input_price)
        .bind(change.chat_output_price)
        .bind(user)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

/// 上書きをやめる（インスタンスの設定に戻す。キーも消す）。消したものがなければ false。
pub async fn remove<'e>(executor: impl sqlx::PgExecutor<'e>, workspace: Uuid) -> Result<bool> {
    Ok(
        sqlx::query("DELETE FROM cms.workspace_llm WHERE workspace_id = $1")
            .bind(workspace)
            .execute(executor)
            .await?
            .rows_affected()
            > 0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_have_no_url_field_and_audit_names_only() {
        let c: Change =
            serde_json::from_str(r#"{"endpoint":"mock","apiKey":"sk-secret","chatModel":"m"}"#)
                .unwrap();
        assert_eq!(c.fields(), vec!["endpoint", "apiKey", "chatModel"]);
        // URL を直接渡す項目はない
        assert!(
            serde_json::from_str::<Change>(
                r#"{"endpoint":"mock","baseUrl":"http://169.254.169.254"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn keys_are_bound_to_the_workspace() {
        let a = Uuid::new_v4();
        assert_ne!(key_aad(a), key_aad(Uuid::new_v4()));
        assert!(key_aad(a).starts_with(b"workspace-llm-key:"));
    }
}
