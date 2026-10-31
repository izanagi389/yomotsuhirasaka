//! 埋め込みモデルの切り替え（08-search-and-related.md「モデルの変更」、Q30・Q138、M3）。
//!
//! - 切り替え先のモデルは設定の `[llm.next_embedding]` に書く。書いただけでは埋め込みを作らない。サーバー管理者が見積もりを見て
//!   開始すると（`cms.embedding_switch`）、ワークスペースごとに、検索用データのジョブが新しいモデルの埋め込みを作る。
//! - 切り替えはワークスペースごと。新しいモデルの埋め込みがそのワークスペースのすべてのチャンクにそろうまでは、検索も質問の埋め込みも
//!   旧モデルのまま（INV-EMB-01）。そろったら、ジョブが一括で切り替える（`search_models`）。
//! - 切り替えの途中に公開・更新された記事は、旧モデルの埋め込みを先に作り、新しいモデルの分も作る（記事が欠けないように）。
//! - 切り替えから [`RETENTION_DAYS`] 日は、旧モデルの埋め込みを残し、その間も旧モデルの分を作り続ける。この間に設定から
//!   切り替え先を消せば、すぐに旧モデルに戻る。期間を過ぎた・設定にないモデルの埋め込みは、ジョブが消す。
//! - 使っているモデルが設定にない（呼べない）ワークスペースは、ベクトル検索を無効として扱う（キーワード検索は動く）。
//!   その状態は、見積もりの管理APIと、サーバー管理者の「やること」に出す。埋め込みを作るのは、開始してから。

use anyhow::Result;
use sqlx::{PgConnection, PgPool};

use crate::config::{Config, LlmConfig};

/// 切り替えの後、旧モデルの埋め込みを残す日数。
pub const RETENTION_DAYS: i64 = 30;

/// モデルと次元数。
pub type ModelId = (String, u32);

fn id_of(c: &LlmConfig) -> ModelId {
    (c.embedding_model.clone(), c.embedding_dimensions)
}

/// ワークスペースのモデルの記録（`search_models`）。
#[derive(Debug, Clone, Default)]
pub struct Recorded {
    /// 今使っているモデル（記録がなければ None。設定の今のモデルを使う）。
    pub active: Option<ModelId>,
    /// 切り替える前のモデル。
    pub previous: Option<ModelId>,
    /// 切り替えから [`RETENTION_DAYS`] 日たっていないか。
    pub within_retention: bool,
    /// 予算が尽きて止まっているか。
    pub paused: bool,
}

/// 記録の行（使っているモデル・次元数、切り替える前のモデル・次元数、期間の中か、止まっているか）。
type RecordedRow = (String, i32, Option<String>, Option<i32>, bool, bool);

/// 記録を読む。`conn` はワークスペースのロールとスキーマに切り替えたトランザクション。
pub async fn recorded(conn: &mut PgConnection) -> Result<Recorded> {
    let row: Option<RecordedRow> = sqlx::query_as(crate::sql!(
        "SELECT active_model, active_dimensions, previous_model, previous_dimensions,
                coalesce(switched_at > now() - interval '{RETENTION_DAYS} days', false),
                paused_at IS NOT NULL
         FROM search_models"
    ))
    .fetch_optional(&mut *conn)
    .await?;
    let dims = |d: i32| u32::try_from(d).unwrap_or(0);
    Ok(match row {
        None => Recorded::default(),
        Some((m, d, pm, pd, within, paused)) => Recorded {
            active: Some((m, dims(d))),
            previous: pm.zip(pd).map(|(m, d)| (m, dims(d))),
            within_retention: within,
            paused,
        },
    })
}

/// サーバー管理者が開始した切り替え先のモデル（なければ None）。
pub async fn started(pool: &PgPool) -> Result<Option<ModelId>> {
    let row: Option<(String, i32)> =
        sqlx::query_as("SELECT model, dimensions FROM cms.embedding_switch")
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(m, d)| (m, u32::try_from(d).unwrap_or(0))))
}

/// ワークスペースの切り替えの状態（管理APIの `state`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// 設定に切り替え先があるが、まだ開始していない。
    NotStarted,
    /// 切り替え先の埋め込みを作っている（検索は旧モデル）。
    InProgress,
    /// 予算が尽きて止まっている（検索は旧モデル）。
    Paused,
    /// 設定のモデル（切り替え先があればそれ）で検索している。
    Switched,
    /// 使っているモデルが設定にない（ベクトル検索は使えない）。
    NeedsMigration,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            Self::NotStarted => "notStarted",
            Self::InProgress => "inProgress",
            Self::Paused => "paused",
            Self::Switched => "switched",
            Self::NeedsMigration => "needsMigration",
        }
    }
}

