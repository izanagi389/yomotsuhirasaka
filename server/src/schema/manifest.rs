//! テンプレートのマニフェスト（デプロイのAPIが受け取るJSON）と、その検査（02-schema-and-delivery-api.md、Q49, Q57, Q92, Q102）。
//!
//! CLIでも同じ検査を行うが、サーバーでも必ずやり直す（13-security.md「デプロイ」）。
//! ここを通った名前だけが、DBの識別子（`ct_<名前>`, `f_<名前>`）とGraphQLの名前の元になる。

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// コンテンツタイプ名・フィールド名の最大の長さ（DBの識別子の上限63に、接頭辞を足しても収まる長さ）。
pub const MAX_NAME_LEN: usize = 40;
/// テンプレート名の最大の長さ。
pub const MAX_TEMPLATE_NAME_LEN: usize = 64;
const MAX_CONTENT_TYPES: usize = 100;
const MAX_FIELDS: usize = 200;
const MAX_LOCALES: usize = 50;
const MAX_ALLOWED_HOSTS: usize = 50;
const MAX_CUSTOM_ATTRS: usize = 50;
const MAX_BODY_NODES: usize = 100;
const MAX_TEXT_CHARS: usize = 2000;
/// 一覧のフィールドで1回に取得できる件数の上限の、さらに上限。
pub const MAX_LIST_LIMIT: u32 = 1000;

/// フィールドの型（M1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    /// 1行の文字列。
    Text,
    /// URLに使う文字列（英小文字・数字・ハイフン）。
    Slug,
    /// 本文（Tiptapの構造化JSON）。
    Richtext,
    /// 他の記事への参照（翻訳グループを指す。Q157）。
    Reference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Field {
    /// フィールドの名前（英小文字で始まる英小文字・数字・`_`）。DBとGraphQLの名前の元になる。
    pub name: String,
    #[serde(rename = "type")]
    pub kind: FieldType,
    /// 公開に必須か（スラッグは常に必須）。
    #[serde(default)]
    pub required: bool,
    /// 参照フィールドの参照先のコンテンツタイプ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_to: Option<String>,
    /// 複数の値を持つか（参照フィールドだけ）。
    #[serde(default)]
    pub multiple: bool,
    /// 編集者向けの説明（Q51）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 開発者向けのメモ。どのAPIにも出さない（Q51、INV-SCH-01）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_note: Option<String>,
    /// 検索・質問応答の対象か（M3、08-search-and-related.md「対象の制御」、Q16）。既定は対象外。
    /// 使えるのは text と richtext だけ。対象でないフィールドの内容は、検索用データにも検索結果にも入れない（INV-SRC-03）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub searchable: bool,
}

/// 関数の書き方（型の引数）: マニフェストではJSのソース（[`crate::script::Script`]）、
/// テンプレートのファイルではTSのファイルへのパス（[`TemplateScript`]。CLIがJSにしてマニフェストに入れる。Q49）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[serde(bound(deserialize = "F: Deserialize<'de>", serialize = "F: Serialize"))]
pub struct ContentType<F = crate::script::Script> {
    /// コンテンツタイプの名前（英小文字で始まる英小文字・数字・`_`）。DBとGraphQLの名前の元になる。
    pub name: String,
    /// ロケールごとの公開URLのテンプレート。使える差し込みは `{locale}` と `{slug}`（Q82）。
    pub url_template: String,
    /// URL生成関数（M2、Q38）。あれば、テンプレートの代わりにこれで公開URLを作る。
    /// 受け取るのは公開する記事（`{id, contentType, locale, slug, fields}`）、返すのは http(s) の絶対URL。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_function: Option<F>,
    /// フック（M2、04-hooks.md）。
    #[serde(default, skip_serializing_if = "Hooks::is_empty")]
    pub hooks: Hooks<F>,
    /// 記事のタイトルのフィールド（text）。訂正フィード・目録・管理画面の一覧などで記事の名前として使う。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_field: Option<String>,
    /// 確認期限（06-corrections-and-freshness.md「鮮度」）。なければ期限なし。記事ごとに上書きできる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<Review>,
    /// AI向けのMarkdown版に載せるもの（M2、07-ai-delivery.md、Q35）。挙げていないフィールドは載せない。
    /// なければ、このタイプの記事はAI向けの素材（Markdown版・目録）にならない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_markdown: Option<AiMarkdown>,
    /// 一覧のフィールドで1回に取得できる件数の上限。
    pub list_limit: u32,
    pub fields: Vec<Field>,
    /// 編集者向けの説明（管理画面に表示する。Q51）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 開発者向けのメモ。どのAPIにも出さない（Q51、INV-SCH-01）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_note: Option<String>,
}

/// AI向けのMarkdown版の中身（07-ai-delivery.md）。CMSは、ここに挙げたものに加えて、URL・ロケール・更新日・最終確認日・
/// 読了時間・訂正履歴・削除済みアンカーを冒頭のメタデータに載せる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiMarkdown {
    /// 見出し（`#`）にするフィールド（text）。目録の記事の名前にも使う。
    pub title: String,
    /// 冒頭のメタデータ（front matter）に載せるフィールド（text・slug）。
    #[serde(default)]
    pub meta: Vec<String>,
    /// 本文として、この順に載せるフィールド（text・richtext）。
    #[serde(default)]
    pub fields: Vec<String>,
    /// 関係として載せる参照フィールド。関係の種類はフィールドの名前、相手は記事のURL。
    #[serde(default)]
    pub relations: Vec<String>,
}

