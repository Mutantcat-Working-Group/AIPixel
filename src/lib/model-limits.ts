// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
/** 常用模型的 Max tokens 对照表。
 *
 * 设置界面靠它给「Max tokens」一个像样的初值：用户填好模型名，两秒之内字段自己就位，
 * 不用去翻文档猜该填 4096 还是 64000。命中不了就回落到 `MAX_TOKENS_FALLBACK`
 * （和 Rust 侧 `crates/agent-core/src/limits.rs` 的 `FALLBACK_MAX_TOKENS` 同一个数），
 * 字段始终有值，也不会骗人。
 *
 * 只有前缀匹配：模型名后面常挂日期（claude-sonnet-4-5-20250929）、常带厂商前缀
 * （openai/gpt-4o、accounts/fireworks/models/...），把最后一段 `/` 之后的名字拿出来比。
 * 名字先归一再比，`-` `.` `_` 三种分隔符一视同仁：`deepseek-v4` 得认下
 * `deepseek-v4.1-flash`，不然用户手里最新那个型号只差一个小数点就查不到上限了。
 *
 * 同一个模型在不同平台上的马甲也归这里管，见 `ALIASES`。认不出的代价不只是上限填错：
 * Rust 侧 `providers::echoes_reasoning` 会把它判成非推理模型，推理历史一丢，续写时
 * 模型当新问题重想一遍，用户看到的就是通篇重写。
 */

/** 表里没有这个模型时的默认值，必须和 Rust 侧的 FALLBACK_MAX_TOKENS 保持一致。
 *
 * 取 32768 而不是 8192：现在连小模型都普遍支持到 32k 以上，给少了用户只会看到
 * 半截代码。用户显式填过的值永远优先，这里只兜底。
 */
export const MAX_TOKENS_FALLBACK = 32768;

const LIMITS: { pattern: string; max_tokens: number }[] = [
  // ---------- Anthropic ----------
  { pattern: "claude-opus-4-5", max_tokens: 64000 },
  { pattern: "claude-sonnet-4-5", max_tokens: 64000 },
  { pattern: "claude-haiku-4-5", max_tokens: 64000 },
  { pattern: "claude-opus-4-1", max_tokens: 32000 },
  { pattern: "claude-opus-4", max_tokens: 32000 },
  { pattern: "claude-sonnet-4", max_tokens: 64000 },
  { pattern: "claude-haiku-4", max_tokens: 32000 },
  { pattern: "claude-3-7-sonnet", max_tokens: 64000 },
  { pattern: "claude-3-5-sonnet", max_tokens: 8192 },
  { pattern: "claude-3-5-haiku", max_tokens: 8192 },
  { pattern: "claude-3-opus", max_tokens: 4096 },
  { pattern: "claude-3-sonnet", max_tokens: 4096 },
  { pattern: "claude-3-haiku", max_tokens: 4096 },
  { pattern: "claude-2.1", max_tokens: 4096 },
  // ---------- OpenAI ----------
  { pattern: "gpt-5", max_tokens: 128000 },
  { pattern: "gpt-5-mini", max_tokens: 128000 },
  { pattern: "gpt-5-nano", max_tokens: 128000 },
  { pattern: "gpt-4.1", max_tokens: 32768 },
  { pattern: "gpt-4.1-mini", max_tokens: 32768 },
  { pattern: "gpt-4.1-nano", max_tokens: 32768 },
  { pattern: "gpt-4o", max_tokens: 16384 },
  { pattern: "gpt-4o-mini", max_tokens: 16384 },
  { pattern: "chatgpt-4o-latest", max_tokens: 16384 },
  { pattern: "gpt-4-turbo", max_tokens: 4096 },
  { pattern: "gpt-4-32k", max_tokens: 4096 },
  { pattern: "gpt-4", max_tokens: 8192 },
  { pattern: "o1", max_tokens: 100000 },
  { pattern: "o1-mini", max_tokens: 100000 },
  { pattern: "o1-pro", max_tokens: 100000 },
  { pattern: "o3", max_tokens: 100000 },
  { pattern: "o3-mini", max_tokens: 100000 },
  { pattern: "o4-mini", max_tokens: 100000 },
  // ---------- Gemini ----------
  { pattern: "gemini-2.5-pro", max_tokens: 65536 },
  { pattern: "gemini-2.5-flash", max_tokens: 65536 },
  { pattern: "gemini-2.0-flash", max_tokens: 8192 },
  { pattern: "gemini-1.5-pro", max_tokens: 8192 },
  { pattern: "gemini-1.5-flash", max_tokens: 8192 },
  // ---------- DeepSeek ----------
  { pattern: "deepseek-v4.1-flash", max_tokens: 65536 },
  { pattern: "deepseek-v4.1-pro", max_tokens: 65536 },
  { pattern: "deepseek-v4-flash", max_tokens: 65536 },
  { pattern: "deepseek-v4-pro", max_tokens: 65536 },
  { pattern: "deepseek-v4", max_tokens: 65536 },
  { pattern: "deepseek-reasoner", max_tokens: 65536 },
  { pattern: "deepseek-chat", max_tokens: 32768 },
  { pattern: "deepseek-v3", max_tokens: 65536 },
  { pattern: "deepseek-r1", max_tokens: 65536 },
  // ---------- Qwen ----------
  { pattern: "qwen3", max_tokens: 32768 },
  { pattern: "qwq", max_tokens: 32768 },
  { pattern: "qwen-max", max_tokens: 32768 },
  { pattern: "qwen-plus", max_tokens: 32768 },
  { pattern: "qwen-turbo", max_tokens: 8192 },
  { pattern: "qwen-long", max_tokens: 8192 },
  { pattern: "qwen-vl", max_tokens: 8192 },
  { pattern: "qwen2.5", max_tokens: 8192 },
  // ---------- 智谱 GLM ----------
  { pattern: "glm-4.6", max_tokens: 32768 },
  { pattern: "glm-4.5", max_tokens: 32768 },
  { pattern: "glm-4-plus", max_tokens: 8192 },
  { pattern: "glm-4-air", max_tokens: 8192 },
  { pattern: "glm-4v", max_tokens: 8192 },
  { pattern: "glm-4", max_tokens: 8192 },
  // ---------- Kimi / Moonshot ----------
  { pattern: "kimi-k2", max_tokens: 32768 },
  { pattern: "kimi-latest", max_tokens: 32768 },
  { pattern: "moonshot-v1", max_tokens: 8192 },
  // ---------- 豆包 ----------
  { pattern: "doubao-seed", max_tokens: 32768 },
  { pattern: "doubao-1.5-pro", max_tokens: 12288 },
  { pattern: "doubao-pro", max_tokens: 4096 },
  // ---------- MiniMax ----------
  { pattern: "minimax-m2", max_tokens: 32768 },
  { pattern: "minimax-m1", max_tokens: 8192 },
  { pattern: "minimax-text", max_tokens: 8192 },
  // ---------- Llama 系 ----------
  { pattern: "llama-3.3", max_tokens: 32768 },
  { pattern: "llama-3.1", max_tokens: 32768 },
];

