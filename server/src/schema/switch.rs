//! テンプレートの切り替え（02-schema-and-delivery-api.md「テンプレートの切り替え」、INV-SCH-08・09、2026-11-11）。
//!
//! ワークスペースを、今のテンプレートの適用済みの版から、デプロイ済みの別のテンプレートの最新の版へ移す。
//! 古いコンテンツタイプとフィールドを、新しいテンプレートのどれに引き継ぐかを対応づける（既定は名前の同じもの）。
//!
//! 手順（呼ぶ側のトランザクションの中で、デプロイのロックを持って行う）:
//! 1. 対応づけのないコンテンツタイプの記事を削除済みにし、その表を消す。対応づけたタイプの、対応づけのない列を消す
//! 2. 対応づけた表と列の名前を、新しい名前にする（入れ替えで名前がぶつからないよう、一度仮の名前を通す）
//! 3. 記事のコンテンツタイプの名前と、版の本文の JSON（過去の版を含む）を、対応づけに従って書き換える
//! 4. 名前を変えた古いマニフェストから新しいマニフェストへ、ふつうの移行（[`super::migrate::apply`]）を行う

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgConnection;

use super::manifest::{ContentType, Field, Manifest};
use super::migrate::{self, column_name, table_name};
use crate::db::Ident;
use crate::workspace_db::WorkspaceDb;

/// 画面から送る対応づけ（古い名前 → 新しい名前。null は対応づけない＝消える。書いていないものは名前の同じものへ）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MappingInput {
    #[serde(default)]
    pub content_types: HashMap<String, Option<String>>,
    /// `"<古いコンテンツタイプ>.<古いフィールド>"` → 対応づけたコンテンツタイプの中のフィールドの名前。
    #[serde(default)]
    pub fields: HashMap<String, Option<String>>,
}

/// フィールドの対応。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FieldPlan {
    pub from: String,
    pub to: Option<String>,
    /// 対応づけられる新しいフィールド（型が合うもの）。画面の選択肢に使う。
    pub candidates: Vec<String>,
}

/// コンテンツタイプの対応。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TypePlan {
    pub from: String,
    pub to: Option<String>,
    pub fields: Vec<FieldPlan>,
}

/// 切り替えの計画（試行の応答）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub content_types: Vec<TypePlan>,
    /// 新しいテンプレートのコンテンツタイプ（画面の選択肢に使う）。
    pub new_content_types: Vec<String>,
    /// 新しいテンプレートにあって、どこからも引き継がないコンテンツタイプとフィールド（空で始まる）。
    pub added: Vec<String>,
    /// 消えるものがあるか（対応づけのないコンテンツタイプかフィールドがある）。INV-SCH-08
    pub destructive: bool,
    /// 対応づけの誤り（あれば切り替えない）。
    pub problems: Vec<String>,
}

/// 型が合うか（参照先は、対応づけたコンテンツタイプの名前で比べる）。
fn compatible(old: &Field, new: &Field, ct_map: &BTreeMap<String, Option<String>>) -> bool {
    let mapped_ref = old
        .reference_to
        .as_ref()
        .map(|r| ct_map.get(r).cloned().flatten().unwrap_or_default());
    old.kind == new.kind && old.multiple == new.multiple && mapped_ref == new.reference_to
}