/// 確認期限の既定値。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    /// 最終確認日からの日数（1〜3650）。
    pub interval_days: u32,
}

/// コンテンツタイプのフック（04-hooks.md、Q12）。種類ごとに1つ。
///
/// - 同期（結果を待つ。外部と通信できない）:
///   - `beforeSave`: 下書きの保存の前。受け取るのは `{entry: {id, contentType, locale, fields}}`、返すのは直すフィールド
///     `{fields: {...}}`。保存は止めない（失敗したら直さずに保存し、知らせる）。
///   - `beforePublish`: 公開の前。受け取るのは同じ形、返すのは公開を止める理由 `{errors: [{field?, message}]}`。
///     失敗したときも公開を止める。
/// - 非同期（後からジョブで動く。失敗したらやり直す）。受け取るのは `{event, entry}`、返した値は使わない:
///   - `afterSave`: 保存が30秒止まってから、記事ごとに1回（その時点の下書き）。
///   - `afterPublish`: 公開・更新・訂正としての公開の後（公開した版）。
///   - `afterUnpublish`: 非公開・削除の後（entry は null）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[serde(bound(deserialize = "F: Deserialize<'de>", serialize = "F: Serialize"))]
pub struct Hooks<F = crate::script::Script> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_save: Option<F>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_publish: Option<F>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_save: Option<F>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_publish: Option<F>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_unpublish: Option<F>,
}

impl<F> Default for Hooks<F> {
    fn default() -> Self {
        Self {
            before_save: None,
            before_publish: None,
            after_save: None,
            after_publish: None,
            after_unpublish: None,
        }
    }
}

impl<F> Hooks<F> {
    pub fn is_empty(&self) -> bool {
        self.all().is_empty()
    }

    /// (名前, 非同期か, 関数)
    pub fn all(&self) -> Vec<(&'static str, bool, &F)> {
        [
            ("beforeSave", false, &self.before_save),
            ("beforePublish", false, &self.before_publish),
            ("afterSave", true, &self.after_save),
            ("afterPublish", true, &self.after_publish),
            ("afterUnpublish", true, &self.after_unpublish),
        ]
        .into_iter()
        .filter_map(|(name, is_async, s)| s.as_ref().map(|s| (name, is_async, s)))
        .collect()
    }
}

/// 本文のノードの定義（Tiptapの拡張機能から、CLIが取り出したもの。Q57, Q142）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BodyNode {
    pub name: String,
    /// 子として許すノードの名前（content式を簡略化したもの）。
    #[serde(default)]
    pub children: Vec<String>,
    /// 許す属性の名前。
    #[serde(default)]
    pub attrs: Vec<String>,
    /// 子のテキストに書式（マーク）を付けられるか。
    #[serde(default)]
    pub marks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BodyMark {
    pub name: String,
    #[serde(default)]
    pub attrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Body {
    /// 標準のノード（変換器が変換の仕方を知っているもの: [`STANDARD_NODES`]）。それ以外は `customNodes` に書く。
    pub nodes: Vec<BodyNode>,
    #[serde(default)]
    pub marks: Vec<BodyMark>,
    /// 埋め込みを許す提供元のホスト名。
    #[serde(default)]
    pub embed_providers: Vec<String>,
    /// カスタムノード（M2、Q36, Q37）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_nodes: Vec<CustomNode>,
}

/// 変換器が変換の仕方を知っている標準のノードとマーク（`packages/converter`）。
pub const STANDARD_NODES: &[&str] = &[
    "doc",
    "paragraph",
    "text",
    "heading",
    "bulletList",
    "orderedList",
    "listItem",
    "blockquote",
    "codeBlock",
    "horizontalRule",
    "hardBreak",
    "image",
    "embed",
];
pub const STANDARD_MARKS: &[&str] = &["bold", "italic", "strike", "code", "link"];

/// カスタムノードの属性の型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AttrType {
    String,
    Number,
    Boolean,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomAttr {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: AttrType,
    /// 編集者向けの説明（管理画面の入力欄に出す。Q51）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// カスタムノード。テンプレートに宣言的に書き、Tiptapの拡張機能はこの宣言から作る（`@new-cms/editor-schema`）。
/// Markdownへの変換関数は必須（ないものはデプロイを拒否する。INV-NODE-01）。HTMLへの変換関数は任意で、
/// 出力は無害化する（INV-CNV-01）。変換関数は純粋な関数で、ホスト関数は使えない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[serde(bound(deserialize = "F: Deserialize<'de>", serialize = "F: Serialize"))]
pub struct CustomNode<F = crate::script::Script> {
    pub name: String,
    /// 編集者向けの説明（管理画面のカードに出す。Q51）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// インライン（段落などの子）か。既定はブロック。
    #[serde(default)]
    pub inline: bool,
    #[serde(default)]
    pub attrs: Vec<CustomAttr>,
    /// 子として許すノード。
    #[serde(default)]
    pub children: Vec<String>,
    /// `toMarkdown(node, ctx)` を既定のエクスポートにするESモジュール。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_markdown: Option<F>,
    /// `toHtml(node, ctx)` を既定のエクスポートにするESモジュール。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_html: Option<F>,
}

/// テンプレートのファイルでの関数: TSのファイルへのパス（テンプレートのフォルダからの相対パス。フォルダの外は指せない）。
/// CLIが esbuild で1つのESモジュールにまとめ、マニフェストの `source` にする。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateScript {
    /// TSのファイル（例: `./functions/url.ts`）。既定のエクスポートが関数本体。
    pub file: String,
    /// 呼べるホスト関数。
    #[serde(default)]
    pub permissions: Vec<crate::script::Permission>,
    /// `http.fetch` で送ってよいホスト名かIPアドレス。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
}

impl Body {
    /// 本文の検証に使うノードの定義（標準のノードと、カスタムノード）。
    pub fn all_nodes(&self) -> Vec<BodyNode> {
        self.nodes
            .iter()
            .cloned()
            .chain(self.custom_nodes.iter().map(|c| BodyNode {
                name: c.name.clone(),
                children: c.children.clone(),
                attrs: c.attrs.iter().map(|a| a.name.clone()).collect(),
                marks: false,
            }))
            .collect()
    }

    pub fn custom_node(&self, name: &str) -> Option<&CustomNode> {
        self.custom_nodes.iter().find(|c| c.name == name)
    }
}

/// テンプレートのマニフェスト。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// コンテンツのロケール（例: `ja`, `en`）。
    pub locales: Vec<String>,
    /// 既定のロケール（`locales` のどれか）。
    pub default_locale: String,
    pub body: Body,
    pub content_types: Vec<ContentType>,
}

