// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 组件里取文案的唯一入口：跟着 store 的 lang 走，切语言时整棵树重渲染。
// 单独一层是为了不让组件去 import i18n 与 store 两处，也避免 store -> i18n 的环。

import { translate, type TVARS, type TKey } from "./i18n";
import { useStore } from "./store";

export type T = (key: TKey, vars?: TVARS) => string;

export function useT(): T {
  const lang = useStore((s) => s.lang);
  return (key, vars) => translate(lang, key, vars);
}