/// 1つのワークスペースで、どのモデルで検索し、どのモデルの埋め込みを作り、どのモデルへ切り替えるか。
#[derive(Debug, Clone)]
pub struct Plan {
    /// 検索に使うモデルの接続先（使っているモデルが設定になければ None）。
    pub serving: Option<LlmConfig>,
    /// 使っているモデル。
    pub active: ModelId,
    /// 埋め込みを作るモデルの接続先（作る順。使っているモデルが先）。
    pub embed: Vec<LlmConfig>,
    /// 埋め込みがそろったら切り替えるモデル。
    pub target: Option<ModelId>,
    /// 切り替え先として望まれているモデル（設定の切り替え先、なければ今のモデル）。
    pub desired: ModelId,
    pub state: State,
}

/// 設定・記録・開始の操作から、ワークスペースのやることを決める（DBに触れない）。
pub fn plan(llm: &LlmConfig, rec: &Recorded, started: Option<&ModelId>) -> Plan {
    let current = llm.current();
    let next = llm.next();
    let active = rec.active.clone().unwrap_or_else(|| id_of(&current));
    let serving = llm.for_model(&active.0, active.1);
    let desired_config = next.unwrap_or(current);
    let desired = id_of(&desired_config);
    let mut embed: Vec<LlmConfig> = serving.iter().cloned().collect();
    // 切り替えの後の期間は、旧モデルの分も作り続ける（切り戻せるように）
    let keep_previous = rec
        .previous
        .as_ref()
        .filter(|p| rec.within_retention && **p != active)
        .and_then(|p| llm.for_model(&p.0, p.1));
    if desired == active {
        embed.extend(keep_previous);
        return Plan {
            serving,
            active,
            embed,
            target: None,
            desired,
            state: State::Switched,
        };
    }
    let confirmed = started == Some(&desired);
    // 期間の中で、切り替える前のモデルへ戻すときは、開始の操作は要らない（埋め込みは残っている）
    let rollback = rec.within_retention && rec.previous.as_ref() == Some(&desired);
    if confirmed || rollback {
        embed.push(desired_config);
    }
    // 使っているモデルが呼べないときは、埋め込みがそろっていれば（費用はかからないので）切り替える
    let target = (confirmed || rollback || serving.is_none()).then(|| desired.clone());
    let state = if serving.is_none() && !(confirmed || rollback) {
        State::NeedsMigration
    } else if !(confirmed || rollback) {
        State::NotStarted
    } else if rec.paused {
        State::Paused
    } else {
        State::InProgress
    };
    Plan {
        serving,
        active,
        embed,
        target,
        desired,
        state,
    }
}

/// 埋め込みを残すモデル（設定のモデル、使っているモデル、期間の中の切り替える前のモデル）。ほかは消す。
pub fn kept(llm: &LlmConfig, rec: &Recorded, plan: &Plan) -> Vec<ModelId> {
    let mut keep = vec![id_of(&llm.current()), plan.active.clone()];
    keep.extend(llm.next().as_ref().map(id_of));
    if rec.within_retention {
        keep.extend(rec.previous.clone());
    }
    keep.sort();
    keep.dedup();
    keep
}

/// 検索に使うモデルの接続先（ワークスペースの記録と設定から）。LLMの接続先がない・使っているモデルが設定になければ None。
pub async fn serving(config: &Config, conn: &mut PgConnection) -> Result<Option<LlmConfig>> {
    let Some(llm) = &config.llm else {
        return Ok(None);
    };
    let rec = recorded(conn).await?;
    let active = rec
        .active
        .unwrap_or_else(|| (llm.embedding_model.clone(), llm.embedding_dimensions));
    Ok(llm.for_model(&active.0, active.1))
}

/// ワークスペースの切り替えの状態と見積もり。
#[derive(Debug, Clone)]
pub struct Status {
    pub state: State,
    /// 今使っているモデル。
    pub active: ModelId,
    /// 切り替え先として望まれているモデルの埋め込みが、まだないチャンクの数。
    pub chunks_to_embed: i64,
    /// そのトークン数の見積もり（1文字を1トークンとして多めに数える。予算の確保と同じ。Q138）。
    pub estimated_tokens: i64,
    /// 費用の見積もり（そのモデルの単価があるときだけ）。
    pub estimated_cost: Option<f64>,
}

