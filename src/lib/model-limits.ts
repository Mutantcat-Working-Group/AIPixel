/** 常用模型的 Max tokens 对照表。
 *
 * 设置界面靠它给「Max tokens」一个像样的初值：用户填好模型名，两秒之内字段自己就位，
 * 不用去翻文档猜该填 4096 还是 64000。命中不了就回落到 `MAX_TOKENS_FALLBACK`
 * （和 Rust 侧 `crates/agent-core/src/limits.rs` 的 `FALLBACK_MAX_TOKENS` 同一个数），
 * 字段始终有值，也不会骗人。
 *
 * 只有前缀匹配：模型名后面常挂日期（claude-sonnet-4-5-20250929）、常带厂商前缀
 * （openai/gpt-4o、accounts/fireworks/models/...），把最后一段 `/` 之后的名字拿出来比。
 * 分隔符必须是 `-` 或 `.`：`deepseek-v4` 得认下 `deepseek-v4.1-flash`，不然用户手里
 * 最新那个型号只差一个小数点就查不到自己的上限了。
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

/** 长模式排前面：gpt-4o-mini 不能输给 gpt-4o，claude-3-5-sonnet 不能输给 claude-3-opus。 */
const SORTED = [...LIMITS].sort((a, b) => b.pattern.length - a.pattern.length);

/** 名字算这个 pattern 的自家人吗：名字相同，或者跟着分隔符往后接版本号。
 *
 * `-` 和 `.` 都算。分隔符不能省——`gpt-4` 不能抢走 `gpt-4o`，`qwen` 也不能抢走 `qwen2.5`。
 * 与 Rust 侧 `limits::name_matches` 同一套规则。
 */
function nameMatches(name: string, pattern: string): boolean {
  if (!pattern) return false;
  if (name === pattern) return true;
  return name.startsWith(`${pattern}-`) || name.startsWith(`${pattern}.`);
}

/** 剥掉 `vendor/` 一类的路径前缀，只留模型本名。 */
function bareName(model: string): string {
  const tail = model.split("/").pop() ?? model;
  return tail.trim().toLowerCase();
}

/** 表里查得到就返回常用上限，查不到返回 null（调用方决定回落）。 */
export function maxTokensHint(model: string): number | null {
  const name = bareName(model);
  if (!name) return null;
  const hit = SORTED.find((row) => nameMatches(name, row.pattern));
  return hit ? hit.max_tokens : null;
}

/** 设界面直接要的数：表里有就用表里的，没有就 8192。 */
export function maxTokensForModel(model: string): number {
  return maxTokensHint(model) ?? MAX_TOKENS_FALLBACK;
}
