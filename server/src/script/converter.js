// 本文（TiptapのJSON）の変換器（03-content-and-editing.md「本文」、Q56）。
//
// - Markdown と HTML を作る。標準のノードも「最初から登録されている変換関数」として扱う。
// - サーバーでは組み込みJS（QuickJS）で、管理画面では Nuxt で、同じこのファイルを動かす。そのため、ほかのモジュールを
//   import せず、ブラウザや Node の機能（DOM など）にも頼らない。
// - HTMLでは、本文中の文字列と属性の値をすべてエスケープする。カスタムノードの toHtml が返したHTMLはそのまま入れるので、
//   出力は必ず無害化してから使う（サーバーは ammonia で無害化する。INV-CNV-01、13-security.md「本文」）。
// - 画像は `cms-media:<メディアのID>` を参照先にして出す。配信のときに、その時点のメディアのURLに置き換える
//   （削除したメディアを出さないため。原則3）。
/** 変換器の版。出力が変わる変更をしたら上げる（全記事の作り直しの判定に使う。Q58）。 */
export const VERSION = "1";
/** 画像の参照先の接頭辞。 */
export const MEDIA_SCHEME = "cms-media:";
const str = (v) => (typeof v === "string" ? v : v === undefined || v === null ? "" : String(v));
// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------
/** HTMLの本文と属性の値のエスケープ。 */
export function escapeHtml(s) {
    return s.replace(/[&<>"'`]/g, (c) => `&#${c.charCodeAt(0)};`);
}
const INLINE_HTML_MARKS = {
    bold: ["<strong>", "</strong>"],
    italic: ["<em>", "</em>"],
    strike: ["<s>", "</s>"],
    code: ["<code>", "</code>"],
};
function textHtml(n) {
    let out = escapeHtml(str(n.text));
    for (const m of n.marks ?? []) {
        if (m.type === "link") {
            out = `<a href="${escapeHtml(str(m.attrs?.["href"]))}">${out}</a>`;
        }
        else {
            const tags = INLINE_HTML_MARKS[m.type];
            if (tags)
                out = tags[0] + out + tags[1];
        }
    }
    return out;
}
function html(n, custom) {
    const inner = () => (n.content ?? []).map((c) => html(c, custom)).join("");
    switch (n.type) {
        case "doc":
            return (n.content ?? []).map((c) => html(c, custom)).join("\n");
        case "text":
            return textHtml(n);
        case "paragraph":
            return `<p>${inner()}</p>`;
        case "heading": {
            const level = Math.min(6, Math.max(1, Number(n.attrs?.["level"]) || 2));
            const id = str(n.attrs?.["id"]);
            return `<h${level}${id ? ` id="${escapeHtml(id)}"` : ""}>${inner()}</h${level}>`;
        }
        case "bulletList":
            return `<ul>${inner()}</ul>`;
        case "orderedList": {
            const start = Number(n.attrs?.["start"]);
            return `<ol${Number.isInteger(start) && start !== 1 ? ` start="${start}"` : ""}>${inner()}</ol>`;
        }
        case "listItem":
            return `<li>${inner()}</li>`;
        case "blockquote":
            return `<blockquote>${inner()}</blockquote>`;
        case "codeBlock": {
            const lang = str(n.attrs?.["language"]).replace(/[^A-Za-z0-9_+-]/g, "");
            const code = (n.content ?? []).map((c) => escapeHtml(str(c.text))).join("");
            return `<pre><code${lang ? ` class="language-${lang}"` : ""}>${code}</code></pre>`;
        }
        case "horizontalRule":
            return "<hr>";
        case "hardBreak":
            return "<br>";
        case "image": {
            const alt = n.attrs?.["decorative"] === true ? "" : str(n.attrs?.["alt"]);
            return `<img src="${escapeHtml(MEDIA_SCHEME + str(n.attrs?.["mediaId"]))}" alt="${escapeHtml(alt)}">`;
        }
        case "embed": {
            // 埋め込みの見せ方はサイト側に任せ、HTMLではリンクにする（iframe は出さない）
            const src = escapeHtml(str(n.attrs?.["src"]));
            return `<p><a href="${src}">${src}</a></p>`;
        }
        default:
            return customOutput(n, custom, "html");
    }
}
// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------
/** Markdownで意味を持つ文字をエスケープする。 */
export function escapeMarkdown(s) {
    return s
        .replace(/[\\`*_{}[\]()<>#+!|~]/g, "\\$&")
        .replace(/^(\s*)([-=])/gm, "$1\\$2")
        .replace(/^(\s*)(\d+)\./gm, "$1$2\\.");
}
function textMarkdown(n) {
    const marks = n.marks ?? [];
    if (marks.some((m) => m.type === "code")) {
        const t = str(n.text);
        const fence = "`".repeat(longestRun(t, "`") + 1);
        return `${fence}${t.startsWith("`") || t.endsWith("`") ? ` ${t} ` : t}${fence}`;
    }
    let out = escapeMarkdown(str(n.text));
    for (const m of marks) {
        if (m.type === "bold")
            out = `**${out}**`;
        else if (m.type === "italic")
            out = `*${out}*`;
        else if (m.type === "strike")
            out = `~~${out}~~`;
        else if (m.type === "link")
            out = `[${out}](<${str(m.attrs?.["href"]).replace(/[<>]/g, encodeURIComponent)}>)`;
    }
    return out;
}
function longestRun(s, ch) {
    let best = 0;
    let run = 0;
    for (const c of s) {
        run = c === ch ? run + 1 : 0;
        best = Math.max(best, run);
    }
    return best;
}
const indent = (s, prefix, first = prefix) => s
    .split("\n")
    .map((line, i) => (i === 0 ? first : line === "" ? "" : prefix) + line)
    .join("\n");
function blocksMarkdown(nodes, custom) {
    return (nodes ?? []).map((c) => markdown(c, custom)).join("\n\n");
}
function inlineMarkdown(nodes, custom) {
    return (nodes ?? []).map((c) => markdown(c, custom)).join("");
}
function markdown(n, custom) {
    switch (n.type) {
        case "doc":
            return blocksMarkdown(n.content, custom);
        case "text":
            return textMarkdown(n);
        case "paragraph":
            return inlineMarkdown(n.content, custom);
        case "heading": {
            const level = Math.min(6, Math.max(1, Number(n.attrs?.["level"]) || 2));
            const id = str(n.attrs?.["id"]);
            return `${"#".repeat(level)} ${inlineMarkdown(n.content, custom)}${id ? ` {#${id}}` : ""}`;
        }
        case "bulletList":
            return (n.content ?? []).map((li) => indent(markdown(li, custom), "  ", "- ")).join("\n");
        case "orderedList": {
            const start = Number(n.attrs?.["start"]);
            const first = Number.isInteger(start) ? start : 1;
            return (n.content ?? [])
                .map((li, i) => {
                const marker = `${first + i}. `;
                return indent(markdown(li, custom), " ".repeat(marker.length), marker);
            })
                .join("\n");
        }
        case "listItem":
            return blocksMarkdown(n.content, custom);
        case "blockquote":
            return indent(blocksMarkdown(n.content, custom), "> ");
        case "codeBlock": {
            const code = (n.content ?? []).map((c) => str(c.text)).join("");
            const fence = "`".repeat(Math.max(3, longestRun(code, "`") + 1));
            const lang = str(n.attrs?.["language"]).replace(/[^A-Za-z0-9_+-]/g, "");
            return `${fence}${lang}\n${code}\n${fence}`;
        }
        case "horizontalRule":
            return "---";
        case "hardBreak":
            return "\\\n";
        case "image": {
            const alt = n.attrs?.["decorative"] === true ? "" : str(n.attrs?.["alt"]);
            return `![${escapeMarkdown(alt)}](${MEDIA_SCHEME}${str(n.attrs?.["mediaId"])})`;
        }
        case "embed": {
            const src = str(n.attrs?.["src"]);
            return `<${src.replace(/[<>]/g, encodeURIComponent)}>`;
        }
        default:
            return customOutput(n, custom, "markdown");
    }
}
// ---------------------------------------------------------------------------
// カスタムノード
// ---------------------------------------------------------------------------
function customOutput(n, custom, format) {
    const def = Object.prototype.hasOwnProperty.call(custom, n.type) ? custom[n.type] : undefined;
    const ctx = (f) => ({
        format: f,
        children: () => {
            const kids = n.content ?? [];
            if (f === "html")
                return kids.map((c) => html(c, custom)).join(def?.inline ? "" : "\n");
            return def?.inline ? inlineMarkdown(kids, custom) : blocksMarkdown(kids, custom);
        },
    });
    if (!def) {
        // 変換関数のないノード（デプロイの検査で止めるので、ふつうは来ない）は、テキストだけを出す
        const plain = plainText(n);
        return format === "html" ? escapeHtml(plain) : escapeMarkdown(plain);
    }
    if (format === "markdown")
        return str(def.toMarkdown(n, ctx("markdown")));
    if (def.toHtml)
        return str(def.toHtml(n, ctx("html")));
    // HTMLへの変換関数がなければ、Markdownへの変換の結果をエスケープして囲む
    const tag = def.inline ? "span" : "div";
    return `<${tag} data-node="${escapeHtml(n.type)}">${escapeHtml(str(def.toMarkdown(n, ctx("markdown"))))}</${tag}>`;
}
/** ノードの中のテキストを、そのまま抜き出す（カスタムノードの変換関数の雛形と同じ考え方）。 */
export function plainText(n) {
    if (typeof n.text === "string")
        return n.text;
    return (n.content ?? []).map(plainText).join(n.type === "doc" ? "\n\n" : "");
}
/** 本文を変換する。 */
export function convert(doc, format, custom = {}) {
    return format === "html" ? html(doc, custom) : markdown(doc, custom);
}
/** サーバーの組み込みJSの入口: 本文のフィールドごとに、Markdown と HTML を作る。 */
export function renderFields(docs, custom = {}) {
    const out = {};
    for (const [name, doc] of Object.entries(docs)) {
        out[name] = { markdown: convert(doc, "markdown", custom), html: convert(doc, "html", custom) };
    }
    return out;
}
