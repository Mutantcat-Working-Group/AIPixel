// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// defineConfig 从 vitest 取：它把 vite 的配置形状放宽出 test 一段，
// 从 vite 取的话 tsc 会拿「未知属性 test」把构建拦下来。
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

import { readFileSync } from "node:fs";

const host = process.env.TAURI_DEV_HOST;

// 版本号只有一个出处：package.json。构建期烘进常量，设置里的「关于」不用再问 Tauri。
const pkg = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8")) as {
  version: string;
};

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  // 样式合同测试要读 styles.css 的原文：vitest 默认把 CSS 模块打成空壳，
  // ?raw 拿回来的是空串，断言全变成查不到原因的 undefined。
  test: { css: true },
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: "es2022",
    minify: "esbuild",
    sourcemap: false,
  },
});