/// テンプレートのファイル（`templates/<テンプレート名>/template.json`）の形。開発者が書くのはこちら。
/// 本文のノードとマークは、CLIが管理画面と同じTiptapの拡張機能から書き出してマニフェストにする（Q57）ので、ここには書かない。
/// それ以外はマニフェストと同じ（テストで確かめる）。エディタの補完と検証に使うJSON Schemaを、この型から作る（Q50）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(
    title = "new-cms template",
    description = "テンプレートのファイル（templates/<テンプレート名>/template.json）。本文のノードとマークは書かない（CLIが、管理画面と同じTiptapの拡張機能から書き出す）。"
)]
pub struct TemplateFile {
    /// このファイルのJSON Schemaの場所（エディタが使う。CMSは読まない）。
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    /// コンテンツのロケール（例: `ja`, `en`）。
    pub locales: Vec<String>,
    /// 既定のロケール（`locales` のどれか）。
    pub default_locale: String,
    #[serde(default)]
    pub body: TemplateBody,
    pub content_types: Vec<ContentType<TemplateScript>>,
}

/// テンプレートのファイルの本文の設定（ノードとマークはCLIが書き出す）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateBody {
    /// 埋め込みを許す提供元のホスト名（例: `www.youtube.com`）。https のURLだけを受け付ける（Q97）。
    #[serde(default)]
    pub embed_providers: Vec<String>,
    /// カスタムノード。
    #[serde(default)]
    pub custom_nodes: Vec<CustomNode<TemplateScript>>,
}

/// テンプレートのファイルのJSON Schema（`cli/schema/template.schema.json` に置き、テストで最新か確かめる）。
pub fn template_json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(TemplateFile)).expect("JSON Schema serializes")
}

impl Manifest {
    pub fn content_type(&self, name: &str) -> Option<&ContentType> {
        self.content_types.iter().find(|c| c.name == name)
    }

    /// スキーマ情報のAPIで所属者に返す写し: `devNote`（INV-SCH-01）と、関数のソースを除く。
    /// 関数（URL生成関数・フック・変換関数）のソースには、送信先の認証情報などが書かれうるため、所属者には見せない。
    pub fn for_members(&self) -> Self {
        let mut m = self.without_dev_notes();
        let clear = |s: &mut Option<crate::script::Script>| {
            if let Some(s) = s {
                s.source.clear();
            }
        };
        for c in &mut m.body.custom_nodes {
            clear(&mut c.to_markdown);
            clear(&mut c.to_html);
        }
        for ct in &mut m.content_types {
            clear(&mut ct.url_function);
            let h = &mut ct.hooks;
            for s in [
                &mut h.before_save,
                &mut h.before_publish,
                &mut h.after_save,
                &mut h.after_publish,
                &mut h.after_unpublish,
            ] {
                clear(s);
            }
        }
        m
    }

    /// `devNote` を除いた写し（スキーマ情報のAPIで返す。INV-SCH-01）。
    pub fn without_dev_notes(&self) -> Self {
        let mut m = self.clone();
        for ct in &mut m.content_types {
            ct.dev_note = None;
            for f in &mut ct.fields {
                f.dev_note = None;
            }
        }
        m
    }
}

