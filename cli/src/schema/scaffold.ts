// `yomi node scaffold <name>`: カスタムノードの変換関数の雛形を作る（03-content-and-editing.md「カスタムノード」、Q37）。
// - ノードの中のテキストを素朴に抜き出すだけの toMarkdown を、<テンプレート>/nodes/<名前>.ts に書く
// - template.json は書き換えない（書式を変えないため）。足す宣言の JSON を表示する

import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { TemplateError } from "./errors.js";

/** ノードの名前（サーバーと同じ: 英字で始まる英数字、40文字まで）。 */
const NODE_NAME = /^[A-Za-z][A-Za-z0-9]{0,39}$/;

export function scaffoldSource(name: string): string {
  return `// カスタムノード「${name}」の Markdown への変換関数（yomi node scaffold で作った雛形）。
// ノードの中のテキストを素朴に抜き出すだけなので、ノードに合わせて書き換えてください。
// 純粋な関数にすること（ホスト関数や外部への通信は使えません）。
import type { CustomNode, ConversionContext } from "@yomotsuhirasaka/cli/types";

function text(node: CustomNode): string {
  if (typeof node.text === "string") return node.text;
  return (node.content ?? []).map(text).join("");
}

export default function toMarkdown(node: CustomNode, ctx: ConversionContext): string {
  const inner = ctx.children();
  return inner !== "" ? inner : text(node);
}
`;
}

/** 雛形を書き、template.json に足す宣言を返す。 */
export function scaffoldNode(templateDir: string, name: string, options: { inline: boolean }): { file: string; declaration: unknown } {
  if (!NODE_NAME.test(name)) throw new TemplateError("the node name must start with a letter and use only letters and digits (at most 40)");
  if (!existsSync(join(templateDir, "template.json"))) throw new TemplateError(`${templateDir} has no template.json`);
  const dir = join(templateDir, "nodes");
  const file = join(dir, `${name}.ts`);
  if (existsSync(file)) throw new TemplateError(`${file} already exists`);
  mkdirSync(dir, { recursive: true });
  writeFileSync(file, scaffoldSource(name), { flag: "wx" });
  const declaration = {
    name,
    inline: options.inline,
    attrs: [],
    children: options.inline ? ["text"] : ["paragraph"],
    toMarkdown: { file: `./nodes/${name}.ts` },
  };
  return { file, declaration };
}
