// 管理画面のリンターの設定。`v-html` は使わない（13-security.md「管理画面の応答のセキュリティヘッダー」、CLAUDE.md の禁止事項）。
import pluginVue from "eslint-plugin-vue";
import tseslint from "typescript-eslint";

export default [
  { ignores: [".nuxt/**", ".output/**", "node_modules/**"] },
  ...tseslint.configs.recommended,
  ...pluginVue.configs["flat/recommended"],
  {
    files: ["**/*.vue"],
    languageOptions: { parserOptions: { parser: tseslint.parser } },
  },
  {
    rules: {
      "vue/no-v-html": "error",
      "vue/no-v-text-v-html-on-component": "error",
      // 動的なコードの評価も禁じる（CSP で unsafe-eval を許さない）
      "no-eval": "error",
      "no-implied-eval": "error",
      "no-new-func": "error",
      "vue/multi-word-component-names": "off",
      // shadcn-vue の部品の書き方（省略できる props に既定値を書かない）に合わせる
      "vue/require-default-prop": "off",
      "vue/max-attributes-per-line": "off",
      "vue/singleline-html-element-content-newline": "off",
      "vue/html-self-closing": "off",
    },
  },
];
