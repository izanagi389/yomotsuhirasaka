//! リダイレクト一覧（05-urls-and-redirects.md「変更履歴とリダイレクト」、Q40）。
//!
//! URLの変更履歴から、配信APIで返す一覧を作る。返すたびに、その時点の記事の状態から作り直す（原則3）。
//! - 旧URLは、その記事の**いまの**URLへ直接向ける。A→B→C と変わっても A→C・B→C にする（連鎖を作らない。INV-RED-01）。
//! - 削除した記事のURLは、過去のものも含めて「消えた（410）」にする（INV-RED-02）。
//! - 非公開にした（削除していない）記事のURLは含めない（公開し直せば戻るため。サイトでは404になる）。
//! - いまどこかの公開中の記事が使っているURLは、転送しない（元のURLに戻った場合を含む）。
//! - 同じURLを複数の記事が使ったことがあるときは、そのURLの記録がいちばん新しい記事に従う。

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use sqlx::PgConnection;
use uuid::Uuid;

/// 一覧の1件。`to` は410のとき None。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub from: String,
    pub to: Option<String>,
    pub status: u16,
}

/// 一覧を作る。ワークスペースのロールのトランザクションの中で呼ぶ。
pub async fn list(conn: &mut PgConnection) -> Result<Vec<Rule>> {
    // 記事の状態: (いまのURL, 公開中か, 削除済みか)
    let entries: Vec<(Uuid, Option<String>, bool, bool)> = sqlx::query_as(
        "SELECT id, url, published_version_id IS NOT NULL AND deleted_at IS NULL, deleted_at IS NOT NULL FROM entries",
    )
    .fetch_all(&mut *conn)
    .await?;
    // URLの変更履歴（古い順）
    let changes: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT entry_id, from_url, to_url FROM url_changes ORDER BY changed_at, id",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(build(&entries, &changes))
}

/// 記事の状態と変更履歴（古い順）から一覧を作る。
fn build(
    entries: &[(Uuid, Option<String>, bool, bool)],
    changes: &[(Uuid, String, String)],
) -> Vec<Rule> {
    let live: HashSet<&str> = entries
        .iter()
        .filter(|(_, _, published, _)| *published)
        .filter_map(|(_, url, _, _)| url.as_deref())
        .collect();
    // 記事ごとの行き先: Some(Some(url)) 転送、Some(None) 410、None 含めない（非公開）
    let target: HashMap<Uuid, Option<Option<&str>>> = entries
        .iter()
        .map(|(id, url, published, deleted)| {
            let t = if *deleted {
                Some(None)
            } else if *published {
                url.as_deref().map(Some)
            } else {
                None
            };
            (*id, t)
        })
        .collect();
    // URL → そのURLの記録がいちばん新しい記事（変更履歴の順に上書きする。削除した記事の最後のURLも含める）
    let mut owner: BTreeMap<&str, Uuid> = BTreeMap::new();
    for (entry, from, to) in changes {
        owner.insert(from, *entry);
        owner.insert(to, *entry);
    }
    for (id, url, _, deleted) in entries {
        if let (true, Some(url)) = (*deleted, url.as_deref()) {
            owner.entry(url).or_insert(*id);
        }
    }
    owner
        .into_iter()
        .filter(|(url, _)| !live.contains(url))
        .filter_map(|(url, entry)| match target.get(&entry).copied().flatten() {
            Some(Some(to)) if to != url => Some(Rule {
                from: url.to_owned(),
                to: Some(to.to_owned()),
                status: 301,
            }),
            Some(None) => Some(Rule {
                from: url.to_owned(),
                to: None,
                status: 410,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }
    fn s(v: &str) -> String {
        v.to_owned()
    }
    fn rule(from: &str, to: Option<&str>) -> Rule {
        Rule {
            from: s(from),
            to: to.map(s),
            status: if to.is_some() { 301 } else { 410 },
        }
    }

    #[test]
    fn chains_point_to_the_current_url() {
        let entries = [(id(1), Some(s("c")), true, false)];
        let changes = [(id(1), s("a"), s("b")), (id(1), s("b"), s("c"))];
        assert_eq!(
            build(&entries, &changes),
            vec![rule("a", Some("c")), rule("b", Some("c"))]
        );
    }

    #[test]
    fn live_urls_are_never_redirected() {
        // x → y → x と戻った記事と、別の記事が y を使っている場合
        let entries = [(id(1), Some(s("x")), true, false)];
        let changes = [(id(1), s("x"), s("y")), (id(1), s("y"), s("x"))];
        assert_eq!(build(&entries, &changes), vec![rule("y", Some("x"))]);
        let entries = [
            (id(1), Some(s("x")), true, false),
            (id(2), Some(s("y")), true, false),
        ];
        assert_eq!(build(&entries, &changes), vec![]);
    }

    #[test]
    fn deleted_entries_are_gone_and_unpublished_ones_are_left_out() {
        let entries = [
            (id(1), Some(s("b")), false, true),
            (id(2), Some(s("q")), false, false),
        ];
        let changes = [(id(1), s("a"), s("b")), (id(2), s("p"), s("q"))];
        assert_eq!(
            build(&entries, &changes),
            vec![rule("a", None), rule("b", None)]
        );
    }

    #[test]
    fn the_latest_owner_of_a_url_wins() {
        // 記事1が u を手放した後、記事2が u から v へ変えた
        let entries = [
            (id(1), Some(s("w")), true, false),
            (id(2), Some(s("v")), true, false),
        ];
        let changes = [(id(1), s("u"), s("w")), (id(2), s("u"), s("v"))];
        assert_eq!(build(&entries, &changes), vec![rule("u", Some("v"))]);
    }
}