/// 対応づけを決めて確かめる。`old` がなければ（まだ適用していない）何も引き継がない。
pub fn plan(old: Option<&Manifest>, new: &Manifest, input: &MappingInput) -> Plan {
    let mut problems = Vec::new();
    let new_types: BTreeMap<&str, &ContentType> = new
        .content_types
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();
    let old_types: Vec<&ContentType> = old
        .map(|m| m.content_types.iter().collect())
        .unwrap_or_default();
    for key in input.content_types.keys() {
        if !old_types.iter().any(|c| &c.name == key) {
            problems.push(format!(
                "contentTypes.{key}: not a content type of the current template"
            ));
        }
    }
    // コンテンツタイプ
    let mut ct_map: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut targets: BTreeSet<String> = BTreeSet::new();
    for oct in &old_types {
        let to = match input.content_types.get(&oct.name) {
            Some(explicit) => explicit.clone(),
            None => new_types
                .contains_key(oct.name.as_str())
                .then(|| oct.name.clone()),
        };
        if let Some(t) = &to {
            if !new_types.contains_key(t.as_str()) {
                problems.push(format!(
                    "contentTypes.{}: `{t}` is not a content type of the new template",
                    oct.name
                ));
            } else if !targets.insert(t.clone()) {
                problems.push(format!(
                    "contentTypes.{}: `{t}` is already chosen for another content type",
                    oct.name
                ));
            }
        }
        ct_map.insert(oct.name.clone(), to);
    }
    // フィールド
    let mut content_types = Vec::new();
    let mut destructive = false;
    let mut used_fields: BTreeSet<String> = BTreeSet::new();
    for oct in &old_types {
        let to = ct_map.get(&oct.name).cloned().flatten();
        let nct = to.as_deref().and_then(|t| new_types.get(t).copied());
        let mut fields = Vec::new();
        for of in &oct.fields {
            let key = format!("{}.{}", oct.name, of.name);
            let candidates: Vec<String> = nct
                .map(|n| {
                    n.fields
                        .iter()
                        .filter(|nf| compatible(of, nf, &ct_map))
                        .map(|nf| nf.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let target = match (nct, input.fields.get(&key)) {
                (None, _) => None,
                (Some(_), Some(explicit)) => explicit.clone(),
                (Some(_), None) => candidates.iter().find(|c| **c == of.name).cloned(),
            };
            if let (Some(t), Some(n)) = (&target, nct) {
                if !candidates.contains(t) {
                    problems.push(format!(
                        "fields.{key}: `{t}` is not a field of `{}` with the same type",
                        n.name
                    ));
                } else if !used_fields.insert(format!("{}.{t}", n.name)) {
                    problems.push(format!(
                        "fields.{key}: `{}.{t}` is already chosen for another field",
                        n.name
                    ));
                }
            }
            destructive |= target.is_none();
            fields.push(FieldPlan {
                from: of.name.clone(),
                to: target,
                candidates,
            });
        }
        destructive |= to.is_none();
        content_types.push(TypePlan {
            from: oct.name.clone(),
            to,
            fields,
        });
    }
    for key in input.fields.keys() {
        let known = key
            .split_once('.')
            .and_then(|(c, f)| {
                old_types
                    .iter()
                    .find(|o| o.name == c)
                    .map(|o| o.field(f).is_some())
            })
            .unwrap_or(false);
        if !known {
            problems.push(format!("fields.{key}: not a field of the current template"));
        }
    }
    // 引き継がないで増えるもの
    let mut added = Vec::new();
    for nct in &new.content_types {
        if !targets.contains(&nct.name) {
            added.push(nct.name.clone());
            continue;
        }
        for nf in &nct.fields {
            if !used_fields.contains(&format!("{}.{}", nct.name, nf.name)) {
                added.push(format!("{}.{}", nct.name, nf.name));
            }
        }
    }
    Plan {
        content_types,
        new_content_types: new.content_types.iter().map(|c| c.name.clone()).collect(),
        added,
        destructive,
        problems,
    }
}

async fn exec(conn: &mut PgConnection, sql: sqlx::AssertSqlSafe<String>) -> Result<()> {
    sqlx::query(sql).execute(conn).await?;
    Ok(())
}

/// 切り替えで消えるデータ（書き出し用）。記事の本文を含む。ログには出さない。
pub async fn lost_data(conn: &mut PgConnection, db: &WorkspaceDb, plan: &Plan) -> Result<Value> {
    let schema = &db.schema;
    let mut types = Vec::new();
    let mut fields = Vec::new();
    for tp in &plan.content_types {
        let dropped: Vec<&str> = match &tp.to {
            None => tp.fields.iter().map(|f| f.from.as_str()).collect(),
            Some(_) => tp
                .fields
                .iter()
                .filter(|f| f.to.is_none())
                .map(|f| f.from.as_str())
                .collect(),
        };
        if dropped.is_empty() {
            continue;
        }
        let table = table_name(&tp.from)?;
        let rows: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(crate::sql!(
            "SELECT e.id, e.locale, to_jsonb(t)::text FROM {schema}.entries e JOIN {schema}.{table} t ON t.entry_id = e.id
             WHERE e.deleted_at IS NULL ORDER BY e.created_at, e.id"
        ))
        .fetch_all(&mut *conn)
        .await?;
        let mut entries = Vec::new();
        for (id, locale, draft) in rows {
            let versions: Vec<(i32, Option<String>)> = sqlx::query_as(crate::sql!(
                "SELECT number, fields::text FROM {schema}.versions WHERE entry_id = $1 ORDER BY number"
            ))
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
            let pick = |obj: Option<&Value>, prefix: &str| -> Map<String, Value> {
                dropped
                    .iter()
                    .filter_map(|f| {
                        obj.and_then(|o| o.get(format!("{prefix}{f}")))
                            .filter(|v| !v.is_null())
                            .map(|v| ((*f).to_owned(), v.clone()))
                    })
                    .collect()
            };
            let draft: Value = serde_json::from_str(&draft)?;
            let versions: Vec<(i32, Option<Value>)> = versions
                .into_iter()
                .map(|(n, f)| Ok((n, f.map(|f| serde_json::from_str(&f)).transpose()?)))
                .collect::<Result<_>>()?;
            let draft_values = pick(Some(&draft), "f_");
            let version_values: Vec<Value> = versions
                .iter()
                .map(|(n, f)| serde_json::json!({ "number": n, "fields": pick(f.as_ref(), "") }))
                .filter(|v| v["fields"].as_object().is_some_and(|o| !o.is_empty()))
                .collect();
            if draft_values.is_empty() && version_values.is_empty() {
                continue;
            }
            entries.push(serde_json::json!({
                "entryId": id, "locale": locale, "draft": draft_values, "versions": version_values,
            }));
        }
        if tp.to.is_none() {
            types.push(serde_json::json!({ "contentType": tp.from, "entries": entries }));
        } else {
            fields.push(serde_json::json!({ "contentType": tp.from, "fields": dropped, "entries": entries }));
        }
    }
    Ok(serde_json::json!({ "removedContentTypes": types, "removedFields": fields }))
}

/// 名前を変えた古いマニフェスト（対応づけたものだけ。参照先も新しい名前にする）。
fn renamed_old(old: &Manifest, plan: &Plan) -> Manifest {
    let ct_map: BTreeMap<&str, &str> = plan
        .content_types
        .iter()
        .filter_map(|t| t.to.as_deref().map(|to| (t.from.as_str(), to)))
        .collect();
    let mut m = old.clone();
    m.content_types = old
        .content_types
        .iter()
        .filter_map(|oct| {
            let tp = plan.content_types.iter().find(|t| t.from == oct.name)?;
            let to = tp.to.as_ref()?;
            let mut ct = oct.clone();
            ct.name = to.clone();
            ct.fields = oct
                .fields
                .iter()
                .filter_map(|of| {
                    let fp = tp.fields.iter().find(|f| f.from == of.name)?;
                    let mut f = of.clone();
                    f.name = fp.to.clone()?;
                    f.reference_to = of
                        .reference_to
                        .as_deref()
                        .and_then(|r| ct_map.get(r).map(|s| (*s).to_owned()));
                    Some(f)
                })
                .collect();
            Some(ct)
        })
        .collect();
    m
}

/// 切り替えを行う（呼ぶ側のトランザクションの中で。`Err(Failure)` なら巻き戻すこと）。
pub async fn apply(
    conn: &mut PgConnection,
    db: &WorkspaceDb,
    old: Option<&Manifest>,
    new: &Manifest,
    plan: &Plan,
) -> Result<Result<(), migrate::Failure>> {
    let schema = &db.schema;
    let Some(old) = old else {
        return migrate::apply(conn, db, None, new).await;
    };
    // 1. 消すもの
    for tp in &plan.content_types {
        let table = table_name(&tp.from)?;
        match &tp.to {
            None => {
                sqlx::query(crate::sql!(
                    "UPDATE {schema}.entries SET deleted_at = now() WHERE content_type = $1 AND deleted_at IS NULL"
                ))
                .bind(&tp.from)
                .execute(&mut *conn)
                .await?;
                exec(conn, crate::sql!("DROP TABLE {schema}.{table}")).await?;
            }
            Some(_) => {
                for f in tp.fields.iter().filter(|f| f.to.is_none()) {
                    let column = column_name(&f.from)?;
                    exec(
                        conn,
                        crate::sql!("ALTER TABLE {schema}.{table} DROP COLUMN {column}"),
                    )
                    .await?;
                }
            }
        }
    }
    // 2. 名前を変える（仮の名前を通す）
    let mapped: Vec<(&TypePlan, &str)> = plan
        .content_types
        .iter()
        .filter_map(|t| t.to.as_deref().map(|to| (t, to)))
        .collect();
    for (i, (tp, _)) in mapped.iter().enumerate() {
        let table = table_name(&tp.from)?;
        let temp = Ident::new(&format!("ct__switch_{i}"))?;
        exec(
            conn,
            crate::sql!("ALTER TABLE {schema}.{table} RENAME TO {temp}"),
        )
        .await?;
        let renamed: Vec<(&str, &str)> = tp
            .fields
            .iter()
            .filter_map(|f| f.to.as_deref().map(|to| (f.from.as_str(), to)))
            .filter(|(from, to)| from != to)
            .collect();
        for (j, (from, _)) in renamed.iter().enumerate() {
            let column = column_name(from)?;
            let col_temp = Ident::new(&format!("f__switch_{j}"))?;
            exec(
                conn,
                crate::sql!("ALTER TABLE {schema}.{temp} RENAME COLUMN {column} TO {col_temp}"),
            )
            .await?;
        }
        for (j, (_, to)) in renamed.iter().enumerate() {
            let col_temp = Ident::new(&format!("f__switch_{j}"))?;
            let column = column_name(to)?;
            exec(
                conn,
                crate::sql!("ALTER TABLE {schema}.{temp} RENAME COLUMN {col_temp} TO {column}"),
            )
            .await?;
        }
    }
    for (i, (_, to)) in mapped.iter().enumerate() {
        let temp = Ident::new(&format!("ct__switch_{i}"))?;
        let table = table_name(to)?;
        exec(
            conn,
            crate::sql!("ALTER TABLE {schema}.{temp} RENAME TO {table}"),
        )
        .await?;
    }
    // 3. 版の本文（過去の版を含む）と、記事のコンテンツタイプの名前。
    //    版は記事の古いコンテンツタイプの名前で選ぶ（名前を入れ替える対応づけや、消すタイプの名前を新しいタイプが使う場合に、
    //    ほかの記事の版を書き換えないよう、記事の名前を変える前に行う）
    for (tp, _) in &mapped {
        let (olds, news): (Vec<&str>, Vec<&str>) = tp
            .fields
            .iter()
            .filter_map(|f| f.to.as_deref().map(|n| (f.from.as_str(), n)))
            .unzip();
        // 変換の結果は新しい形で作り直す（変換結果のない版は、移行の後に作り直す。Q58）
        sqlx::query(crate::sql!(
            "UPDATE {schema}.versions v SET
                 fields = coalesce((SELECT jsonb_object_agg(m.new, v.fields -> m.old)
                                    FROM unnest($1::text[], $2::text[]) AS m(old, new)
                                    WHERE v.fields ? m.old), '{{}}'::jsonb),
                 rendered = NULL, rendered_with = NULL
             WHERE v.fields IS NOT NULL
               AND v.entry_id IN (SELECT id FROM {schema}.entries WHERE content_type = $3)"
        ))
        .bind(&olds)
        .bind(&news)
        .bind(&tp.from)
        .execute(&mut *conn)
        .await?;
    }
    let (from_names, to_names): (Vec<&str>, Vec<&str>) =
        mapped.iter().map(|(t, to)| (t.from.as_str(), *to)).unzip();
    sqlx::query(crate::sql!(
        "UPDATE {schema}.entries e SET content_type = m.new
         FROM unnest($1::text[], $2::text[]) AS m(old, new)
         WHERE e.content_type = m.old AND e.deleted_at IS NULL"
    ))
    .bind(&from_names)
    .bind(&to_names)
    .execute(&mut *conn)
    .await?;
    // 4. ふつうの移行
    migrate::apply(conn, db, Some(&renamed_old(old, plan)), new).await
}

/// 切り替えの指定。
#[derive(Debug, Clone)]
pub struct Request {
    pub template: String,
    pub mapping: MappingInput,
    pub allow_destructive: bool,
    pub allow_broken_queries: bool,
    pub dry_run: bool,
}

/// 切り替えを行わなかった理由。
#[derive(Debug)]
pub enum Rejected {
    /// 切り替え先のテンプレートがデプロイされていない。
    NotDeployed,
    /// 今と同じテンプレート。
    SameTemplate,
    /// 対応づけの誤り。
    Mapping(Vec<String>),
    /// データが消えるのに、明示的な指定がない（INV-SCH-08）。
    Destructive(Plan),
    /// 登録済みのクエリが壊れるのに、明示的な指定がない。
    BrokenQueries(Vec<super::deploy::BrokenQuery>),
    /// 既存のデータが新しいテンプレートを満たさない（必須のフィールドが空、など）。
    Failed(String),
}

/// 切り替えの結果（試行なら計画だけ）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub dry_run: bool,
    pub from: String,
    pub template: String,
    pub version: i32,
    pub plan: Plan,
    pub broken_queries: Vec<super::deploy::BrokenQuery>,
    /// 公開URLを計算し直すコンテンツタイプ（新しいテンプレートのすべて。応答には含めない）。
    #[serde(skip)]
    pub content_types: Vec<String>,
}

/// 書き出しの結果。
pub struct Export {
    pub plan: Plan,
    pub data: Value,
}

struct Loaded {
    from: String,
    old: Option<Manifest>,
    version: i32,
    new: Manifest,
}

async fn load(
    conn: &mut PgConnection,
    workspace_id: uuid::Uuid,
    template: &str,
    lock_row: bool,
) -> Result<std::result::Result<Loaded, Rejected>> {
    let sql = if lock_row {
        "SELECT template, schema_version FROM yomi.workspaces WHERE id = $1 FOR UPDATE"
    } else {
        "SELECT template, schema_version FROM yomi.workspaces WHERE id = $1"
    };
    let (from, applied): (String, Option<i32>) = sqlx::query_as(sql)
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?;
    if from == template {
        return Ok(Err(Rejected::SameTemplate));
    }
    let Some((version, new)) = super::deploy::latest_version(conn, template).await? else {
        return Ok(Err(Rejected::NotDeployed));
    };
    let old = match applied {
        Some(v) => Some(
            super::deploy::load_version(&mut *conn, &from, v)
                .await?
                .ok_or_else(|| anyhow::anyhow!("applied template version is missing"))?,
        ),
        None => None,
    };
    Ok(Ok(Loaded {
        from,
        old,
        version,
        new,
    }))
}

/// 消えるデータを書き出す（何も変えない）。
pub async fn export(
    migration: &sqlx::PgPool,
    workspace_id: uuid::Uuid,
    template: &str,
    mapping: &MappingInput,
) -> Result<std::result::Result<Export, Rejected>> {
    let mut conn = migration.acquire().await?;
    let loaded = match load(&mut conn, workspace_id, template, false).await? {
        Ok(l) => l,
        Err(r) => return Ok(Err(r)),
    };
    let plan = plan(loaded.old.as_ref(), &loaded.new, mapping);
    if !plan.problems.is_empty() {
        return Ok(Err(Rejected::Mapping(plan.problems)));
    }
    let db = WorkspaceDb::names_for(workspace_id);
    let data = lost_data(&mut conn, &db, &plan).await?;
    Ok(Ok(Export { plan, data }))
}

/// テンプレートを切り替える（デプロイのロックを持って、1つのトランザクションで）。
pub async fn switch(
    migration: &sqlx::PgPool,
    workspace_id: uuid::Uuid,
    req: Request,
    limits: crate::delivery::schema::QueryLimits,
    unused_days: u32,
    actor: uuid::Uuid,
) -> Result<std::result::Result<Outcome, Rejected>> {
    super::deploy::with_deploy_lock(migration, move |conn| {
        Box::pin(
            async move { switch_locked(conn, workspace_id, req, limits, unused_days, actor).await },
        )
    })
    .await
}

async fn switch_locked(
    conn: &mut PgConnection,
    workspace_id: uuid::Uuid,
    req: Request,
    limits: crate::delivery::schema::QueryLimits,
    unused_days: u32,
    actor: uuid::Uuid,
) -> Result<std::result::Result<Outcome, Rejected>> {
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let Loaded {
        from,
        old,
        version,
        new,
    } = match load(&mut tx, workspace_id, &req.template, true).await? {
        Ok(l) => l,
        Err(r) => return Ok(Err(r)),
    };
    let plan = plan(old.as_ref(), &new, &req.mapping);
    if !plan.problems.is_empty() {
        return Ok(Err(Rejected::Mapping(plan.problems)));
    }
    let schema = crate::delivery::schema::build(&new, false, limits)?;
    let broken =
        super::deploy::broken_in_workspace(&mut tx, &schema, workspace_id, unused_days).await?;
    let content_types = new.content_types.iter().map(|c| c.name.clone()).collect();
    if req.dry_run {
        return Ok(Ok(Outcome {
            dry_run: true,
            from,
            template: req.template,
            version,
            plan,
            broken_queries: broken,
            content_types,
        }));
    }
    if plan.destructive && !req.allow_destructive {
        return Ok(Err(Rejected::Destructive(plan)));
    }
    if !broken.is_empty() && !req.allow_broken_queries {
        return Ok(Err(Rejected::BrokenQueries(broken)));
    }
    let db = WorkspaceDb::names_for(workspace_id);
    if let Err(failure) = apply(&mut tx, &db, old.as_ref(), &new, &plan).await? {
        tx.rollback().await?;
        return Ok(Err(Rejected::Failed(failure.code())));
    }
    sqlx::query(
        "UPDATE yomi.workspaces SET template = $2, schema_version = $3, schema_state = 'ok',
                schema_target_version = NULL, schema_error = NULL
         WHERE id = $1",
    )
    .bind(workspace_id)
    .bind(&req.template)
    .bind(version)
    .execute(&mut *tx)
    .await?;
    crate::audit::record(
        &mut *tx,
        Some(workspace_id),
        Some(actor),
        crate::audit::action::TEMPLATE_SWITCHED,
        &[],
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(Outcome {
        dry_run: false,
        from,
        template: req.template,
        version,
        plan,
        broken_queries: broken,
        content_types,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(json: &str) -> Manifest {
        serde_json::from_str(json).unwrap()
    }

    fn base(types: &str) -> String {
        format!(
            r#"{{"locales":["ja"],"defaultLocale":"ja","body":{{"nodes":[{{"name":"doc","children":["paragraph"]}},{{"name":"paragraph","children":["text"]}},{{"name":"text"}}],"marks":[],"embedProviders":[]}},"contentTypes":[{types}]}}"#
        )
    }

    const POST: &str = r#"{"name":"post","urlTemplate":"https://example.com/{slug}","listLimit":5,"fields":[{"name":"title","type":"text","required":true},{"name":"slug","type":"slug","required":true},{"name":"summary","type":"text"}]}"#;
    const ARTICLE: &str = r#"{"name":"article","urlTemplate":"https://example.com/a/{slug}","listLimit":5,"fields":[{"name":"headline","type":"text","required":true},{"name":"slug","type":"slug","required":true},{"name":"body","type":"richtext"}]}"#;

    #[test]
    fn same_names_are_mapped_by_default() {
        let old = manifest(&base(POST));
        let new = manifest(&base(
            &POST.replace(r#",{"name":"summary","type":"text"}"#, ""),
        ));
        let p = plan(Some(&old), &new, &MappingInput::default());
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        assert!(p.destructive);
        let post = &p.content_types[0];
        assert_eq!(post.to.as_deref(), Some("post"));
        assert_eq!(
            post.fields.iter().find(|f| f.from == "summary").unwrap().to,
            None
        );
        assert_eq!(
            post.fields
                .iter()
                .find(|f| f.from == "title")
                .unwrap()
                .to
                .as_deref(),
            Some("title")
        );
        // 増えるだけなら消えない
        let back = plan(Some(&new), &old, &MappingInput::default());
        assert!(!back.destructive);
        assert_eq!(back.added, vec!["post.summary".to_owned()]);
    }

    #[test]
    fn explicit_mappings_rename_and_are_checked() {
        let old = manifest(&base(POST));
        let new = manifest(&base(ARTICLE));
        let input: MappingInput = serde_json::from_str(
            r#"{"contentTypes":{"post":"article"},"fields":{"post.title":"headline","post.summary":null}}"#,
        )
        .unwrap();
        let p = plan(Some(&old), &new, &input);
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        let t = &p.content_types[0];
        assert_eq!(t.to.as_deref(), Some("article"));
        assert_eq!(
            t.fields
                .iter()
                .find(|f| f.from == "title")
                .unwrap()
                .to
                .as_deref(),
            Some("headline")
        );
        assert_eq!(
            t.fields
                .iter()
                .find(|f| f.from == "slug")
                .unwrap()
                .to
                .as_deref(),
            Some("slug")
        );
        assert_eq!(p.added, vec!["article.body".to_owned()]);
        let renamed = renamed_old(&old, &p);
        assert_eq!(renamed.content_types[0].name, "article");
        assert!(renamed.content_types[0].field("headline").is_some());
        assert!(renamed.content_types[0].field("summary").is_none());

        // 型の合わないフィールド、二重の対応づけ、知らない名前は誤り
        for bad in [
            r#"{"contentTypes":{"post":"article"},"fields":{"post.title":"body"}}"#,
            r#"{"contentTypes":{"post":"article"},"fields":{"post.title":"headline","post.summary":"headline"}}"#,
            r#"{"contentTypes":{"post":"nope"}}"#,
            r#"{"fields":{"post.nope":null}}"#,
            r#"{"contentTypes":{"nope":null}}"#,
        ] {
            let input: MappingInput = serde_json::from_str(bad).unwrap();
            assert!(!plan(Some(&old), &new, &input).problems.is_empty(), "{bad}");
        }
    }

    #[test]
    fn nothing_is_carried_over_before_the_first_template() {
        let new = manifest(&base(POST));
        let p = plan(None, &new, &MappingInput::default());
        assert!(p.content_types.is_empty() && !p.destructive && p.problems.is_empty());
        assert_eq!(p.added, vec!["post".to_owned()]);
    }
}