/// ワークスペースの切り替えの状態と見積もりを読む。LLMの接続先がなければ None。
pub async fn status(
    pool: &PgPool,
    config: &Config,
    db: &crate::workspace_db::WorkspaceDb,
    started: Option<&ModelId>,
) -> Result<Option<Status>> {
    let Some(llm) = &config.llm else {
        return Ok(None);
    };
    let mut tx = crate::workspace_db::begin(pool, db).await?;
    let rec = recorded(&mut tx).await?;
    let p = plan(llm, &rec, started);
    // 埋め込みに渡す文字列（パンくずと本文。chunk::embedding_input）の長さで数える。同じテキストは1回だけ
    let (chunks, tokens): (i64, i64) = sqlx::query_as(
        "WITH missing AS (
           SELECT c.content_hash,
                  char_length(c.content)
                    + CASE WHEN cardinality(c.breadcrumb) = 0 THEN 0
                           ELSE char_length(array_to_string(c.breadcrumb, ' > ')) + 1 END AS chars
           FROM search_chunks c
           WHERE NOT EXISTS (SELECT 1 FROM search_embeddings x
                             WHERE x.content_hash = c.content_hash AND x.model = $1 AND x.dimensions = $2))
         SELECT (SELECT count(*) FROM missing),
                (SELECT coalesce(sum(chars), 0)::bigint FROM (SELECT DISTINCT ON (content_hash) chars FROM missing) d)",
    )
    .bind(&p.desired.0)
    .bind(i32::try_from(p.desired.1).unwrap_or(i32::MAX))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let price = llm
        .for_model(&p.desired.0, p.desired.1)
        .and_then(|c| c.embedding_price_per_million_tokens);
    Ok(Some(Status {
        state: p.state,
        active: p.active,
        chunks_to_embed: chunks,
        estimated_tokens: tokens,
        estimated_cost: price.map(|pr| pr * tokens as f64 / 1_000_000.0),
    }))
}