impl ContentType {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// コンテンツタイプ名・フィールド名として正しいか: 英小文字で始まり、英小文字・数字・アンダースコアだけ（Q92）。
pub fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && name.len() <= MAX_NAME_LEN
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// テンプレート名として正しいか: 英小文字・数字・ハイフン（ワークスペースの作成と同じ規則）。
pub fn valid_template_name(name: &str) -> bool {
    (1..=MAX_TEMPLATE_NAME_LEN).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// 本文のノード・マークの名前（Tiptapの名前。英字で始まる英数字）。
fn valid_node_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z'))
        && name.len() <= MAX_NAME_LEN
        && bytes.all(|b| b.is_ascii_alphanumeric())
}

/// ロケールのコード（例: `ja`, `en`, `pt-BR`, `zh-Hant`）。
fn valid_locale(code: &str) -> bool {
    let mut parts = code.split('-');
    let Some(lang) = parts.next() else {
        return false;
    };
    (2..=3).contains(&lang.len())
        && lang.bytes().all(|b| b.is_ascii_lowercase())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// ホスト名（埋め込みの提供元）。
fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && host.contains('.')
}

/// URLのテンプレート: `https://` か `http://` で始まり、差し込みは `{locale}` と `{slug}` だけ。`{slug}` は必須。
fn valid_url_template(t: &str) -> bool {
    let Some(rest) = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
    else {
        return false;
    };
    if rest.is_empty()
        || t.len() > MAX_TEXT_CHARS
        || t.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let mut placeholders = Vec::new();
    let mut s = rest;
    while let Some(start) = s.find('{') {
        let Some(end) = s[start..].find('}') else {
            return false;
        };
        placeholders.push(&s[start + 1..start + end]);
        s = &s[start + end + 1..];
    }
    if s.contains('}') {
        return false;
    }
    placeholders.iter().all(|p| *p == "locale" || *p == "slug") && placeholders.contains(&"slug")
}

fn check_text(problems: &mut Vec<String>, path: &str, value: &Option<String>) {
    if let Some(v) = value
        && v.chars().count() > MAX_TEXT_CHARS
    {
        problems.push(format!(
            "{path}: must be at most {MAX_TEXT_CHARS} characters"
        ));
    }
}

/// 関数の形の検査（読み込めるかは、デプロイのときに組み込みJSで確かめる）。
/// 外部への送信（`http.fetch`）は非同期のフックにだけ許し、宛先の宣言を必須にする（INV-HOOK-01、Q12）。
fn script_problems(path: &str, s: &crate::script::Script, is_async: bool) -> Vec<String> {
    use crate::script::Permission;
    let mut problems = Vec::new();
    if s.source.trim().is_empty() || s.source.len() > crate::script::MAX_SOURCE_BYTES {
        problems.push(format!(
            "{path}.source: must be 1 to {} bytes",
            crate::script::MAX_SOURCE_BYTES
        ));
    }
    let unique: BTreeSet<_> = s.permissions.iter().collect();
    if unique.len() != s.permissions.len() {
        problems.push(format!("{path}.permissions: duplicated"));
    }
    let fetches = s.permissions.contains(&Permission::HttpFetch);
    if fetches && !is_async {
        problems.push(format!(
            "{path}.permissions: `http.fetch` is only allowed in asynchronous hooks (afterSave, afterPublish, afterUnpublish)"
        ));
    }
    if fetches && s.allowed_hosts.is_empty() {
        problems.push(format!(
            "{path}.allowedHosts: list the hosts that `http.fetch` may send to"
        ));
    }
    if !fetches && !s.allowed_hosts.is_empty() {
        problems.push(format!(
            "{path}.allowedHosts: only for functions with the `http.fetch` permission"
        ));
    }
    if s.allowed_hosts.len() > MAX_ALLOWED_HOSTS {
        problems.push(format!("{path}.allowedHosts: at most {MAX_ALLOWED_HOSTS}"));
    }
    for (i, h) in s.allowed_hosts.iter().enumerate() {
        if !(valid_host(h) || h.parse::<std::net::IpAddr>().is_ok()) {
            problems.push(format!(
                "{path}.allowedHosts[{i}]: must be a host name or an IP address"
            ));
        }
    }
    problems
}

/// 変換関数の検査: 形の検査に加え、ホスト関数を使わない純粋な関数であること（Q36）。
fn conversion_problems(path: &str, s: &crate::script::Script) -> Vec<String> {
    let mut problems = script_problems(path, s, false);
    if !s.permissions.is_empty() || !s.allowed_hosts.is_empty() {
        problems.push(format!(
            "{path}.permissions: conversion functions are pure and cannot use host functions"
        ));
    }
    problems
}

/// マニフェストに含まれる関数（場所, 関数）。
pub fn scripts(m: &Manifest) -> Vec<(String, &crate::script::Script)> {
    let mut out = Vec::new();
    for (i, c) in m.body.custom_nodes.iter().enumerate() {
        if let Some(f) = &c.to_markdown {
            out.push((format!("body.customNodes[{i}].toMarkdown"), f));
        }
        if let Some(f) = &c.to_html {
            out.push((format!("body.customNodes[{i}].toHtml"), f));
        }
    }
    for (i, ct) in m.content_types.iter().enumerate() {
        if let Some(f) = &ct.url_function {
            out.push((format!("contentTypes[{i}].urlFunction"), f));
        }
        for (name, _, f) in ct.hooks.all() {
            out.push((format!("contentTypes[{i}].hooks.{name}"), f));
        }
    }
    out
}

/// マニフェストを検査し、問題の一覧を返す（空なら正しい）。メッセージには場所と理由だけを書く。
pub fn validate(m: &Manifest) -> Vec<String> {
    let mut problems = Vec::new();

    // ロケール
    if m.locales.is_empty() || m.locales.len() > MAX_LOCALES {
        problems.push(format!("locales: must have 1 to {MAX_LOCALES} entries"));
    }
    let mut seen = BTreeSet::new();
    for (i, l) in m.locales.iter().enumerate() {
        if !valid_locale(l) {
            problems.push(format!("locales[{i}]: invalid locale code"));
        }
        if !seen.insert(l) {
            problems.push(format!("locales[{i}]: duplicated"));
        }
    }
    if !m.locales.contains(&m.default_locale) {
        problems.push("defaultLocale: must be one of locales".to_owned());
    }

    // 本文のノードとマーク
    let body = &m.body;
    if body.nodes.len() > MAX_BODY_NODES || body.marks.len() > MAX_BODY_NODES {
        problems.push(format!("body: at most {MAX_BODY_NODES} nodes and marks"));
    }
    let mut node_names = BTreeSet::new();
    for (i, n) in body.nodes.iter().enumerate() {
        if !valid_node_name(&n.name) {
            problems.push(format!("body.nodes[{i}].name: invalid node name"));
        }
        if !node_names.insert(n.name.as_str()) {
            problems.push(format!("body.nodes[{i}].name: duplicated"));
        }
        // 変換の仕方を知らないノードは、変換関数を付けたカスタムノードとして書かせる（INV-NODE-01）
        if !STANDARD_NODES.contains(&n.name.as_str()) {
            problems.push(format!(
                "body.nodes[{i}].name: `{}` is not a standard node; declare it in body.customNodes with a toMarkdown function",
                n.name
            ));
        }
        for (j, a) in n.attrs.iter().enumerate() {
            if !valid_node_name(a) {
                problems.push(format!(
                    "body.nodes[{i}].attrs[{j}]: invalid attribute name"
                ));
            }
        }
    }
    for (i, c) in body.custom_nodes.iter().enumerate() {
        let path = format!("body.customNodes[{i}]");
        if !valid_node_name(&c.name) {
            problems.push(format!("{path}.name: invalid node name"));
        }
        if STANDARD_NODES.contains(&c.name.as_str()) || STANDARD_MARKS.contains(&c.name.as_str()) {
            problems.push(format!("{path}.name: `{}` is a standard name", c.name));
        }
        if !node_names.insert(c.name.as_str()) {
            problems.push(format!("{path}.name: duplicated"));
        }
        if c.attrs.len() > MAX_CUSTOM_ATTRS {
            problems.push(format!("{path}.attrs: at most {MAX_CUSTOM_ATTRS}"));
        }
        check_text(
            &mut problems,
            &format!("{path}.description"),
            &c.description,
        );
        let mut seen = BTreeSet::new();
        for (j, a) in c.attrs.iter().enumerate() {
            check_text(
                &mut problems,
                &format!("{path}.attrs[{j}].description"),
                &a.description,
            );
            if !valid_node_name(&a.name) {
                problems.push(format!("{path}.attrs[{j}].name: invalid attribute name"));
            }
            if !seen.insert(a.name.as_str()) {
                problems.push(format!("{path}.attrs[{j}].name: duplicated"));
            }
        }
        match &c.to_markdown {
            None => problems.push(format!(
                "{path}.toMarkdown: a Markdown conversion function is required for every custom node"
            )),
            Some(s) => problems.extend(conversion_problems(&format!("{path}.toMarkdown"), s)),
        }
        if let Some(s) = &c.to_html {
            problems.extend(conversion_problems(&format!("{path}.toHtml"), s));
        }
    }
    for (i, mk) in body.marks.iter().enumerate() {
        if !STANDARD_MARKS.contains(&mk.name.as_str()) {
            problems.push(format!(
                "body.marks[{i}].name: `{}` is not a standard mark",
                mk.name
            ));
        }
    }
    for required in ["doc", "text"] {
        if !node_names.contains(required) {
            problems.push(format!("body.nodes: `{required}` is required"));
        }
    }
    // 配信API（GraphQL）の名前がぶつからないこと
    problems.extend(crate::delivery::names::problems(m));
    // CMSが意味を知っているノードは、検証と公開の検査に使う属性を持たなければならない（Q75, Q97）
    for (i, n) in body.nodes.iter().enumerate() {
        let needed: &[&str] = match n.name.as_str() {
            "image" => &["mediaId", "alt", "decorative"],
            "embed" => &["src"],
            _ => &[],
        };
        for a in needed {
            if !n.attrs.iter().any(|x| x == a) {
                problems.push(format!(
                    "body.nodes[{i}].attrs: `{}` needs the attribute `{a}`",
                    n.name
                ));
            }
        }
    }
    for (i, n) in body.nodes.iter().enumerate() {
        for (j, c) in n.children.iter().enumerate() {
            if !node_names.contains(c.as_str()) {
                problems.push(format!("body.nodes[{i}].children[{j}]: unknown node"));
            }
        }
    }
    for (i, n) in body.custom_nodes.iter().enumerate() {
        for (j, c) in n.children.iter().enumerate() {
            if !node_names.contains(c.as_str()) {
                problems.push(format!("body.customNodes[{i}].children[{j}]: unknown node"));
            }
        }
    }
    let mut mark_names = BTreeSet::new();
    for (i, mk) in body.marks.iter().enumerate() {
        if !valid_node_name(&mk.name) {
            problems.push(format!("body.marks[{i}].name: invalid mark name"));
        }
        if !mark_names.insert(mk.name.as_str()) {
            problems.push(format!("body.marks[{i}].name: duplicated"));
        }
        for (j, a) in mk.attrs.iter().enumerate() {
            if !valid_node_name(a) {
                problems.push(format!(
                    "body.marks[{i}].attrs[{j}]: invalid attribute name"
                ));
            }
        }
    }
    let mut providers = BTreeSet::new();
    for (i, h) in body.embed_providers.iter().enumerate() {
        if !valid_host(h) {
            problems.push(format!("body.embedProviders[{i}]: invalid host name"));
        }
        if !providers.insert(h) {
            problems.push(format!("body.embedProviders[{i}]: duplicated"));
        }
    }

    // コンテンツタイプとフィールド
    if m.content_types.len() > MAX_CONTENT_TYPES {
        problems.push(format!("contentTypes: at most {MAX_CONTENT_TYPES}"));
    }
    let type_names: BTreeSet<&str> = m.content_types.iter().map(|c| c.name.as_str()).collect();
    let mut seen_types = BTreeSet::new();
    for (i, ct) in m.content_types.iter().enumerate() {
        let path = format!("contentTypes[{i}]");
        if !valid_name(&ct.name) {
            problems.push(format!(
                "{path}.name: use 1 to {MAX_NAME_LEN} characters of a-z, 0-9 and _, starting with a letter"
            ));
        }
        if !seen_types.insert(ct.name.as_str()) {
            problems.push(format!("{path}.name: duplicated"));
        }
        if !valid_url_template(&ct.url_template) {
            problems.push(format!(
                "{path}.urlTemplate: must be an http(s) URL using only {{locale}} and {{slug}}, and include {{slug}}"
            ));
        }
        if let Some(f) = &ct.url_function {
            problems.extend(script_problems(&format!("{path}.urlFunction"), f, false));
        }
        for (name, is_async, f) in ct.hooks.all() {
            problems.extend(script_problems(
                &format!("{path}.hooks.{name}"),
                f,
                is_async,
            ));
        }
        if let Some(r) = &ct.review
            && !(1..=3650).contains(&r.interval_days)
        {
            problems.push(format!("{path}.review.intervalDays: must be 1 to 3650"));
        }
        if let Some(tf) = &ct.title_field
            && !ct
                .fields
                .iter()
                .any(|f| &f.name == tf && f.kind == FieldType::Text)
        {
            problems.push(format!("{path}.titleField: must name a text field"));
        }
        if let Some(ai) = &ct.ai_markdown {
            problems.extend(ai_markdown_problems(&path, ct, ai));
        }
        if !(1..=MAX_LIST_LIMIT).contains(&ct.list_limit) {
            problems.push(format!("{path}.listLimit: must be 1 to {MAX_LIST_LIMIT}"));
        }
        check_text(
            &mut problems,
            &format!("{path}.description"),
            &ct.description,
        );
        check_text(&mut problems, &format!("{path}.devNote"), &ct.dev_note);
        if ct.fields.len() > MAX_FIELDS {
            problems.push(format!("{path}.fields: at most {MAX_FIELDS}"));
        }
        let mut seen_fields = BTreeSet::new();
        for (j, f) in ct.fields.iter().enumerate() {
            let fpath = format!("{path}.fields[{j}]");
            if !valid_name(&f.name) {
                problems.push(format!(
                    "{fpath}.name: use 1 to {MAX_NAME_LEN} characters of a-z, 0-9 and _, starting with a letter"
                ));
            }
            if !seen_fields.insert(f.name.as_str()) {
                problems.push(format!("{fpath}.name: duplicated"));
            }
            match (f.kind, &f.reference_to) {
                (FieldType::Reference, Some(to)) if type_names.contains(to.as_str()) => {}
                (FieldType::Reference, _) => {
                    problems.push(format!("{fpath}.referenceTo: must name a content type"))
                }
                (_, Some(_)) => problems.push(format!(
                    "{fpath}.referenceTo: only reference fields can have it"
                )),
                (_, None) => {}
            }
            if f.multiple && f.kind != FieldType::Reference {
                problems.push(format!(
                    "{fpath}.multiple: only reference fields can be multiple"
                ));
            }
            if f.searchable && !matches!(f.kind, FieldType::Text | FieldType::Richtext) {
                problems.push(format!(
                    "{fpath}.searchable: only text and richtext fields can be searchable"
                ));
            }
            check_text(
                &mut problems,
                &format!("{fpath}.description"),
                &f.description,
            );
            check_text(&mut problems, &format!("{fpath}.devNote"), &f.dev_note);
        }
        // URLを作るためのスラッグ
        let slugs = ct
            .fields
            .iter()
            .filter(|f| f.kind == FieldType::Slug)
            .count();
        if slugs != 1 {
            problems.push(format!("{path}.fields: exactly one slug field is required"));
        }
    }
    problems
}

/// `aiMarkdown` の検査: 挙げたフィールドがあり、使い道に合う型で、重ならないこと。
fn ai_markdown_problems<F>(path: &str, ct: &ContentType<F>, ai: &AiMarkdown) -> Vec<String> {
    let mut problems = Vec::new();
    let kind_of = |name: &str| ct.fields.iter().find(|f| f.name == name).map(|f| f.kind);
    if kind_of(&ai.title) != Some(FieldType::Text) {
        problems.push(format!("{path}.aiMarkdown.title: must name a text field"));
    }
    let lists: [(&str, &Vec<String>, &[FieldType]); 3] = [
        ("meta", &ai.meta, &[FieldType::Text, FieldType::Slug]),
        (
            "fields",
            &ai.fields,
            &[FieldType::Text, FieldType::Richtext],
        ),
        ("relations", &ai.relations, &[FieldType::Reference]),
    ];
    for (key, names, kinds) in lists {
        let mut seen = BTreeSet::new();
        for (i, name) in names.iter().enumerate() {
            if !kind_of(name).is_some_and(|k| kinds.contains(&k)) {
                let allowed: Vec<String> = kinds
                    .iter()
                    .map(|k| format!("{k:?}").to_lowercase())
                    .collect();
                problems.push(format!(
                    "{path}.aiMarkdown.{key}[{i}]: must name a field of type {}",
                    allowed.join(" or ")
                ));
            }
            if !seen.insert(name.as_str()) {
                problems.push(format!("{path}.aiMarkdown.{key}[{i}]: duplicated"));
            }
        }
    }
    problems
}

/// 名前 → 定義の対応（差分を取るため）。
pub fn types_by_name(m: &Manifest) -> BTreeMap<&str, &ContentType> {
    m.content_types
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample() -> Manifest {
        serde_json::from_str(
            r#"{
              "locales": ["ja", "en"],
              "defaultLocale": "ja",
              "body": {
                "nodes": [
                  {"name": "doc", "children": ["paragraph"]},
                  {"name": "paragraph", "children": ["text"], "marks": true},
                  {"name": "text"}
                ],
                "marks": [{"name": "bold"}, {"name": "link", "attrs": ["href"]}],
                "embedProviders": ["www.youtube.com"]
              },
              "contentTypes": [{
                "name": "post",
                "urlTemplate": "https://example.com/{locale}/blog/{slug}",
                "listLimit": 20,
                "devNote": "secret note",
                "fields": [
                  {"name": "title", "type": "text", "required": true},
                  {"name": "slug", "type": "slug", "required": true},
                  {"name": "body", "type": "richtext"},
                  {"name": "related", "type": "reference", "referenceTo": "post", "multiple": true, "devNote": "x"}
                ]
              }]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn sample_is_valid() {
        assert_eq!(validate(&sample()), Vec::<String>::new());
    }

    #[test]
    fn known_nodes_need_their_attributes() {
        let mut m = sample();
        m.body.nodes[0].children.push("image".into());
        m.body
            .nodes
            .push(serde_json::from_str(r#"{"name": "image", "attrs": ["mediaId"]}"#).unwrap());
        let problems = validate(&m);
        assert!(problems.iter().any(|p| p.contains("`alt`")), "{problems:?}");
        assert!(
            problems.iter().any(|p| p.contains("`decorative`")),
            "{problems:?}"
        );
        m.body.nodes.last_mut().unwrap().attrs =
            vec!["mediaId".into(), "alt".into(), "decorative".into()];
        assert_eq!(validate(&m), Vec::<String>::new());
    }

    #[test]
    fn names() {
        for ok in ["post", "valid_name_2", "a", &"a".repeat(MAX_NAME_LEN)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "Post",
            "post-name",
            "post name",
            "post\";drop table x;--",
            "ポスト",
            "post$",
            "post.x",
            "2post",
            "_post",
            &"a".repeat(MAX_NAME_LEN + 1),
            "addedField",
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(valid_template_name("inv-sch02-1a2b3c4d"));
        assert!(!valid_template_name("Inv"));
        assert!(
            valid_locale("pt-BR")
                && valid_locale("zh-Hant")
                && !valid_locale("JA")
                && !valid_locale("j")
        );
        assert!(valid_host("www.youtube.com") && !valid_host("localhost") && !valid_host("a..b"));
    }

    #[test]
    fn url_templates() {
        assert!(valid_url_template("https://example.com/{locale}/{slug}"));
        assert!(valid_url_template("http://localhost:3000/{slug}"));
        for bad in [
            "https://example.com/{locale}",
            "https://example.com/{id}/{slug}",
            "javascript:alert(1)//{slug}",
            "/blog/{slug}",
            "https://example.com/{slug",
            "https://example.com/slug}",
            "https://exa mple.com/{slug}",
        ] {
            assert!(!valid_url_template(bad), "{bad}");
        }
    }

    #[test]
    fn only_text_and_richtext_fields_can_be_searchable() {
        let mut m = sample();
        for f in &mut m.content_types[0].fields {
            f.searchable = true;
        }
        let problems = validate(&m);
        let searchable: Vec<&String> = problems
            .iter()
            .filter(|p| p.contains(".searchable"))
            .collect();
        // 例のコンテンツタイプのうち、text と richtext 以外（スラッグなど）だけが問題になる
        let others = m.content_types[0]
            .fields
            .iter()
            .filter(|f| !matches!(f.kind, FieldType::Text | FieldType::Richtext))
            .count();
        assert!(others > 0);
        assert_eq!(searchable.len(), others, "{problems:?}");
        m.content_types[0]
            .fields
            .iter_mut()
            .for_each(|f| f.searchable = matches!(f.kind, FieldType::Text | FieldType::Richtext));
        assert!(!validate(&m).iter().any(|p| p.contains(".searchable")));
    }

    #[test]
    fn invalid_manifests_are_reported() {
        let mut m = sample();
        m.content_types[0].name = "Post".into();
        m.content_types[0].fields.push(Field {
            name: "post-name".into(),
            kind: FieldType::Text,
            required: false,
            reference_to: Some("post".into()),
            multiple: true,
            description: None,
            dev_note: None,
            searchable: false,
        });
        m.default_locale = "fr".into();
        m.body.nodes.retain(|n| n.name != "text");
        let problems = validate(&m);
        for needle in [
            "contentTypes[0].name",
            "fields[4].name",
            "fields[4].referenceTo",
            "fields[4].multiple",
            "defaultLocale",
            "`text` is required",
            "children[0]: unknown node",
            // 参照先も Post に変わったため
            "fields[3].referenceTo",
        ] {
            assert!(
                problems.iter().any(|p| p.contains(needle)),
                "{needle}: {problems:?}"
            );
        }
    }

    #[test]
    fn fetch_is_only_for_asynchronous_hooks() {
        let script = |permissions: &str, hosts: &str| -> crate::script::Script {
            serde_json::from_str(&format!(
                r#"{{"source": "export default function () {{}}", "permissions": [{permissions}], "allowedHosts": [{hosts}]}}"#
            ))
            .unwrap()
        };
        let fetch = script(r#""http.fetch""#, r#""api.example.com", "127.0.0.1""#);
        let mut m = sample();
        m.content_types[0].hooks.after_publish = Some(fetch.clone());
        assert_eq!(validate(&m), Vec::<String>::new());
        for set in [
            |h: &mut Hooks, s| h.before_save = Some(s),
            |h: &mut Hooks, s| h.before_publish = Some(s),
        ] {
            let mut m = sample();
            set(&mut m.content_types[0].hooks, fetch.clone());
            assert!(
                validate(&m)
                    .iter()
                    .any(|p| p.contains("only allowed in asynchronous"))
            );
        }
        let mut m = sample();
        m.content_types[0].url_function = Some(fetch);
        assert!(
            validate(&m)
                .iter()
                .any(|p| p.contains("only allowed in asynchronous"))
        );
        for (perm, hosts, needle) in [
            (r#""http.fetch""#, "", "list the hosts"),
            ("", r#""api.example.com""#, "only for functions"),
            (r#""http.fetch""#, r#""not a host""#, "host name or an IP"),
        ] {
            let mut m = sample();
            m.content_types[0].hooks.after_save = Some(script(perm, hosts));
            assert!(validate(&m).iter().any(|p| p.contains(needle)), "{needle}");
        }
    }

    #[test]
    fn function_sources_are_not_shown_to_members() {
        let mut m = sample();
        let s: crate::script::Script =
            serde_json::from_str(r#"{"source": "export default () => 'SECRET-TOKEN'"}"#).unwrap();
        m.content_types[0].url_function = Some(s.clone());
        m.content_types[0].hooks.after_publish = Some(s.clone());
        m.body.custom_nodes.push(CustomNode {
            name: "callout".into(),
            description: None,
            inline: false,
            attrs: vec![],
            children: vec![],
            to_markdown: Some(s.clone()),
            to_html: Some(s),
        });
        let out = serde_json::to_string(&m.for_members()).unwrap();
        assert!(!out.contains("SECRET-TOKEN"), "{out}");
        assert!(out.contains("callout"));
    }

    #[test]
    fn dev_notes_are_removed() {
        let m = sample().without_dev_notes();
        assert!(!serde_json::to_string(&m).unwrap().contains("secret note"));
        assert!(!serde_json::to_string(&m).unwrap().contains("devNote"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let mut v = serde_json::to_value(sample()).unwrap();
        v["contentTypes"][0]["fields"][0]["sql"] = "x".into();
        assert!(serde_json::from_value::<Manifest>(v).is_err());
    }

    /// `cli/schema/template.schema.json` が、この型から作ったものと同じか。
    /// 変えたときは `CMS_UPDATE_TEMPLATE_SCHEMA=1 cargo test -p cms-server template_json_schema` で書き直す。
    #[test]
    fn template_json_schema_is_up_to_date() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../cli/schema/template.schema.json");
        let generated = format!(
            "{}\n",
            serde_json::to_string_pretty(&template_json_schema()).unwrap()
        );
        if std::env::var_os("CMS_UPDATE_TEMPLATE_SCHEMA").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == generated,
            "{} is out of date. Run: CMS_UPDATE_TEMPLATE_SCHEMA=1 cargo test -p cms-server template_json_schema",
            path.display()
        );
    }

    /// テンプレートのファイルは、本文のノードとマークを除けば、マニフェストと同じ項目を持つ。
    #[test]
    fn template_file_matches_the_manifest() {
        let props = |v: serde_json::Value| -> BTreeSet<String> {
            v["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        };
        let manifest = serde_json::to_value(schemars::schema_for!(Manifest)).unwrap();
        let template = template_json_schema();
        let mut expected = props(manifest.clone());
        expected.insert("$schema".to_owned());
        assert_eq!(props(template.clone()), expected);
        let body = |root: &serde_json::Value, name: &str| -> BTreeSet<String> {
            root["$defs"][name]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        };
        let mut manifest_body = body(&manifest, "Body");
        manifest_body.remove("nodes");
        manifest_body.remove("marks");
        assert_eq!(body(&template, "TemplateBody"), manifest_body);
        // コンテンツタイプ・フック・カスタムノードは同じ項目を持つ（関数の書き方だけが型の引数で違う）
        let def_props = |root: &serde_json::Value, prefix: &str| -> BTreeSet<String> {
            let defs = root["$defs"].as_object().unwrap();
            let (_, def) = defs
                .iter()
                .find(|(k, _)| k.starts_with(prefix))
                .unwrap_or_else(|| panic!("no $defs starting with {prefix}"));
            def["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        };
        for name in ["ContentType", "Hooks", "CustomNode"] {
            assert_eq!(
                def_props(&template, name),
                def_props(&manifest, name),
                "{name}"
            );
        }
        assert_eq!(template["$defs"]["Field"], manifest["$defs"]["Field"]);
        // 関数は、ソース（source）の代わりにTSのファイル（file）を持つ
        let mut script = def_props(&manifest, "Script");
        script.remove("source");
        script.insert("file".to_owned());
        assert_eq!(def_props(&template, "TemplateScript"), script);
    }
}
