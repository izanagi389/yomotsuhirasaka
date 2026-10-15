// 拡張機能とマニフェストの本文の定義（`body`）のあいだの変換。
// - bodyDefinition: 拡張機能からマニフェストの `body` を作る（CLIが使う。Q57, Q142）
// - forManifest: ワークスペースのマニフェストで使えるノードとマークだけに、拡張機能を絞る（管理画面が使う）
// - sanitize: エディタのJSONから、マニフェストにない属性を取り除く（サーバーは定義にない属性を拒否する）
// - unknownTypes: エディタが扱えないノードとマーク（あれば本文を編集させない。読み込むと消えてしまうため）

import { type AnyExtension, type JSONContent, getSchema } from "@tiptap/core";
import type { NodeType, Schema } from "@tiptap/pm/model";

import { ANCHOR_ATTR } from "./nodes.ts";

/** マニフェストの本文のノードの定義（server/src/schema/manifest.rs の BodyNode）。 */
export interface BodyNode {
  name: string;
  children?: string[];
  attrs?: string[];
  marks?: boolean;
}

/** マニフェストの本文のマークの定義。 */
export interface BodyMark {
  name: string;
  attrs?: string[];
}

/** マニフェストの `body`。 */
export interface ManifestBody {
  nodes: BodyNode[];
  marks?: BodyMark[];
  embedProviders?: string[];
}

/** 見出しのアンカーIDは、マニフェストの定義によらずサーバーが扱う属性なので、定義には書かない。 */
function definedAttrs(typeName: string, attrs: Record<string, unknown> | undefined): string[] {
  return Object.keys(attrs ?? {}).filter((a) => !(typeName === "heading" && a === ANCHOR_ATTR));
}

/** content式から、子になれるノードを（スキーマの順で）取り出す。 */
function childrenOf(type: NodeType, schema: Schema): string[] {
  const found = new Set<string>();
  const seen = new Set<unknown>();
  const queue = [type.contentMatch];
  while (queue.length > 0) {
    const match = queue.pop()!;
    if (seen.has(match)) continue;
    seen.add(match);
    for (let i = 0; i < match.edgeCount; i++) {
      const edge = match.edge(i);
      found.add(edge.type.name);
      queue.push(edge.next);
    }
  }
  return Object.keys(schema.nodes).filter((n) => found.has(n));
}

/** 拡張機能から、マニフェストの `body` のノードとマークの定義を作る（埋め込みの提供元はテンプレートが決める）。 */
export function bodyDefinition(extensions: AnyExtension[]): { nodes: Required<BodyNode>[]; marks: Required<BodyMark>[] } {
  const schema = getSchema(extensions);
  const nodes = Object.values(schema.nodes).map((type) => ({
    name: type.name,
    children: childrenOf(type, schema),
    attrs: definedAttrs(type.name, type.spec.attrs),
    // 子のテキストにマークを付けられるか
    marks: type.inlineContent && type.spec.marks !== "",
  }));
  const marks = Object.values(schema.marks).map((type) => ({
    name: type.name,
    attrs: definedAttrs(type.name, type.spec.attrs),
  }));
  return { nodes, marks };
}

/** 拡張機能が定義するノードとマークの名前。 */
function namesOf(extensions: AnyExtension[]): { nodes: Set<string>; marks: Set<string> } {
  const schema = getSchema(extensions);
  return { nodes: new Set(Object.keys(schema.nodes)), marks: new Set(Object.keys(schema.marks)) };
}

/** エディタに必ず要るノード（ないマニフェストでは本文を編集できない）。 */
const REQUIRED_NODES = ["doc", "paragraph", "text"];

export interface Narrowed {
  /** エディタに渡す拡張機能。 */
  extensions: AnyExtension[];
  /** マニフェストにあるが、エディタが知らないノードとマーク（カスタムノードなど）。 */
  unsupported: string[];
  /** 本文を編集できるか（必須のノードがマニフェストにあるか）。 */
  editable: boolean;
}

/**
 * マニフェストにあるノードとマークだけに絞る。StarterKit のように複数のノードを含む拡張機能は、
 * 含まれるノードのうちマニフェストにないものを設定で外す。
 */
export function forManifest(extensions: AnyExtension[], body: ManifestBody): Narrowed {
  const allowedNodes = new Set(body.nodes.map((n) => n.name));
  const allowedMarks = new Set((body.marks ?? []).map((m) => m.name));
  const known = namesOf(extensions);
  const narrowed: AnyExtension[] = [];
  for (const ext of extensions) {
    if (ext.type === "node") {
      if (allowedNodes.has(ext.name)) narrowed.push(ext);
    } else if (ext.type === "mark") {
      if (allowedMarks.has(ext.name)) narrowed.push(ext);
    } else if (ext.name === "starterKit") {
      // StarterKit の中のノードとマークは、設定で false にすると外れる
      const off: Record<string, false> = {};
      for (const inner of namesOf([ext]).nodes) if (!allowedNodes.has(inner) && !REQUIRED_NODES.includes(inner)) off[inner] = false;
      for (const inner of namesOf([ext]).marks) if (!allowedMarks.has(inner)) off[inner] = false;
      narrowed.push(ext.configure({ ...ext.options, ...off }));
    } else {
      narrowed.push(ext);
    }
  }
  const unsupported = [
    ...[...allowedNodes].filter((n) => !known.nodes.has(n)),
    ...[...allowedMarks].filter((m) => !known.marks.has(m)),
  ];
  const editable = REQUIRED_NODES.every((n) => allowedNodes.has(n));
  return { extensions: narrowed, unsupported, editable };
}

/** 本文の中の、拡張機能が知らないノードとマークの名前（あれば本文を編集させない）。 */
export function unknownTypes(doc: JSONContent, extensions: AnyExtension[]): string[] {
  const { nodes, marks } = namesOf(extensions);
  const out = new Set<string>();
  const walk = (n: JSONContent) => {
    if (n.type && !nodes.has(n.type)) out.add(n.type);
    for (const m of n.marks ?? []) if (!marks.has(m.type)) out.add(m.type);
    for (const c of n.content ?? []) walk(c);
  };
  walk(doc);
  return [...out];
}

/** マニフェストにない属性を取り除いた写しを返す（見出しのアンカーIDは残す）。 */
export function sanitize(doc: JSONContent, body: ManifestBody): JSONContent {
  const nodeAttrs = new Map(body.nodes.map((n) => [n.name, new Set(n.attrs ?? [])]));
  const markAttrs = new Map((body.marks ?? []).map((m) => [m.name, new Set(m.attrs ?? [])]));
  const pick = (attrs: Record<string, unknown> | undefined, allowed: Set<string> | undefined, extra?: string) => {
    if (!attrs) return undefined;
    const kept = Object.entries(attrs).filter(([k]) => allowed?.has(k) || k === extra);
    return kept.length > 0 ? Object.fromEntries(kept) : undefined;
  };
  const walk = (n: JSONContent): JSONContent => {
    const out: JSONContent = { ...n };
    const attrs = pick(n.attrs, nodeAttrs.get(n.type ?? ""), n.type === "heading" ? ANCHOR_ATTR : undefined);
    if (attrs) out.attrs = attrs;
    else delete out.attrs;
    if (n.marks) {
      out.marks = n.marks.map((m) => {
        const a = pick(m.attrs, markAttrs.get(m.type));
        return a ? { type: m.type, attrs: a } : { type: m.type };
      });
    }
    if (n.content) out.content = n.content.map(walk);
    return out;
  };
  return walk(doc);
}