/// 開始する（切り替え先のモデルを記録する。前の記録は書き直す）。
pub async fn start(pool: &PgPool, model: &ModelId, user_id: uuid::Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO cms.embedding_switch (id, model, dimensions, started_by, started_at)
         VALUES (true, $1, $2, $3, now())
         ON CONFLICT (id) DO UPDATE SET model = $1, dimensions = $2, started_by = $3, started_at = now()",
    )
    .bind(&model.0)
    .bind(i32::try_from(model.1).unwrap_or(i32::MAX))
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 使っているモデルが設定にないワークスペースを、起動のときにログに出す（ワークスペースのIDだけ）。
pub async fn warn_unconfigured(pool: &PgPool, config: &Config) -> Result<()> {
    if config.llm.is_none() {
        return Ok(());
    }
    let ids: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM cms.workspaces ORDER BY created_at")
            .fetch_all(pool)
            .await?;
    for id in ids {
        let Some(db) = crate::workspace_db::lookup(pool, id).await? else {
            continue;
        };
        let mut tx = crate::workspace_db::begin(pool, &db).await?;
        let serving = serving(config, &mut tx).await?;
        tx.commit().await?;
        if serving.is_none() {
            tracing::warn!(
                workspace_id = %id,
                "the embedding model this workspace uses is not in the config; vector search is disabled \
                 until a server administrator starts the switch (GET /api/admin/embedding-switch)"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NextEmbeddingConfig;

    fn llm(next: Option<(&str, u32)>) -> LlmConfig {
        LlmConfig {
            base_url: "https://llm.example.test/v1".into(),
            api_key: "k".into(),
            embedding_model: "old".into(),
            embedding_dimensions: 8,
            embedding_price_per_million_tokens: None,
            default_workspace_embedding_tokens_per_month: None,
            default_workspace_embedding_tokens_per_day: None,
            chat_model: None,
            chat_input_price_per_million_tokens: None,
            chat_output_price_per_million_tokens: None,
            default_workspace_answer_input_tokens_per_month: None,
            default_workspace_answer_input_tokens_per_day: None,
            default_workspace_answer_output_tokens_per_month: None,
            default_workspace_answer_output_tokens_per_day: None,
            next_embedding: next.map(|(m, d)| NextEmbeddingConfig {
                model: m.into(),
                dimensions: d,
                base_url: None,
                api_key: None,
                embedding_price_per_million_tokens: None,
            }),
            allowed_endpoints: Vec::new(),
        }
    }

    fn id(m: &str, d: u32) -> ModelId {
        (m.to_owned(), d)
    }

    fn models(p: &Plan) -> Vec<ModelId> {
        p.embed.iter().map(id_of).collect()
    }

    #[test]
    fn without_a_next_model_the_current_one_is_used() {
        let p = plan(&llm(None), &Recorded::default(), None);
        assert_eq!(p.state, State::Switched);
        assert_eq!(p.active, id("old", 8));
        assert_eq!(models(&p), vec![id("old", 8)]);
        assert!(p.target.is_none());
    }

    #[test]
    fn a_next_model_waits_for_the_start() {
        let c = llm(Some(("new", 4)));
        let p = plan(&c, &Recorded::default(), None);
        assert_eq!(p.state, State::NotStarted);
        assert_eq!(
            models(&p),
            vec![id("old", 8)],
            "新しいモデルの埋め込みは、開始するまで作らない"
        );
        assert!(p.target.is_none());
        // 別のモデルを開始していても、設定の切り替え先でなければ始まらない
        assert_eq!(
            plan(&c, &Recorded::default(), Some(&id("other", 4))).state,
            State::NotStarted
        );
        // 開始すると、旧モデルを先に、新しいモデルも作り、そろえば切り替える。検索は旧モデルのまま
        let p = plan(&c, &Recorded::default(), Some(&id("new", 4)));
        assert_eq!(p.state, State::InProgress);
        assert_eq!(models(&p), vec![id("old", 8), id("new", 4)]);
        assert_eq!(p.target, Some(id("new", 4)));
        assert_eq!(p.serving.as_ref().map(id_of), Some(id("old", 8)));
        let paused = Recorded {
            paused: true,
            ..Recorded::default()
        };
        assert_eq!(plan(&c, &paused, Some(&id("new", 4))).state, State::Paused);
    }

    #[test]
    fn after_the_switch_the_old_model_is_kept_for_a_while() {
        let c = llm(Some(("new", 4)));
        let switched = Recorded {
            active: Some(id("new", 4)),
            previous: Some(id("old", 8)),
            within_retention: true,
            paused: false,
        };
        let p = plan(&c, &switched, Some(&id("new", 4)));
        assert_eq!(p.state, State::Switched);
        assert_eq!(p.serving.as_ref().map(id_of), Some(id("new", 4)));
        assert_eq!(
            models(&p),
            vec![id("new", 4), id("old", 8)],
            "期間の中は旧モデルの分も作る"
        );
        // 切り替え先を設定から消せば、すぐに戻る（開始の操作は要らない）
        let back = plan(&llm(None), &switched, None);
        assert!(back.serving.is_none(), "新しいモデルは、もう呼べない");
        assert_eq!(back.target, Some(id("old", 8)));
        assert_eq!(back.state, State::InProgress);
        // 期間を過ぎた後に消すと、使っているモデルが設定にない
        let expired = Recorded {
            within_retention: false,
            ..switched.clone()
        };
        let p = plan(&llm(None), &expired, None);
        assert_eq!(p.state, State::NeedsMigration);
        assert!(p.embed.is_empty(), "埋め込みは開始してから作る");
        assert_eq!(
            p.target,
            Some(id("old", 8)),
            "埋め込みがそろっていれば（費用なしで）切り替える"
        );
        assert_eq!(
            plan(&llm(None), &expired, Some(&id("old", 8))).state,
            State::InProgress
        );
    }

    #[test]
    fn embeddings_of_unused_models_are_not_kept() {
        let c = llm(None);
        let old = Recorded {
            active: Some(id("old", 8)),
            previous: Some(id("older", 8)),
            within_retention: false,
            paused: false,
        };
        let p = plan(&c, &old, None);
        assert_eq!(kept(&c, &old, &p), vec![id("old", 8)]);
        let recent = Recorded {
            within_retention: true,
            ..old
        };
        let p = plan(&c, &recent, None);
        assert_eq!(kept(&c, &recent, &p), vec![id("old", 8), id("older", 8)]);
    }
}
