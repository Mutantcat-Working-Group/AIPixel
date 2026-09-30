// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// @ts-check
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";

export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "node_modules/**",
      "src-tauri/**",
      "target/**",
      "example/**",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    languageOptions: {
      globals: { window: "readonly", document: "readonly", console: "readonly" },
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // 条件式调用 hook 会在运行时静默崩溃（React 抛 "Rendered fewer hooks
    // than expected"），这类问题单测和 tsc 都发现不了，只能靠静态规则兜。
      "react-hooks/rules-of-hooks": "error",
      // 其余几条是编译期推导出的性能建议，先只提醒不拦，别一次性推翻既有写法。
      "react-hooks/exhaustive-deps": "warn",
      "react-hooks/set-state-in-effect": "warn",
      "react-hooks/set-state-in-render": "warn",
      "react-hooks/purity": "warn",
      "react-hooks/immutability": "warn",
      "react-hooks/refs": "warn",
      "react-hooks/globals": "warn",
    },
  },
);