/** 同一个模型在不同平台上的简名。左边是别名叫，右边是表里的正式名。
 *
 * 平台之间从来不商量命名：官方叫 `deepseek-v4.1-flash`，中转站可能写成
 * `deepseek-v4-1-flash`（归一就能对上），也可能干脆只留 `deepseek-flash`
 * （版本整段省掉）。和 Rust 侧 `limits::ALIASES` 必须是同一份名单，两边各改一处
 * 就会一个填上限一个不填。
 */
const ALIASES: { alias: string; target: string }[] = [
  { alias: "deepseek-flash", target: "deepseek-v4.1-flash" },
];

/** 归一到可以比较的形式：剥掉厂商路径前缀、小写，`.` 和 `_` 都换成 `-`。
 *
 * `deepseek-v4.1-flash`、`deepseek-v4-1-flash`、`DeepSeek_V4.1_Flash` 是同一个东西，
 * 分开记三条就等着漏。归一之后分隔符只剩 `-`，前缀匹配也只需要考虑一种边界。
 * 与 Rust 侧 `limits::canonical_name` 同一套规则。
 */
function canonicalName(model: string): string {
  const tail = model.split("/").pop() ?? model;
  return tail.trim().toLowerCase().replace(/[._]/g, "-");
}

/** 认过别名再交表：同一个模型换几个马甲也查到同一份上限。
 *
 * 简名后面挂着日期之类的后缀时，把后缀一起搬过去：
 * `deepseek-flash-20260101` 该变成 `deepseek-v4-1-flash-20260101`。
 * 与 Rust 侧 `limits::resolve_alias` 同一套规则。
 */
export function resolveAlias(model: string): string {
  const name = canonicalName(model);
  for (const { alias, target } of ALIASES) {
    const rest =
      name === alias
        ? ""
        : name.startsWith(`${alias}-`)
          ? name.slice(alias.length + 1)
          : null;
    if (rest === null) continue;
    const full = canonicalName(target);
    return rest ? `${full}-${rest}` : full;
  }
  return name;
}

/** 表里查得到就返回常用上限，查不到返回 null（调用方决定回落）。
 *
 * 命中的最长者赢，跟表里的书写顺序无关：`gpt-4o-mini` 不能输给 `gpt-4o`，
 * `claude-3-5-sonnet` 也不能输给 `claude-3-opus`。与 Rust 侧 `ceiling_for` 同规则。
 */
export function maxTokensHint(model: string): number | null {
  const name = resolveAlias(model);
  if (!name) return null;
  let best: { len: number; tokens: number } | null = null;
  for (const row of LIMITS) {
    const pattern = canonicalName(row.pattern);
    if (!pattern) continue;
    if (name !== pattern && !name.startsWith(`${pattern}-`)) continue;
    if (!best || pattern.length > best.len) {
      best = { len: pattern.length, tokens: row.max_tokens };
    }
  }
  return best ? best.tokens : null;
}

/** 设界面直接要的数：表里有就用表里的，没有就 8192。 */
export function maxTokensForModel(model: string): number {
  return maxTokensHint(model) ?? MAX_TOKENS_FALLBACK;
}
