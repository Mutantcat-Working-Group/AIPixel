//! Agent 主循环：prompt admission -> provider 流式输出 -> tool_use -> 工具执行 -> 结果回填 -> 续轮。
//!
//! 设计要点：
//! - 文档是唯一权威状态，每轮重新组装系统提示词（上一轮工具可能已经改过 canvas）。
//! - 模型永远不手写矩阵：所有绘制经由 `tools::execute`（ops / Lua 沙箱 / RLE 读回）。
//! - 预算：`max_tool_steps`（单 turn 工具步数）、`max_turns`（工具跑完再来一问的次数）、
//!   `max_tool_result_bytes`（回灌截断），外加「同一个失败调用连续 3 次」的退避保护。
//! - 护栏：`loop_limits` 给续写、重试、纯思考续写分别封顶。模型再轴也有收摊的时刻。
//! - 没说完的话：stop reason 是 `max_tokens` / `length` 就自动续写，最多 5 次；
//!   请求失败（网络、假死）按 1s/2s/4s/8s 退避重发，最多 5 次。
//! - 中断：流式期间按 120ms 轮询取消标志，`interrupt()` 立刻收尾并回 `Interrupted`。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use super::imagegen::{self, ImageGenParams, LandSpot};
use super::knowledge;
use super::limits;
use super::mcp::{self, McpRegistry};
use super::models::{
    ActiveContext, AgentEvent, ApprovalDecision, Attachment, Capabilities, ChatRequest,
    ContentBlock, LlmEvent, Message, ModelConfig, PermissionMode, Role, RunnerConfig, ToolSpec,
    UiText,
};
use super::plan::{self, TurnPlan, PLAN_TOOL};
use super::prompt;
use super::providers::{self, LlmProvider, ProviderError};
use super::roles::{ModelRole, RoleBinding};
use super::tools::{self, ToolOutcome, IMAGE_GEN_TOOL};
use pixel_core::decode;
use pixel_core::document::Document;

/// 一次待执行的工具调用（JSON 解析失败的也排进来，让模型收到可修复的错误）。
struct PlannedCall {
    id: String,
    name: String,
    input: Value,
    parse_error: Option<String>,
}

/// 挂起中的审批。一个 turn 顺序执行工具，同时最多挂一条，所以单槽就够。
struct ApprovalSlot {
    call_id: String,
    tx: oneshot::Sender<ApprovalDecision>,
}

/// 整个响应流允许静默多久。
///
/// 想得久一点没关系——推理增量、心跳、工具入参分段都会带来字节；
/// 这么久一个字节都没有，基本就是连接假死（代理把流吞了、对端不吭声挂了）。
/// 不设这条线的话 `stream.next()` 会永远挂住，前端只剩一个思考节点空转。
const STREAM_IDLE_LIMIT: Duration = Duration::from_secs(180);

/// 「整轮一个工具都没调」时最多催几次。
///
/// 模型偶尔会犯一种很亏的毛病：用户让它画东西，它动嘴不动手，讲两句
/// 「已完成」就收场。用户拿到一句汇报，画布上却一片空白。这种轮次得
/// 当场拦住再问一次。催的次数必须封顶，不然一个轴模型能把整轮预算
/// 全耗在互相瞪眼上。
const MAX_TOOL_NUDGES: usize = 2;

/// 「用户在编辑器里动了什么」最多攒几条。
///
/// 用户可能连着涂抹几十下才想起问模型一句。全塞进去既浪费 token 也让模型
/// 找不到重点；挤掉最旧的，是因为越近的改动对「下一步该画什么」越有参考价值。
const MAX_PENDING_EDITS: usize = 12;

/// 流静默计时器：记下最后一次见到字节的时刻，答一句「是不是该判死刑了」。
#[derive(Debug, Clone, Copy)]
struct IdleWatch {
    last_data: Instant,
    limit: Duration,
}

impl IdleWatch {
    fn new(limit: Duration) -> Self {
        Self {
            last_data: Instant::now(),
            limit,
        }
    }

    fn touch(&mut self) {
        self.last_data = Instant::now();
    }

    fn expired(&self) -> bool {
        self.last_data.elapsed() >= self.limit
    }
}

/// 回复撞上输出上限时，最多替用户自动续写几次。
///
/// provider 报的 stop reason 是 `max_tokens` / `length` 只说明「话没说完」，
/// 不等于模型说完了。像素画的 Lua 脚本动辄几百行，撞线极其常见；
/// 不续写的话，用户看到的永远是一段半截代码。
///
/// 取 20 而不是 5：按常用模型的 64k 上限算，这相当于一百三十万 token 的总产出，
/// 对一段分镜脚本来说就是「写不完不收手」。真正的刹车是 `continuation_progressed`——
/// 模型一旦开始原地复读，第一发就收，不会白烧二十次。
///
#[cfg(test)]
// 这只是默认值，用户在设置里能调；`LoopLimits::DEFAULT` 是同一组数的唯一出处。
const MAX_CONTINUATIONS: usize = crate::models::LoopLimits::DEFAULT.max_continuations;

/// 续写一次至少要吐出这么多个字符，否则算没推进。
const MIN_CONTINUATION_GAIN: usize = 8;

/// 回灌「你写到哪里了」时，尾巴截多长。
const RESUME_TAIL_CHARS: usize = 400;

/// 结尾是不是断在半截。
///
/// 有的 provider 不老实：撞了输出上限，finish_reason 却报成 `stop`，或者干脆不返回
/// 这个字段。对这种平台，唯一的线索是文字本身没写完——代码块没合上、一行以连接符
/// 收尾。只收高精度的信号：猜错一次就要白跑一整轮续写。
///
/// 猜错也不要紧。续写发出去，模型只会答「没什么要补充的」，那时候照原样收场，
/// 不会拿一句报错把好好一条回复判成失败。
fn looks_cut_off(text: &str) -> bool {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return false;
    }
    // 奇数个 ``` 一定意味着后面还有内容。写像素画脚本的 agent 最吃这一条。
    if trimmed.matches("```").count() % 2 == 1 {
        return true;
    }
    let last = trimmed.lines().last().unwrap_or_default().trim_end();
    // 一行以连接符收尾：表达式、参数表、对象字面量都被从中间剪了一刀。
    // 中文破折号「——」是全角字符，碰不到这张 ASCII 表，不会被误伤。
    const DANGLING: &[&str] = &[",", "+", "&&", "||", "=>", "->", "(", "{", "[", "="];
    DANGLING.iter().any(|tail| last.ends_with(tail))
}

/// 这一发实际该给多少 max_tokens。
///
/// 优先级：provider 报过的上限 < 用户显式填的值 < 按模型名查表。
/// 用户填过的值必须被尊重；provider 报过的上限是硬事实，谁的数小听谁的。
/// 输出短了不丢人——续写机制会把几截拼成一条完整回复。
fn resolve_max_tokens(user: Option<u32>, model: &str, ceiling: Option<u32>) -> u32 {
    let wanted = user.unwrap_or_else(|| limits::ceiling_for(model));
    ceiling.map_or(wanted, |c| c.min(wanted))
}

/// 一次请求失败后最多重试几次：网络抖动、连接被代理掐断、流直接报错。
///
#[cfg(test)]
// 同样只是默认值，用户能在设置里把它调到 0（失败立即现形）或更大。
const MAX_ROUND_RETRIES: usize = crate::models::LoopLimits::DEFAULT.max_retries;

/// 「整轮只写了一篇推理」的长度门槛，字符数。
///
/// 平台不报截断时，光看 stop reason 认不出这种轮次。这个阈值比一段正常思考
/// 长得多：真想清楚了的模型写个三五行就动笔了，写到这里还没动的，就是把
/// 预算烧空了。
const REASONING_ONLY_CHARS: usize = 600;

/// 连续几轮只吐推理就收手。
///
/// 推理模型最常见的死法：整轮预算全烧在思考上，正文一个字没有，工具一个不调。
/// 这种轮次当然值得续（见 `a_round_that_only_thought_is_still_progress`），
/// 但续写的上限必须比正文续写紧得多——要三轮还在想，说明它根本不想动笔，
#[cfg(test)]
// 再要二十次也只是让用户盯着一个空转的思考节点。
const MAX_REASONING_CONTINUATIONS: usize =
    crate::models::LoopLimits::DEFAULT.max_reasoning_continuations;

/// 这次失败值不值得重发。
///
/// 网络抖、连接被代理掐、流解析到一半断了、限流 429、5xx——换个时间再来就好。
/// 而 400/401/404/413/422 是请求本身写错了：key 不对、模型名打错、路径没挂上，
/// 原样重发多少次都是同一个报错，所以立刻现形，让用户去改配置。
///
/// 403 走另一条路：它看着像「服务器不答应」，可 `permission_denied_error` 里有
/// 相当一部分是网关侧的限流窗口、令牌套餐切换、区域策略在作怪，等一等就放行。
/// 宁可让用户等十几秒看到同一个 403，也不能把一条其实能跑通的请求判死。
fn retryable(err: &ProviderError) -> bool {
    match err {
        ProviderError::Network(_) | ProviderError::Decode(_) => true,
        ProviderError::Http { status, .. } => {
            matches!(*status, 403 | 408 | 409 | 425 | 429) || (500..=599).contains(status)
        }
        ProviderError::Config(_) => false,
    }
}

/// 续写时对照的「已经写到哪里」：正文和推理各留一段尾巴。
///
/// 模型续写时最爱犯的毛病，是把刚写完的最后几句原样再念一遍。念出来的字会顺着
/// `LlmEvent::Token` 直达界面——等整轮收场才发现重复，用户已经看到同一段话写了
/// 两遍。所以在流式入口就按住比对，复读的那截在推给前端之前就吃掉。
///
/// 判据只有一条：复读的那截必然是「已写内容里某一段的开头，一直连到刚写完的
/// 地方」。模型被掐断后收到的回灌里就带着最后几百个字，它下一句照着念的
/// 就是那一段。所以匹配得在整个窗口里找起点，不能只盯最后一个字——复读
/// 常常是从好几句之前那个字开始的，只对尾巴的话一句都拦不住。
struct EchoTrim {
    /// 正文这一路。
    text: EchoStream,
    /// 同 `text`，只是这一路是推理。推理一样会复读，而且复读起来更啰嗦。
    reasoning: EchoStream,
    /// 已经吃掉的复读字符数。只为了把续写指令说得更重。
    eaten: usize,
}

/// 对照窗口取多长。模型复读的通常就是最后一段话，再往上就是另一段了；
/// 窗口越长比对越贵，而多出来的部分基本对不上。
const ECHO_WINDOW: usize = 400;

/// 短于这个长度的「像已写内容」不算复读：换行、半个括号、一个句号常常是正经内容。
const ECHO_MIN: usize = 16;

/// 匹配的段落不在已写内容的末尾收头、却又这么长，认它是跳回前面重念。
/// 对代码来说十几行的重复少见，对「从头再写一遍」来说正好。
const ECHO_DEEP: usize = 48;

/// 正文或推理其中一路的复读筛：一个对照窗口，加手里还没下定论的一截。
///
/// 窗口记的是「已经写到哪儿了」，`held` 是新流进来正在比对的那截。正文和推理
/// 各有一份：模型可能复读推理却接着正文往下写，也可能反过来。
struct EchoStream {
    /// 已写的最后 `ECHO_WINDOW` 个字符。
    window: Vec<char>,
    /// 正在比对、还没下定论的分片。整段都像尾巴时先按住，看下一个字符。
    held: String,
    /// 窗口里哪些下标开头还接着对得上。空表示没在比对。
    cands: Vec<usize>,
}

impl EchoStream {
    fn new() -> Self {
        Self {
            window: Vec::new(),
            held: String::new(),
            cands: Vec::new(),
        }
    }

    /// 把这一段新写的并进窗口。续写判定必须跨轮活着：模型复读的常常不是最后
    /// 一句，而是上一轮整段。
    fn absorb(&mut self, written: &str) {
        self.window.extend(written.chars());
        keep_tail(&mut self.window, ECHO_WINDOW);
    }

    /// 收下一段新流进来的内容，返回真正该放行的那部分。整段都在复读就返回空。
    ///
    /// 逐字走：每来一个字，先看手里这些候选起点还能不能往前接，接不上的当场
    /// 淘汰。一个候选都不剩了，说明手里按住的这截到此为止——前面那段是照搬，
    /// 最后这个字才是新内容。候选还有气就继续按住：现在放行，放出去的可能
    /// 正是复读的头几个字。
    fn feed(&mut self, chunk: &str, eaten: &mut usize) -> String {
        let mut fresh = String::new();
        for ch in chunk.chars() {
            let k = self.held.chars().count();
            // 手里这段要是正好顶到已写内容的末尾，这一字就会把那个候选断掉。
            // 对上的那段一路连到已写内容的末尾，是复读最地道的形状：模型接着
            // 自己刚写完的地方往下念。
            let anchored = k > 0 && self.cands.iter().any(|&i| i + k == self.window.len());
            if k == 0 {
                // 新开一段：窗口里每个同字的位置都可能是复读的起点。
                self.cands.clear();
                self.cands.extend(
                    self.window
                        .iter()
                        .enumerate()
                        .filter(|(_, &c)| c == ch)
                        .map(|(i, _)| i),
                );
            } else {
                // 已在比对：每个候选都往前赶一个字，对不上的淘汰。
                self.cands.retain(|&i| self.window.get(i + k) == Some(&ch));
            }
            self.held.push(ch);
            if !self.cands.is_empty() {
                continue;
            }
            // 都对不上了：手里前 k 个字是照搬。太短的当正经内容放行，别误删；
            // 对在中间就断的，得长到 ECHO_DEEP 才认——十几二十个字的重合在
            // 代码和套话里太常见了，删了就是删真东西。
            if k >= ECHO_MIN && (anchored || k >= ECHO_DEEP) {
                *eaten += k;
                fresh.push(ch);
            } else {
                fresh.push_str(&self.held);
            }
            self.held.clear();
        }
        if !fresh.is_empty() {
            // 放行出去的字要并回窗口：下一段的复读判定得照最新的写。
            self.window.extend(fresh.chars());
            keep_tail(&mut self.window, ECHO_WINDOW);
        }
        fresh
    }

    /// 一段流收尾时手上还按着的那截怎么算。够长又顶在末尾的，是模型念到刚写完
    /// 的地方就没词了；剩下的当正经内容还回去。
    fn seal(&mut self, eaten: &mut usize) -> String {
        let len = self.held.chars().count();
        let anchored = self.cands.iter().any(|&i| i + len == self.window.len());
        let fresh = if len >= ECHO_MIN && (anchored || len >= ECHO_DEEP) {
            *eaten += len;
            String::new()
        } else {
            std::mem::take(&mut self.held)
        };
        self.held.clear();
        self.cands.clear();
        fresh
    }
}

impl EchoTrim {
    fn new(reasoning: &str, text: &str) -> Self {
        let mut trim = Self {
            text: EchoStream::new(),
            reasoning: EchoStream::new(),
            eaten: 0,
        };
        trim.absorb(reasoning, text);
        trim
    }

    /// 把这一轮新写下的内容并进对照窗口。续写判定必须跨轮活着：模型复读的
    /// 常常不是最后一句，而是上一轮整段。
    fn absorb(&mut self, reasoning: &str, text: &str) {
        self.reasoning.absorb(reasoning);
        self.text.absorb(text);
    }

    /// 收下一段新正文，返回真正该放行的那部分。整段都在复读就返回空。
    fn feed_text(&mut self, chunk: &str) -> String {
        self.text.feed(chunk, &mut self.eaten)
    }

    /// 收下一段新推理。推理一样会复读，而且复读起来更啰嗦。
    fn feed_reasoning(&mut self, chunk: &str) -> String {
        self.reasoning.feed(chunk, &mut self.eaten)
    }

    /// 这一轮收尾。还按住的那截整段都在照搬，而那些字前面已经写过了，
    /// 吃掉不会少任何内容——但得把状态清干净，不然下一轮对着残渣比对。
    /// 对不齐、又不够长的，宁可放行：删错一个字的代价比多显示一遍高。
    fn seal(&mut self) -> (String, String) {
        let fresh_text = self.text.seal(&mut self.eaten);
        let fresh_reasoning = self.reasoning.seal(&mut self.eaten);
        (fresh_text, fresh_reasoning)
    }

    /// 之前有没有吃过复读。为了让续写指令把话说重。
    fn ate_something(&self) -> bool {
        self.eaten > 0
    }
}

/// 只留最后 `keep` 个字符。中文一个字也是一个字符，这里一律按字符算。
fn keep_tail(text: &mut Vec<char>, keep: usize) {
    let total = text.len();
    if total <= keep {
        return;
    }
    text.drain(..total - keep);
}

/// 续写时回灌给模型的指令。界面文案走字典，这句是喂模型的，必须英文。
///
/// 关键是把「你写到哪里了」的原文尾巴一起给它。只说一句泛泛的「继续」，
/// 模型很可能重起一趟：先复述前面写过的，再往下接——用户看到的就是同一段话
/// 被写了两遍。把断点原文按在眼前，它只能接着那几个字往下走。
fn continue_nudge(carried: &str, repeated: bool) -> String {
    // 上一发续写已经复读过了：光说「别重复」不够，得把话说重。
    // 模型的通病是看见「继续」就从头念，念过一次尤其容易再念。
    let scold = if repeated {
        "\n\nYour previous continuation opened by repeating text that was already written \
above. That repetition was discarded. Do not do it again: start with the first \
character that is actually missing."
    } else {
        ""
    };
    let tail: String = carried
        .chars()
        .rev()
        .take(RESUME_TAIL_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!(
        "Your previous reply was cut off by the output limit before it finished. \
You had already written {} characters, and it stopped mid-stream at exactly this point:\n\n\
---{tail}\n---\n\n\
Continue from that exact point. Output only the text that is still missing: \
do not repeat any of it, do not restate or summarise what you already wrote, \
do not re-announce a plan, and do not start over. \
Finish the line, statement, or tool call that was in progress.{scold}",
        carried.chars().count(),
    )
}

/// 这一发续写到底有没有往下走。
///
/// 模型偶尔会犯轴：把已经写完的原样再吐一遍，或者干脆只吐几个字。长度在涨，
/// 内容一步没动。这种续写再要二十次也白搭，不如当场收手告诉用户断在哪。
fn continuation_progressed(carried: &str, fresh: &str) -> bool {
    // 第一发还没攒下任何内容，没有锚点可比较。此时哪怕只接上几个字
    // （一个反引号、半个函数名）也算往前走，不该被当成复读。
    if carried.is_empty() {
        return !fresh.is_empty();
    }
    // 原样复读：模型把已经写过的又念了一遍。这条比长度更准，所以排在前头。
    if carried.contains(fresh) {
        return false;
    }
    // 真的写了新东西，但只有一两个字（一个句号、半个括号）。这种续写再要二十次
    // 也拼不出结尾，是原地打转，不是写完前的最后一口气。门槛压到 8：收尾常常
    // 就剩一个 ``` 加一个 end，卡太紧会把好好一条回复判成卡死。
    if fresh.chars().count() < MIN_CONTINUATION_GAIN {
        return false;
    }
    !carried.contains(fresh)
}

/// 重试退避：1s、2s、4s、8s，之后封顶。失败不该把用户晾在原地干等。
fn retry_backoff(attempt: usize) -> Duration {
    Duration::from_millis(1000u64 << attempt.saturating_sub(1).min(3))
}

/// 重发前的退避等待。
///
/// 测试里整段跳过：一组五次重试真要等 23 秒，而假 provider 的剧本一秒就能走完。
#[cfg(test)]
async fn pause_before_retry(_attempt: usize) {}

#[cfg(not(test))]
async fn pause_before_retry(attempt: usize) {
    tokio::time::sleep(retry_backoff(attempt)).await;
}

/// 一次回复是怎么收场的：模型自己说完了，还是被输出上限掐断了。
/// 两者的后续动作完全相反——一个该收尾，一个该接着写。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopKind {
    Finished,
    Truncated,
}

/// 只有输出上限算掐断；`end_turn` / `tool_use` / `stop_sequence` / 空值都是说完了。
fn stop_kind(reason: &str) -> StopKind {
    match reason {
        "max_tokens" | "length" => StopKind::Truncated,
        _ => StopKind::Finished,
    }
}

/// 一整轮流式输出的原始收获。单独拎出来是为了「失败可以重发、掐断可以续写」：
/// 重试时已经收到的文本不能丢，续写时要把它作为历史回灌给模型。
struct RoundRaw {
    text: String,
    reasoning: String,
    /// 只留完整的 tool_use。掐断那半截入参不可信，更不能进历史——
    /// 历史里出现残缺的 tool_use 会让下一轮请求直接被 provider 拒掉。
    accumulator: BTreeMap<usize, (String, String, String)>,
    stop: StopKind,
    failure: Option<String>,
    /// 失败值不值得再发一次：429 该退避重来，401 就现原形。
    retryable_failure: bool,
    cancelled: bool,
}

impl RoundRaw {
    fn empty() -> Self {
        Self {
            text: String::new(),
            reasoning: String::new(),
            accumulator: BTreeMap::new(),
            stop: StopKind::Finished,
            failure: None,
            retryable_failure: false,
            cancelled: false,
        }
    }

    /// 还有东西能接着写才算可续。一个字都没吐出来的不算「说了一半」，
    /// 那是重试该管的事，硬续只会把同一个空回复再要五遍。
    fn resumable(&self) -> bool {
        !self.text.is_empty() || !self.reasoning.is_empty()
    }

    /// 值得进历史的回复块：推理在前、正文在后，与流里的到达顺序一致。
    fn blocks(&self) -> Vec<ContentBlock> {
        let mut blocks: Vec<ContentBlock> = Vec::new();
        if !self.reasoning.is_empty() {
            blocks.push(ContentBlock::Reasoning {
                text: self.reasoning.clone(),
            });
        }
        if !self.text.is_empty() {
            blocks.push(ContentBlock::Text {
                text: self.text.clone(),
            });
        }
        blocks
    }
}
/// 单个 agent 会话：持有文档（权威状态）、消息历史与 provider。
/// 纯 Rust，不依赖 Tauri；上层通过 `tokio::sync::mpsc` 通道收 `AgentEvent`。
struct Engine {
    config: ModelConfig,
    provider: Arc<dyn LlmProvider>,
}

pub struct AgentSession {
    pub id: String,
    /// 模型配置与 provider 放在同一把锁里，运行时切换模型不必重建会话、丢历史。
    engine: Mutex<Engine>,
    /// 生图 / 识图 / 读视频的另外三个模型。按角色各一个，没有就回落主模型。
    /// 和 engine 分成两处而不是塞进 Engine：主模型的语义是「会话本身那把」，
    /// 换它要连带提示词和工具默认值一起变，角色模型只是某段流程的替身。
    role_engines: Mutex<BTreeMap<ModelRole, Engine>>,
    runner_config: Mutex<RunnerConfig>,
    messages: Mutex<Vec<Message>>,
    document: Mutex<Document>,
    active: Mutex<ActiveContext>,
    cancelled: AtomicBool,
    /// 当前挂起的审批发送端；None 表示没有调用在等用户。
    approval: Mutex<Option<ApprovalSlot>>,
    /// 用户自配的 MCP 工具服务器注册表；None 表示这个会话不接外部工具。
    /// 放锁里是因为「全局 MCP 开关」要在会话活着的时候摘掉它：
    /// 用户在设置里关掉 MCP，下一轮就不该再把外部工具暴露给模型。
    mcp: Mutex<Option<Arc<McpRegistry>>>,
    /// 侧边栏显示名。None = 用默认编号，用户改过就是改过的名字。
    title: Mutex<Option<String>>,
    /// 排序位。新建时拿自增序号，前端拖动排序后整批改写。
    order: AtomicU64,
    /// 合成 function_call 节点的自增号。用自增而不是随机串：
    /// 前端拿它当列表 key、配对 ToolCall/ToolResult，重号会让节点自己合并掉。
    ref_nodes: AtomicU64,
    /// 这一轮的分流表：参照定性、成品意图、风格预设、知识条目。
    /// 开 turn 时按用户原话定一次，模型中途调 pixel_plan 纠正时就地改写——
    /// 出图清单和提示词的 TURN ROUTING 段必须跟着改，不然模型嘴上要的是
    /// 「只借画风」，清单里还写着「视觉真值」，两处各说各话。
    plan: Mutex<TurnPlan>,
    /// 这一轮的用户原话。知识库每发一次请求都要重新检索一遍，
    /// 因为纠正可能把意图和风格换掉，命中的知识条目也该跟着换。
    turn_text: Mutex<String>,
    /// 用户在编辑器里动手的痕迹（一句话一条）。下一轮请求前冲刷成一条 user 消息，
    /// 让模型知道「画面已经被人改过了」，别照着自己上一轮的想象继续画。
    pending_edits: Mutex<Vec<String>>,
}

impl AgentSession {
    pub fn new(id: impl Into<String>, config: ModelConfig, document: Document) -> Self {
        let active = prompt::default_active(&document);
        let provider = providers::build_provider(&config);
        AgentSession {
            id: id.into(),
            runner_config: Mutex::new(RunnerConfig::default()),
            engine: Mutex::new(Engine { config, provider }),
            role_engines: Mutex::new(BTreeMap::new()),
            messages: Mutex::new(Vec::new()),
            document: Mutex::new(document),
            active: Mutex::new(active),
            cancelled: AtomicBool::new(false),
            approval: Mutex::new(None),
            mcp: Mutex::new(None),
            title: Mutex::new(None),
            order: AtomicU64::new(0),
            ref_nodes: AtomicU64::new(0),
            plan: Mutex::new(TurnPlan::default()),
            turn_text: Mutex::new(String::new()),
            pending_edits: Mutex::new(Vec::new()),
        }
    }

    pub fn with_runner_config(mut self, config: RunnerConfig) -> Self {
        *self.runner_config.get_mut().unwrap() = config;
        self
    }

    /// 挂上 MCP 注册表：会话每轮把它 expose 的工具并进工具清单，
    /// `mcp__*` 调用分流过去。注册表是共享的，会话换模型不影响连接。
    pub fn with_mcp_registry(mut self, registry: Arc<McpRegistry>) -> Self {
        *self.mcp.get_mut().unwrap() = Some(registry);
        self
    }

    /// 换掉（或摘掉）MCP 注册表。None = 这个会话从此不接外部工具，
    /// 已经暴露过的工具清单下一轮重建时自然消失。
    pub fn set_mcp_registry(&self, registry: Option<Arc<McpRegistry>>) {
        *self.mcp.lock().unwrap() = registry;
    }

    /// 这个会话现在接不接外部工具。给总开关和诊断用：不把注册表本身交出去，
    /// 想摘它的人只能走 set_mcp_registry。
    pub fn mcp_attached(&self) -> bool {
        self.mcp.lock().unwrap().is_some()
    }

    /// 给会话一个排序位。侧边栏拖动排序后按新的位次整批改写。
    pub fn with_order(self, order: u64) -> Self {
        self.order.store(order, Ordering::SeqCst);
        self
    }

    pub fn order(&self) -> u64 {
        self.order.load(Ordering::SeqCst)
    }

    pub fn set_order(&self, order: u64) {
        self.order.store(order, Ordering::SeqCst);
    }

    /// 侧边栏显示名；None 表示还没改过，前端拿默认编号显示。
    pub fn title(&self) -> Option<String> {
        self.title.lock().unwrap().clone()
    }

    pub fn set_title(&self, title: Option<String>) {
        let trimmed = title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        *self.title.lock().unwrap() = trimmed;
    }

    /// 记一笔「用户在编辑器里动了什么」。空文本不入簿：一条空行只会
    /// 让下一轮的上下文更啰嗦。簿子有上限，超长的历史会话不会把它撑爆。
    pub fn note_edit(&self, note: impl Into<String>) {
        let note = note.into();
        if note.trim().is_empty() {
            return;
        }
        let mut edits = self.pending_edits.lock().unwrap();
        // 记到上限就把最旧的一条挤掉：越近的改动对「下一步画什么」越有参考价值。
        if edits.len() >= MAX_PENDING_EDITS {
            edits.remove(0);
        }
        edits.push(note);
    }

    /// 取走所有待冲刷的改动记录。取走即清空，没人会读第二遍。
    pub fn take_edits(&self) -> Vec<String> {
        std::mem::take(&mut *self.pending_edits.lock().unwrap())
    }

    /// 把某个角色另绑到一个模型。配同一个角色就是换模型。
    /// 没配过这个角色时，它本来在蹭主模型的饭碗，现在才有自己的。
    pub fn rebind_role(&self, role: ModelRole, config: ModelConfig) {
        if !role.is_detachable() {
            // 主模型只能整体 rebind_provider，这里是调用方走错了门。
            return;
        }
        let provider = providers::build_provider(&config);
        self.role_engines
            .lock()
            .unwrap()
            .insert(role, Engine { config, provider });
    }

    /// 取消某个角色的单独绑定，让它回落去蹭主模型。没绑过就是空操作。
    pub fn clear_role(&self, role: ModelRole) {
        self.role_engines.lock().unwrap().remove(&role);
    }

    /// 这个角色实际该用哪个模型配置：单独绑了就用它，否则用主模型。
    /// 单模型用户什么都没配，所以拿到的永远是主模型，行为和以前一致。
    pub fn model_for_role(&self, role: ModelRole) -> ModelConfig {
        self.role_engines
            .lock()
            .unwrap()
            .get(&role)
            .map(|engine| engine.config.clone())
            .unwrap_or_else(|| self.model_config())
    }

    /// 四个角色各自实际在干的活，给 UI 摆「谁负责哪段流程」。
    /// 没有分工引擎的会话也照答：回落主模型，`detached` 是 false。
    pub fn role_bindings(&self) -> Vec<RoleBinding> {
        let primary = self.model_config();
        let roles = self.role_engines.lock().unwrap();
        ModelRole::all()
            .into_iter()
            .map(|role| match roles.get(&role) {
                Some(engine) => RoleBinding {
                    role,
                    model_id: engine.config.id.clone(),
                    model_label: engine.config.label.clone(),
                    detached: true,
                },
                None => RoleBinding {
                    role,
                    model_id: primary.id.clone(),
                    model_label: primary.label.clone(),
                    detached: false,
                },
            })
            .collect()
    }

    /// 会话实际能跑的工作流能力：把各角色自己那份能力并进来。
    /// 单模型用户没有角色引擎，并集就等于主模型自己的能力。
    pub fn effective_capabilities(&self) -> Capabilities {
        let mut caps = self.model_config().capabilities;
        for engine in self.role_engines.lock().unwrap().values() {
            caps.vision |= engine.config.capabilities.vision;
            caps.image_gen |= engine.config.capabilities.image_gen;
            caps.video |= engine.config.capabilities.video;
            caps.reasoning |= engine.config.capabilities.reasoning;
        }
        caps
    }

    /// 本会话可见的 MCP 工具规格（没挂注册表就空）。
    fn mcp_specs(&self) -> Vec<ToolSpec> {
        self.mcp
            .lock()
            .unwrap()
            .as_ref()
            .map(|r| r.specs())
            .unwrap_or_default()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn model_config(&self) -> ModelConfig {
        self.engine.lock().unwrap().config.clone()
    }

    pub fn runner_config(&self) -> RunnerConfig {
        self.runner_config.lock().unwrap().clone()
    }

    pub fn set_runner_config(&self, config: RunnerConfig) {
        *self.runner_config.lock().unwrap() = config;
    }

    /// 运行时切换模型：重建 provider，保留文档与消息历史。
    pub fn rebind_provider(&self, config: ModelConfig) {
        let provider = providers::build_provider(&config);
        *self.engine.lock().unwrap() = Engine { config, provider };
    }

    /// 用外部文档整体替换当前文档（前端加载 .aip 或撤销后同步回来）。
    /// 激活图层/帧若已不存在则回落到首个，避免后续工具调用落空。
    pub fn sync_document(&self, document: Document) {
        let mut doc = self.document.lock().unwrap();
        let mut active = self.active.lock().unwrap();
        if !doc_has_layer(&doc, &active.layer) {
            if let Some(first) = doc.layers.first() {
                active.layer = first.id.clone();
            }
        }
        if !doc_has_frame(&doc, &active.frame) {
            if let Some(first) = doc.frames.first() {
                active.frame = first.id.clone();
            }
        }
        *doc = document;
    }

    pub fn set_active(&self, active: ActiveContext) {
        *self.active.lock().unwrap() = active;
    }

    pub fn active(&self) -> ActiveContext {
        self.active.lock().unwrap().clone()
    }

    pub fn set_permission(&self, permission: PermissionMode) {
        self.runner_config.lock().unwrap().permission = permission;
    }

    /// 用户对一条挂起的工具调用给出决定。call_id 对不上说明这是过期决定
    /// （新 turn 已经开始），直接拒绝而不是把它送进死队列。
    pub fn resolve_approval(
        &self,
        call_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        let slot = self.approval.lock().unwrap().take();
        match slot {
            Some(pending) if pending.call_id == call_id => {
                let _ = pending.tx.send(decision);
                Ok(())
            }
            Some(_) => Err("approval request is stale".into()),
            None => Err("no approval is pending".into()),
        }
    }

    pub fn history(&self) -> Vec<Message> {
        self.messages.lock().unwrap().clone()
    }

    pub fn load_history(&self, messages: Vec<Message>) {
        *self.messages.lock().unwrap() = messages;
    }

    pub fn clear_history(&self) {
        self.messages.lock().unwrap().clear();
    }

    pub fn document(&self) -> Document {
        self.document.lock().unwrap().clone()
    }

    /// 借出文档做只读的一次性计算（渲染垫图、导出预览）。
    /// 和 `with_document_mut` 用同一把锁，但不会 bump revision，也不会让外界改到文档。
    pub fn with_document<T>(&self, f: impl FnOnce(&Document) -> T) -> T {
        let doc = self.document.lock().unwrap();
        f(&doc)
    }

    /// 借出文档做一次性原地修改（插帧、量化、编辑器操作），完成后返回新 revision。
    /// 主循环的 agent turn 也走同一把锁，所以这里不会和并发 turn 交织。
    pub fn with_document_mut<T>(&self, f: impl FnOnce(&mut Document) -> T) -> T {
        let mut doc = self.document.lock().unwrap();
        f(&mut doc)
    }

    pub fn document_json(&self) -> Value {
        let doc = self.document.lock().unwrap();
        serde_json::to_value(&*doc).unwrap_or(Value::Null)
    }

    pub fn revision(&self) -> u64 {
        self.document.lock().unwrap().revision
    }

    /// 序列化成 .aip v2 文本（中间文件格式不变）。
    pub fn aip_text(&self) -> Result<String, String> {
        let doc = self.document.lock().unwrap();
        pixel_core::context::to_aip(&doc)
    }

    pub fn interrupt(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        // 有审批挂着的话，发送端一掉，等待中的主循环立刻收手，不会和用户赌手感。
        self.approval.lock().unwrap().take();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// 挂起一条审批并等待用户决定。等待期间照样本轮询取消标志，
    /// 所以中断不会把这一笔调用彻底卡死。Err(()) = 没人再会给这一笔发决定。
    async fn await_approval(
        &self,
        tx: &UnboundedSender<AgentEvent>,
        call: &PlannedCall,
    ) -> Result<ApprovalDecision, ()> {
        let (sender, mut receiver) = oneshot::channel();
        {
            // 上一轮的挂起没清掉就顶掉：turn 顺序执行，出现即异常，顶掉保证不死等。
            let mut slot = self.approval.lock().unwrap();
            *slot = Some(ApprovalSlot {
                call_id: call.id.clone(),
                tx: sender,
            });
        }
        emit(
            tx,
            AgentEvent::ApprovalRequest {
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call.input.clone(),
            },
        );
        let mut tick = tokio::time::interval(Duration::from_millis(120));
        loop {
            tokio::select! {
                decision = &mut receiver => return decision.map_err(|_| ()),
                _ = tick.tick() => {
                    if self.is_cancelled() {
                        return Err(());
                    }
                }
            }
        }
    }

    /// 跑一个 turn：发一条用户消息，驱动模型通过工具改画布，直到它不再调工具。
    /// `images` 为 `(media_type, base64)` 对；所有过程事件经 `tx` 广播给 UI。
    pub async fn run_turn(
        &self,
        text: String,
        attachments: Vec<Attachment>,
        tx: UnboundedSender<AgentEvent>,
    ) {
        self.cancelled.store(false, Ordering::SeqCst);

        if text.trim().is_empty() && attachments.is_empty() {
            emit(
                &tx,
                AgentEvent::Error {
                    message: UiText::new("agent.empty_message", "empty message"),
                },
            );
            return;
        }

        let mut content: Vec<ContentBlock> = Vec::new();
        if !text.trim().is_empty() {
            content.push(ContentBlock::Text { text: text.clone() });
        }
        // 分流先走：Rust 按用户原话把参照定性、成品意图、风格预设、知识条目一次定完，
        // 存进会话状态。模型读原话觉得判错了再调 pixel_plan 纠正，两段接力。少了前一段，
        // 「光思考不干活」的模型就根本不会把约束带上路——而那几条一漏，「照这个画风」
        // 就变成了复刻，「瓦片」就变成了一张大地图。
        let plan = TurnPlan::from_text(&text, &attachments);
        *self.plan.lock().unwrap() = plan.clone();
        *self.turn_text.lock().unwrap() = text.clone();
        self.emit_plan_node(&tx, &attachments, &plan);

        // 图片清单走在图片前面：模型必须知道每张图的身份与顺序，
        // 否则「快照只是上下文、参考图才是真值」这条约束无从执行。
        if !attachments.is_empty() {
            content.push(ContentBlock::Text {
                // 每张参考图都带上它的参照方式和约束原文。定性当场生效，模型不必
                // 再猜「这算哪种参照」——猜错的方向是彻底相反的两种画法。
                text: Attachment::caption(&attachments, &plan.reference_modes),
            });
        }
        for attachment in attachments {
            content.push(ContentBlock::Image {
                media_type: attachment.media_type,
                data_base64: attachment.data_base64,
            });
        }
        self.messages.lock().unwrap().push(Message {
            role: Role::User,
            content,
        });

        // 用户发消息前在编辑器里动过的痕迹，先冲刷成一条 user 消息。
        // 少了这一步，模型会照着自己上一轮的想象继续画，把用户的手笔当成不存在。
        let manual_edits = self.take_edits();
        if !manual_edits.is_empty() {
            self.messages.lock().unwrap().push(Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: manual_edit_digest(&manual_edits),
                }],
            });
        }

        // 进入 turn 时快照一份预算，避免中途改设置导致行为漂移。
        // mut：Ask 模式下用户点「本次放行」会把它降级成 Auto，只影响本 turn。
        let mut runner_config = self.runner_config();
        // 这句话是不是在要图。纯聊天不必催，见 asks_for_artwork。
        let art_requested = asks_for_artwork(&text);
        let mut steps = 0usize;
        let mut last_failure: Option<(String, String)> = None;
        let mut failure_streak = 0usize;
        let mut usage_in: Option<u32> = None;
        let mut usage_out: Option<u32> = None;

        // 三个配额：逻辑轮次吃 max_turns 预算，续写和重试各自封顶五次。
        let mut logical_rounds = 0usize;
        let mut continuations = 0usize;
        let mut retries = 0usize;
        // 整轮零工具调用时催过几次。见 MAX_TOOL_NUDGES。
        let mut tool_nudges = 0usize;

        // 关思考：用户钉死（Some）就照办；没钉死（None）先按模型默认来，
        // 但保留一次自动翻盘的机会——见下面 retry 小循环里的兜底。
        let pinned_thinking = {
            let engine = self.engine.lock().unwrap();
            engine.config.disable_thinking
        };
        let mut thinking_off = pinned_thinking.unwrap_or(false);
        let mut thinking_off_tried = pinned_thinking.is_some();

        loop {
            if self.is_cancelled() {
                emit(&tx, AgentEvent::Interrupted);
                return;
            }

            // 只有「工具跑完再来一问」算一个逻辑轮次；同一句话说一半被掐断后
            // 接着写不算——那本来就是这一轮的尾巴，不该吃掉用户的续轮预算。
            if logical_rounds >= runner_config.max_turns {
                emit(
                    &tx,
                    AgentEvent::Error {
                        message: UiText::new(
                            "agent.turn_budget",
                            "this turn ran out of rounds after {rounds} edit(s); start a new message to keep going",
                        )
                        .with("rounds", runner_config.max_turns as u64),
                    },
                );
                return;
            }
            logical_rounds += 1;

            let provider = self.engine.lock().unwrap().provider.clone();
            // 本逻辑轮已经写下的正文。续写指令要把断点原文按在模型眼前，
            // 光说一句「继续」，它会当新问题重想一遍、把开头再念一次。
            let mut carried = String::new();
            let mut stalled = false;
            // provider 直说过「最多给这么多」时记住它，本逻辑轮内都照这个来。
            let mut token_ceiling: Option<u32> = None;
            // 续写时对照的「已经写到哪里」。第一发没有旧内容可对照，掐断之后才建起来，
            // 之后每一发都带着它走，跨轮活着。
            let mut echo: Option<EchoTrim> = None;
            // provider 报的 stop reason 明明是「说完了」，结尾却断在半截。
            let mut inferred_cut = false;
            // 连续几轮只吐推理、没动笔。护栏里的硬闸，理由见 MAX_REASONING_CONTINUATIONS。
            let mut thinking_only = 0usize;
            // 重试与续写都收在这个小循环里。请求每一发都重装：历史里刚 push 的
            // 半截回复必须在这发请求里生效，不然就是白续一次。
            let mut raw: RoundRaw = loop {
                // 退避睡到一半用户点了中断：别等睡醒再开口，直接收。
                if self.is_cancelled() {
                    emit(&tx, AgentEvent::Interrupted);
                    return;
                }
                let request = self.chat_request(&runner_config, token_ceiling, thinking_off);

                let stream = match provider.request(&request).await {
                    Ok(stream) => stream,
                    Err(e) => {
                        // provider 说「max_tokens 太大了，最多 N」：降下来重发，
                        // 别让整轮请求因为一个猜大的数字跑黄。下一次这个循环迭代
                        // 就会带上新上限，输出短了由续写拼完整。
                        if let Some(cap) = providers::token_cap_from_error(&e)
                            .filter(|cap| token_ceiling.is_none_or(|known| *cap < known))
                        {
                            token_ceiling = Some(cap);
                            emit(
                                &tx,
                                AgentEvent::Status {
                                    message: lowering_status(cap),
                                },
                            );
                            continue;
                        }
                        // 发不出去：网络抖动、代理掐线、base_url 填错。比起把一句
                        // 「失败」摔在用户脸上，按退避重发更有人味。
                        if retries < runner_config.loop_limits.max_retries && retryable(&e) {
                            retries += 1;
                            emit(
                                &tx,
                                AgentEvent::Status {
                                    message: retrying_status(
                                        &e.to_string(),
                                        retries,
                                        runner_config.loop_limits.max_retries,
                                    ),
                                },
                            );
                            pause_before_retry(retries).await;
                            continue;
                        }
                        emit(
                            &tx,
                            AgentEvent::Error {
                                message: request_failed(&e.to_string()),
                            },
                        );
                        return;
                    }
                };

                // 这一发之前已经吃掉过多少复读。整段都在照搬的话，收下来一个字
                // 不剩，跟「模型什么都没吐」长得一模一样——比一下才知道是谁。
                let eaten_before = echo.as_ref().map_or(0, |t| t.eaten);
                let mut raw = self
                    .consume_stream(stream, &tx, &mut usage_in, &mut usage_out, echo.as_mut())
                    .await;
                let ate_now = echo.as_ref().is_some_and(|t| t.eaten > eaten_before);

                if raw.cancelled {
                    emit(&tx, AgentEvent::Interrupted);
                    return;
                }
                if let Some(failure) = raw.failure.take() {
                    if retries < runner_config.loop_limits.max_retries && raw.retryable_failure {
                        retries += 1;
                        emit(
                            &tx,
                            AgentEvent::Status {
                                message: retrying_status(
                                    &failure,
                                    retries,
                                    runner_config.loop_limits.max_retries,
                                ),
                            },
                        );
                        pause_before_retry(retries).await;
                        continue;
                    }
                    emit(
                        &tx,
                        AgentEvent::Error {
                            message: request_failed(&failure),
                        },
                    );
                    return;
                }
                retries = 0;
                // 这一轮确实收到了东西，失败额度重新算：下次失误仍该有五次机会。

                // 模型拿着工具，却把一轮额度全烧在思考上，正事一件没干。这类模型
                // （LongCat、DeepSeek-R1 一系）默认就爱想，得让它先把嘴闭上：
                // 关掉思考重问一次，它立刻就调工具去了。整个 turn 只翻一次盘，
                // 翻完还不行就照旧走续写那条路，别再翻第二次。
                // 只认「正文一个字没有」：正文被掐断是另一种病，走续写，别抢。
                // 平台不报截断的就认长度：没正文、没工具、却写了一长篇推理，
                // 也是同一副药。阈值放在正常思考的长度之上，免得误伤真想清楚了
                // 只是话少的模型。
                if burned_down_to_reasoning(&raw)
                    && !request.tools.is_empty()
                    && !thinking_off_tried
                {
                    thinking_off_tried = true;
                    thinking_off = true;
                    // 翻盘成功要把结论留住。只翻本 turn 内的一个局部变量的话，
                    // 用户发下一句话时又得先烧掉一整轮预算，看同一句「已关掉思考」
                    // 再看同一段光想不干的推理——那是纯浪费。
                    // 只写本次运行内的会话状态，不动 models.json 里的模型定义：
                    // 那是用户手改的东西，不该被一次兜底悄悄改写。
                    let mut engine = self.engine.lock().unwrap();
                    if engine.config.disable_thinking.is_none() {
                        engine.config.disable_thinking = Some(true);
                    }
                    drop(engine);
                    emit(
                        &tx,
                        AgentEvent::Status {
                            message: thinking_off_status(),
                        },
                    );
                    continue;
                }

                // 有的平台撞了上限也不吭声。文字断在半截就当它掐了：续写会把
                // 剩下半截接上。猜错也不要紧，见收场处的处理。
                if raw.stop == StopKind::Finished
                    && !raw.text.is_empty()
                    && looks_cut_off(&raw.text)
                {
                    raw.stop = StopKind::Truncated;
                    inferred_cut = true;
                }
                // 续写那一发整段都在照搬：复读筛咽干净之后 `resumable()` 直接是假，
                // 看着像「模型什么都没产出」。不是那么回事——它把写过的又念了
                // 一遍。按卡死收场，报错才说得清到底断在哪。
                if raw.stop == StopKind::Truncated && !raw.resumable() && ate_now {
                    stalled = true;
                }
                // 话没说完就接着问。前半截已经逐字推给前端了，续写只是继续追加，
                // 用户看到的是一段完整输出，而不是半句摆在屏幕上。
                if raw.stop == StopKind::Truncated && raw.resumable() {
                    if continuations >= runner_config.loop_limits.max_continuations {
                        break raw;
                    }
                    // 这一轮写下的全部内容：推理算，正文也算。只认正文的话，
                    // 「一整轮都耗在思考上、一个字没吐」会被当成没推进，
                    // 而它恰恰是最需要续写的那一种。
                    let fresh = format!("{}{}", raw.reasoning, raw.text);
                    if !continuation_progressed(&carried, &fresh) {
                        stalled = true;
                        break raw;
                    }
                    // 只吐推理没动笔：正文一个字没有，工具调用一个没发。这种续写
                    // 要得着，但见好就收——再要下去它只会把下一轮预算也全烧在
                    // 思考上，用户盯着的是一个永远不画画的思考节点。
                    if raw.text.is_empty() && raw.accumulator.is_empty() {
                        thinking_only += 1;
                        if thinking_only > runner_config.loop_limits.max_reasoning_continuations {
                            stalled = true;
                            break raw;
                        }
                    } else {
                        thinking_only = 0;
                    }
                    carried.push_str(&fresh);
                    continuations += 1;
                    // 把这一轮写下的内容并进对照窗口，下一发续写就照着最新的比。
                    // 第一次掐断时窗口就是这半截回复本身。
                    echo = Some(match echo {
                        Some(mut trim) => {
                            trim.absorb(&raw.reasoning, &raw.text);
                            trim
                        }
                        None => EchoTrim::new(&raw.reasoning, &raw.text),
                    });
                    let repeated = echo.as_ref().is_some_and(EchoTrim::ate_something);
                    self.push_resume(&raw, &continue_nudge(&carried, repeated));
                    emit(
                        &tx,
                        AgentEvent::Status {
                            message: continuing_status(
                                continuations,
                                runner_config.loop_limits.max_continuations,
                            ),
                        },
                    );
                    continue;
                }
                break raw;
            };

            // 没写出个结果就收场了：如实告诉用户断在哪、为什么断，
            // 别让一段半截 Lua 看起来像是画完了。
            if raw.stop == StopKind::Truncated {
                // 猜错了的续写：provider 说「说完了」，只是结尾长得像断在半截。
                // 续写发出去模型只会答「没什么要补充的」——那就照原样收场，
                // 别拿一句报错把好好一条回复判成失败。
                let wrong_guess = stalled && inferred_cut;
                if !wrong_guess {
                    let message = if thinking_only > 0 {
                        // 无限思考：钱全烧在推理上，画一笔都没动。得把出路说清楚，
                        // 不然用户只会反复重试同一条消息。
                        stalled_thinking_message(thinking_only)
                    } else if stalled {
                        // 「原样再写一遍」和「几乎没吐新东西」都走这一条，
                        // 话说得太死会冤枉了后者。
                        UiText::new(
                            "agent.stalled",
                            "the reply stopped making progress: the model either repeated text it had already written or produced almost nothing new, so the run stopped at the point shown above",
                        )
                    } else if raw.resumable() {
                        UiText::new(
                            "agent.output_limit",
                            "the reply still hit the output limit after {done} continuation(s); raise Max tokens in model settings and resend your request",
                        )
                        .with("done", continuations as u64)
                    } else {
                        UiText::new(
                            "agent.output_limit_empty",
                            "the model produced nothing before the output limit; raise Max tokens in model settings",
                        )
                    };
                    emit(&tx, AgentEvent::Error { message });
                    return;
                }
                self.undo_resume();
                raw = RoundRaw::empty();
                emit(
                    &tx,
                    AgentEvent::Status {
                        message: finished_whole_status(),
                    },
                );
            } else if inferred_cut {
                // 续写回来了，provider 说这一轮也说完了。它到底补上了东西没有？
                // 没补上就说明模型自己都觉得前面已经写完了：撤掉那句没被答复的
                // 追问，历史停在那条完整回复上，别把「没什么要补充的」当正文。
                let fresh = format!("{}{}", raw.reasoning, raw.text);
                if !continuation_progressed(&carried, &fresh) {
                    self.undo_resume();
                    raw = RoundRaw::empty();
                    emit(
                        &tx,
                        AgentEvent::Status {
                            message: finished_whole_status(),
                        },
                    );
                }
            }

            let mut blocks = raw.blocks();

            let mut calls: Vec<PlannedCall> = Vec::new();
            for (_, (id, name, partial)) in raw.accumulator {
                let trimmed = partial.trim();
                let (input, parse_error) = if trimmed.is_empty() {
                    (json!({}), None)
                } else {
                    match serde_json::from_str::<Value>(trimmed) {
                        Ok(value) => (value, None),
                        Err(e) => (
                            json!({ "_raw": partial, "_error": e.to_string() }),
                            Some(e.to_string()),
                        ),
                    }
                };
                emit(
                    &tx,
                    AgentEvent::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                );
                calls.push(PlannedCall {
                    id,
                    name,
                    input,
                    parse_error,
                });
            }
            for call in &calls {
                blocks.push(ContentBlock::ToolUse {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                });
            }
            if !blocks.is_empty() {
                self.messages
                    .lock()
                    .unwrap()
                    .push(Message::assistant(blocks));
            }

            // 没有工具调用。这一轮多半就是最终答复，但有一种例外必须当场拦住：
            // 模型整轮一笔没画，光说了几句就宣布「已完成」。那是纯空转——用户
            // 拿到一句汇报，画布上却什么也没多。只在「本 turn 一个工具都没跑过」
            // 时才催，免得把正常的收尾提问也当成没干活。
            if calls.is_empty() {
                if steps == 0
                    && tool_nudges < MAX_TOOL_NUDGES
                    && art_requested
                    && !looks_like_a_question(&raw.text)
                {
                    tool_nudges += 1;
                    self.messages
                        .lock()
                        .unwrap()
                        .push(Message::user_text(tool_nudge()));
                    emit(
                        &tx,
                        AgentEvent::Status {
                            message: nudging_tools_status(),
                        },
                    );
                    continue;
                }
                emit(
                    &tx,
                    AgentEvent::Usage {
                        input_tokens: usage_in,
                        output_tokens: usage_out,
                    },
                );
                emit(
                    &tx,
                    AgentEvent::Completed {
                        turns: logical_rounds,
                    },
                );
                return;
            }

            for call in calls {
                if self.is_cancelled() {
                    emit(&tx, AgentEvent::Interrupted);
                    return;
                }
                if steps >= runner_config.max_tool_steps {
                    emit(
                        &tx,
                        AgentEvent::Error {
                            message: UiText::new(
                                "agent.tool_budget",
                                "this turn ran out of tool steps after {steps} step(s); start a new message to keep going",
                            )
                            .with("steps", steps as u64),
                        },
                    );
                    return;
                }
                steps += 1;

                // 权限门：放行之后才动文档。否掉不当失败调用（不进退避计数），
                // 喂一条 tool_result 让模型换方向，对话继续。
                if needs_approval(runner_config.permission, &call.name) {
                    match self.await_approval(&tx, &call).await {
                        Ok(ApprovalDecision::Approve) => {}
                        Ok(ApprovalDecision::ApproveAll) => {
                            // 「这次别烦了」只降级本 turn；会话配置不动。
                            runner_config.permission = PermissionMode::Auto;
                        }
                        Ok(ApprovalDecision::Reject) => {
                            emit(
                                &tx,
                                AgentEvent::ToolResult {
                                    id: call.id.clone(),
                                    name: call.name.clone(),
                                    summary: "rejected by user".into(),
                                    is_error: false,
                                },
                            );
                            self.messages.lock().unwrap().push(Message::tool_result(
                                call.id.clone(),
                                "the user rejected this tool call. Do not retry it as-is; explain what you were about to do and ask how you should proceed.",
                                false,
                            ));
                            continue;
                        }
                        // 没人再会给这一笔发决定（中断，或挂起被新 turn 顶掉）。
                        Err(()) => {
                            emit(&tx, AgentEvent::Interrupted);
                            return;
                        }
                    }
                }

                let outcome: ToolOutcome = match &call.parse_error {
                    Some(e) => ToolOutcome {
                        content: format!(
                            "tool call rejected: arguments were not valid JSON ({e}). Resend the same tool call with corrected JSON arguments."
                        ),
                        is_error: true,
                    },
                    None => {
                        let registry = self.mcp.lock().unwrap().clone();
                        if call.name.starts_with(mcp::MCP_TOOL_PREFIX) {
                            // 外部工具：分流到用户自配的 MCP 服务器，不进文档锁。
                            match registry {
                                Some(registry) => {
                                    match registry.dispatch(&call.name, &call.input).await {
                                        Ok(outcome) => outcome,
                                        Err(e) => ToolOutcome {
                                            content: e,
                                            is_error: true,
                                        },
                                    }
                                }
                                None => ToolOutcome {
                                    content: format!(
                                        "no MCP registry is attached to this session, so {call} cannot run",
                                        call = call.name
                                    ),
                                    is_error: true,
                                },
                            }
                        } else if call.name == PLAN_TOOL {
                            // 只改会话里的分流表，不碰文档、不等模型。按 sync 写
                            // 就够，签名跟着其余分支保持 async 是为了分流链一致。
                            self.run_plan(&call.input).await
                        } else if call.name == tools::IMAGE_GEN_TOOL {
                            // 生图要等模型回图，异步跑；await 期间绝不持有文档锁。
                            self.run_image_gen(&call.input).await
                        } else {
                            let mut doc = self.document.lock().unwrap();
                            let active = self.active.lock().unwrap();
                            tools::execute(&mut doc, &active, &call.name, &call.input)
                        }
                    }
                };

                emit(
                    &tx,
                    AgentEvent::ToolResult {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        summary: summarize(&outcome.content),
                        is_error: outcome.is_error,
                    },
                );

                let mut content = outcome.content;
                let budget = runner_config.max_tool_result_bytes;
                if content.chars().count() > budget {
                    let head: String = content.chars().take(budget).collect();
                    content = format!("{head}\n...[truncated to {budget} chars]");
                }
                self.messages.lock().unwrap().push(Message::tool_result(
                    call.id,
                    content,
                    outcome.is_error,
                ));

                if outcome.is_error {
                    let key = (call.name.clone(), call.input.to_string());
                    if last_failure.as_ref() == Some(&key) {
                        failure_streak += 1;
                    } else {
                        last_failure = Some(key);
                        failure_streak = 1;
                    }
                    if failure_streak >= 3 {
                        emit(
                            &tx,
                            AgentEvent::Error {
                                message: UiText::new(
                                    "agent.same_call_failed",
                                    "the same tool call failed three times in a row; stopping so you can adjust the request",
                                ),
                            },
                        );
                        return;
                    }
                } else {
                    last_failure = None;
                    failure_streak = 0;
                }

                // 读操作不改文档，不推 DocumentUpdated。
                // MCP 工具在文档之外跑（不回推文档事件）；读操作本来也不推。
                if call.name != "pixel_read_canvas" && !call.name.starts_with(mcp::MCP_TOOL_PREFIX)
                {
                    let (revision, document) = {
                        let doc = self.document.lock().unwrap();
                        (
                            doc.revision,
                            serde_json::to_value(&*doc).unwrap_or(Value::Null),
                        )
                    };
                    emit(&tx, AgentEvent::DocumentUpdated { revision, document });
                }
            }
        }
    }

    /// 组装一轮请求。抽出来是因为重试和续写都要重新发一次请求：
    /// 历史每条消息都锁一次会碎，合成一次才看得出「这一轮到底发了什么」。
    /// 锁顺序固定为 document -> active -> messages，全程一致，不会自锁。
    fn chat_request(
        &self,
        cfg: &RunnerConfig,
        ceiling: Option<u32>,
        disable_thinking: bool,
    ) -> ChatRequest {
        let doc = self.document.lock().unwrap();
        let active = self.active.lock().unwrap();
        let engine = self.engine.lock().unwrap();
        // 本轮分流和知识条目按当前 plan 和原话现算：模型中途纠正过，
        // 下一发请求就该带着新结论上路，缓存会把纠正吃掉。
        // 锁顺序固定 document -> active -> engine -> plan -> turn_text。
        let (routing, craft_notes) = {
            let plan = self.plan.lock().unwrap();
            let text = self.turn_text.lock().unwrap();
            (
                plan.prompt_sections(),
                knowledge::prompt_section(
                    &text,
                    knowledge::DEFAULT_LIMIT,
                    knowledge::DEFAULT_BUDGET,
                ),
            )
        };
        ChatRequest {
            system: prompt::build_system_prompt(
                &doc,
                &active.layer,
                &active.frame,
                active.color.as_deref(),
                cfg.canvas_context_chars,
                &routing,
                &craft_notes,
            ),
            messages: self.messages.lock().unwrap().clone(),
            // MCP 工具追加在内建 pixel_* 之后：模型每轮看到的都是当前真实能力。
            tools: {
                let mut specs = tools::specs();
                specs.extend(self.mcp_specs());
                specs
            },
            // 三层取最小：用户填过的值最该被尊重，没填过就按模型名查表，
            // 而 provider 报过的上限是硬事实，谁小听谁。
            max_tokens: resolve_max_tokens(engine.config.max_tokens, &engine.config.model, ceiling),
            temperature: engine.config.temperature,
            disable_thinking,
            // 推理模型要拿回上一轮的思考，不然续写时它会当新问题重想一遍。
            echo_reasoning: providers::echoes_reasoning(&engine.config.model),
        }
    }

    /// 把流喝干。失败/取消/掐断都不在这里收尾，只写进 `RoundRaw`，
    /// 交给调用方决定是重发还是续写——这样这个函数里没有任何 `return` 分支逃逸。
    ///
    /// `echo` 只在续写那几发里带着：模型把刚写完的原样再念一遍时，念出来的字
    /// 在这里就被吃掉，不会流到界面上让用户看两遍。第一发没有可对照的旧内容。
    async fn consume_stream(
        &self,
        mut stream: providers::EventStream,
        tx: &UnboundedSender<AgentEvent>,
        usage_in: &mut Option<u32>,
        usage_out: &mut Option<u32>,
        mut echo: Option<&mut EchoTrim>,
    ) -> RoundRaw {
        let mut raw = RoundRaw::empty();
        let mut tick = tokio::time::interval(Duration::from_millis(120));
        let mut idle = IdleWatch::new(STREAM_IDLE_LIMIT);

        loop {
            tokio::select! {
                item = stream.next() => {
                    // 收到什么都算活着：思考越久越要刷新，免得把慢模型误判成假死。
                    idle.touch();
                    match item {
                        Some(Ok(event)) => match event {
                            LlmEvent::Token(t) => {
                                let fresh = match echo.as_mut() {
                                    Some(trim) => trim.feed_text(&t),
                                    None => t,
                                };
                                if !fresh.is_empty() {
                                    raw.text.push_str(&fresh);
                                    emit(tx, AgentEvent::Token { text: fresh });
                                }
                            }
                            LlmEvent::Reasoning(t) => {
                                let fresh = match echo.as_mut() {
                                    Some(trim) => trim.feed_reasoning(&t),
                                    None => t,
                                };
                                if !fresh.is_empty() {
                                    raw.reasoning.push_str(&fresh);
                                    emit(tx, AgentEvent::Reasoning { text: fresh });
                                }
                            }
                            LlmEvent::ToolUseStart { index, id, name } => {
                                raw.accumulator.insert(index, (id, name, String::new()));
                            }
                            LlmEvent::ToolInputDelta { index, json_partial } => {
                                if let Some(slot) = raw.accumulator.get_mut(&index) {
                                    slot.2.push_str(&json_partial);
                                }
                            }
                            LlmEvent::Usage { input_tokens, output_tokens } => {
                                if input_tokens.is_some() {
                                    *usage_in = input_tokens;
                                }
                                if output_tokens.is_some() {
                                    *usage_out = output_tokens;
                                }
                            }
                            // stop reason 是关键：max_tokens / length 说明被掐断，不是说完。
                            LlmEvent::Done { stop_reason } => {
                                raw.stop = stop_kind(&stop_reason);
                                break;
                            }
                        },
                        Some(Err(e)) => {
                            raw.failure = Some(format!("stream error: {e}"));
                            raw.retryable_failure = retryable(&e);
                            break;
                        }
                        None => break,
                    }
                }
                _ = tick.tick() => {
                    if self.is_cancelled() {
                        drop(stream);
                        raw.cancelled = true;
                        break;
                    }
                    if idle.expired() {
                        drop(stream);
                        raw.failure = Some(format!(
                            "no data from the model for {}s, the stream looks stalled",
                            STREAM_IDLE_LIMIT.as_secs()
                        ));
                        // 假死多半是代理在掐连接，原样再发一次常常就通了。
                        raw.retryable_failure = true;
                        break;
                    }
                }
            }
        }
        // 还按住的那截到此收尾。够长的照搬：模型念到刚写完的地方就没词了，
        // 那些字前面已经写过了，咽下去不显。不够长的还回去——流到这儿
        // 新内容还没来齐，咽了就是真删了用户要看的东西。
        if let Some(trim) = echo.as_mut() {
            let (fresh_text, fresh_reasoning) = trim.seal();
            if !fresh_text.is_empty() {
                raw.text.push_str(&fresh_text);
                emit(tx, AgentEvent::Token { text: fresh_text });
            }
            if !fresh_reasoning.is_empty() {
                raw.reasoning.push_str(&fresh_reasoning);
                emit(
                    tx,
                    AgentEvent::Reasoning {
                        text: fresh_reasoning,
                    },
                );
            }
        }
        raw
    }

    /// 续写：把半截回复原样写进历史，再塞一句「接着写」。
    /// 分两次 push 中间不放开锁——否则换个线程插一条消息，顺序就乱了。
    fn push_resume(&self, raw: &RoundRaw, nudge: &str) {
        // 推理块在过网前会被丢掉，只有推理的半截回复推平等于推一条空
        // content，provider 会当场拒掉整个请求。宁可只推指令那一条。
        if raw.text.is_empty() {
            self.messages
                .lock()
                .unwrap()
                .push(Message::user_text(nudge));
            return;
        }
        let mut messages = self.messages.lock().unwrap();
        messages.push(Message::assistant(raw.blocks()));
        messages.push(Message::user_text(nudge));
    }

    /// 撤掉最后那条「继续」指令。
    ///
    /// 猜错了的续写才用得上：模型已经没什么要补充的，那句没被答复的追问留在
    /// 历史里，下一轮只会看到一句悬空的 user 消息。`push_resume` 永远以这条
    /// 指令收尾，所以退一条就够。
    fn undo_resume(&self) {
        self.messages.lock().unwrap().pop();
    }

    /// agent 生图工具：让模型直接产出位图，再量化落到画布。与同步工具分开跑，
    /// 因为它要等模型回图，期间绝不能占着文档锁。锁顺序仍是 document -> active。
    /// 本轮分流节点：一进 turn 就摆出去，让用户当场看见「这句话被理解成了什么」，
    /// 而不是等模型画完才发现一整张图被复刻、或瓦片被画成了一张地图。
    /// ToolCall / ToolResult 成对发，和模型自己调工具在对话里长得一模一样；
    /// 但它不进 messages——它只是把已经写进提示词的那几条约束显式地说一遍。
    fn emit_plan_node(
        &self,
        tx: &UnboundedSender<AgentEvent>,
        _attachments: &[Attachment],
        plan: &TurnPlan,
    ) {
        // 一件都没定出来就不发：聊天气泡里凭空多一张跟画面无关的卡片，只是噪声。
        if !plan.has_references()
            && plan.intent.is_none()
            && plan.style.is_none()
            && plan.knowledge_ids.is_empty()
        {
            return;
        }
        let id = format!(
            "turn-plan-{}",
            self.ref_nodes.fetch_add(1, Ordering::SeqCst) + 1
        );
        let input = plan.node_input();
        let summary = summarize(&plan.outcome_text());
        emit(
            tx,
            AgentEvent::ToolCall {
                id: id.clone(),
                name: PLAN_TOOL.into(),
                input,
            },
        );
        emit(
            tx,
            AgentEvent::ToolResult {
                id,
                name: PLAN_TOOL.into(),
                summary,
                is_error: false,
            },
        );
    }

    /// 模型纠正本轮分流。只改会话里的 plan：出图清单和提示词的 TURN ROUTING 段
    /// 都由它推导，所以纠正对下一发请求立刻生效，不必等用户再发一句话。
    async fn run_plan(&self, input: &Value) -> ToolOutcome {
        let updates = match plan::parse_updates(input) {
            Ok(updates) => updates,
            Err(e) => {
                return ToolOutcome {
                    content: e,
                    is_error: true,
                }
            }
        };
        let mut plan = self.plan.lock().unwrap();
        // 只认这一轮清单里真有的参考图。清单之外的下标说明模型在自说自话：照改
        // 会让定性表和清单错开位，之后每张图的约束全串到另一张上去。
        // 先全验、再全改。半途报错会把前面几步的改动留在表里，模型收到错误却
        // 不知道分流已经动过，下一轮的清单会带着它没同意的约束走。
        for update in &updates.references {
            let known = update.index >= 1
                && matches!(
                    plan.reference_modes.get(update.index - 1),
                    Some(slot) if slot.is_some()
                );
            if !known {
                return ToolOutcome {
                    content: format!(
                        "{}: image {} is not an attached reference image; correct only the \
                         1-based numbers in the attachment list",
                        PLAN_TOOL, update.index
                    ),
                    is_error: true,
                };
            }
        }
        let mut changed = Vec::new();
        for update in updates.references {
            let slot = plan.reference_modes.get_mut(update.index - 1).unwrap();
            if *slot != Some(update.mode) {
                *slot = Some(update.mode);
                changed.push(format!(
                    "image {} is now {}",
                    update.index,
                    update.mode.as_str()
                ));
            }
        }
        if let Some(intent) = updates.intent {
            match intent {
                Some(intent) => {
                    if plan.intent != Some(intent) {
                        plan.intent = Some(intent);
                        changed.push(format!("the deliverable is now {}", intent.id()));
                    }
                }
                None => {
                    if plan.intent.take().is_some() {
                        changed.push("the deliverable is no longer pinned".to_string());
                    }
                }
            }
        }
        if let Some(style) = updates.style {
            match style {
                Some(style) => {
                    if plan.style != Some(style) {
                        plan.style = Some(style);
                        changed.push(format!("the art style is now {}", style.id()));
                    }
                }
                None => {
                    if plan.style.take().is_some() {
                        changed.push("the art style is no longer pinned".to_string());
                    }
                }
            }
        }
        if changed.is_empty() {
            return ToolOutcome {
                content: format!(
                    "{PLAN_TOOL}: nothing to correct - the TURN ROUTING section of your system \
                     prompt already carries exactly this state, so restating it changed nothing. \
                     Do not re-send routing; go straight to the drawing tool."
                ),
                is_error: true,
            };
        }
        ToolOutcome {
            content: format!(
                "{}; the next prompt restates the routing for this turn.",
                changed.join("; ")
            ),
            is_error: false,
        }
    }

    async fn run_image_gen(&self, input: &Value) -> ToolOutcome {
        let params = match tools::ImageGenToolParams::parse(input) {
            Ok(p) => p,
            Err(e) => {
                return ToolOutcome {
                    content: e,
                    is_error: true,
                }
            }
        };
        let config = self.model_config();
        if !config.capabilities.image_gen {
            return ToolOutcome {
                content: "this model is not able to generate images; enable image generation in model settings and point it at an OpenAI-compatible image model".into(),
                is_error: true,
            };
        }
        // 垫图先在读锁里渲染；await 之前必须放掉文档锁。
        let reference = match params.reference_frame.as_deref() {
            Some(frame) => match self.with_document(|doc| imagegen::frame_reference(doc, frame)) {
                Ok(att) => Some(att),
                Err(e) => {
                    return ToolOutcome {
                        content: format!("{IMAGE_GEN_TOOL}: {e}"),
                        is_error: true,
                    }
                }
            },
            None => None,
        };
        let generator = imagegen::build_image_generator(&config);
        let request = ImageGenParams {
            prompt: params.prompt.clone(),
            size: params.size.clone(),
            reference,
        };
        let image = match generator.generate(&request).await {
            Ok(img) => img,
            Err(e) => {
                return ToolOutcome {
                    content: format!("{IMAGE_GEN_TOOL}: image generation failed: {e}"),
                    is_error: true,
                }
            }
        };
        let (rgba, width, height) = match decode::decode_image(&image.bytes, &image.media_type) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutcome {
                    content: format!("{IMAGE_GEN_TOOL}: could not decode the model's image: {e}"),
                    is_error: true,
                }
            }
        };
        let active = self.active();
        let landed = self.with_document_mut(|doc| {
            tools::land_generated(doc, &active, &params, &rgba, width, height)
        });
        let landed = match landed {
            Ok(l) => l,
            Err(e) => {
                return ToolOutcome {
                    content: format!("{IMAGE_GEN_TOOL}: could not land the image: {e}"),
                    is_error: true,
                }
            }
        };
        // 新建帧时把激活帧挪过去：下一轮工具该接着这一帧画，用户看到的也对得上。
        if params.spot == LandSpot::NewFrame {
            let mut next = active;
            next.frame = landed.frame.clone();
            self.set_active(next);
        }
        let mut content = format!(
            "generated a {w}x{h} image ({transport}) and landed it on layer {layer} frame {frame}: {colors} color(s) used, +{added} palette color(s).\n",
            w = width,
            h = height,
            transport = image.transport,
            layer = landed.layer,
            frame = landed.frame,
            colors = landed.report.colors_used,
            added = landed.report.palette_added,
        );
        if !image.note.trim().is_empty() {
            content.push_str(&format!("model note: {}\n", image.note.trim()));
        }
        let grid = self.with_document(|doc| tools::active_grid(doc, &landed.layer, &landed.frame));
        content.push_str(&grid);
        ToolOutcome {
            content,
            is_error: false,
        }
    }
}

fn doc_has_layer(doc: &Document, id: &str) -> bool {
    doc.layers.iter().any(|l| l.id == id)
}

fn doc_has_frame(doc: &Document, id: &str) -> bool {
    doc.frames.iter().any(|f| f.id == id)
}

/// 这次调用要不要审批。Auto 全放行；Ask 每个调用都问；Chat 放行只读的读回
/// （读网格不改文档），写操作一律过问——用户要盯的是「模型在改我的画」。
fn needs_approval(mode: PermissionMode, name: &str) -> bool {
    match mode {
        PermissionMode::Auto => false,
        PermissionMode::Ask => true,
        // Chat 模式只放行「只读」和「改定性」两种：前者只是看画布，后者只是把
        // 上一行清单里的结论换个说法，都碰不到画面。弹审批卡反而逼着用户为一个
        // 文本决定反复点同意。
        PermissionMode::Chat => name != "pixel_read_canvas" && name != PLAN_TOOL,
    }
}

fn emit(tx: &UnboundedSender<AgentEvent>, event: AgentEvent) {
    let _ = tx.send(event);
}

/// 「正在接着写」的状态条。前端把它渲染成 notice 节点，让用户知道这不是卡死。
fn continuing_status(done: usize, max: usize) -> UiText {
    UiText::new(
        "agent.continuing",
        "the reply hit the output limit, continuing ({done} of {max})",
    )
    .with("done", done as u64)
    .with("max", max as u64)
}

/// 「这次没成，正在重试」的状态条。带上原因，用户才好判断是网的事还是模型的事。
fn retrying_status(reason: &str, attempt: usize, max: usize) -> UiText {
    // 报错常常裹着一整条 URL 或响应体，截一刀免得通知条把聊天区撑爆。
    let short: String = reason.chars().take(160).collect();
    let short = if reason.chars().count() > 160 {
        format!("{short}...")
    } else {
        short
    };
    UiText::new(
        "agent.retrying",
        "that request failed ({reason}); retrying {attempt} of {max}",
    )
    .with("reason", short)
    .with("attempt", attempt as u64)
    .with("max", max as u64)
}

/// 「provider 对输出上限有意见，我们按它说的数降下来了」的状态条。
///
/// 带上新数字：用户看到字段里填的还是自己那个值，会以为降级没生效。
fn lowering_status(cap: u32) -> UiText {
    UiText::new(
        "agent.lowering_tokens",
        "this model accepts at most {cap} output tokens; switched to that and the reply will be stitched together",
    )
    .with("cap", cap as u64)
}

/// 「光思考不干活，关掉思考再问一次」的状态条。
fn thinking_off_status() -> UiText {
    UiText::new(
        "agent.thinking_off_retry",
        "this model spent the whole budget thinking and called no tool; retrying with thinking turned off",
    )
}

/// 这一轮是不是「只说推理没干正事」：正文一个字没有、工具一个没调，只有推理。
///
/// 认两种：provider 报了的截断，和它没报、但推理长到把预算烧空了的。
/// 正文被掐断是另一种病，走续写那条路，不在这里抢。
fn burned_down_to_reasoning(raw: &RoundRaw) -> bool {
    if !raw.text.is_empty() || !raw.accumulator.is_empty() || raw.reasoning.is_empty() {
        return false;
    }
    raw.stop == StopKind::Truncated || raw.reasoning.chars().count() >= REASONING_ONLY_CHARS
}

/// 催「光说话不干活」的模型动手。这句是给模型看的，用英文写：提示词是英文。
fn tool_nudge() -> String {
    "You replied with text only and called no tool, so nothing on the canvas changed. Your turn is not finished: the available canvas tools include pixel_run_shader and pixel_apply_operations. Do the work now - if the request is about artwork, end this reply with the tool call that draws it. Reply with text alone only if you genuinely need one piece of information from the user before you can edit the canvas."
        .to_string()
}

/// 把「用户手动改了什么」拼成一条模型能读懂的消息。
///
/// 语气写成系统在转述用户动作，而不是用户本人在下指令——否则模型容易把这些
/// 当成新的需求逐条照办，而它真正要做的是「接着这个改过的画面继续」。
fn manual_edit_digest(edits: &[String]) -> String {
    let mut out = String::from(
        "The user hand-edited the canvas in the editor while composing this message. \
         The canvas state below already contains those edits, and they are now the ground truth: \
         continue from what is on the canvas, not from what you last drew. \
         What the user touched, most recent last:\n",
    );
    for edit in edits {
        out.push_str("- ");
        out.push_str(edit.trim());
        out.push('\n');
    }
    out.push_str(
        "Keep those parts unless the user asks to change them, and treat the current pixels as authoritative.",
    );
    out
}

/// 用户这句话是不是在要图。
///
/// 只在「用户要画、模型却光说不练」时才催它动手。纯聊天（问配色怎么配、
/// 问这个工具怎么使）用文字收尾是天经地义，一催就凭空多出一段废话，
/// 反而更吵。判漏了顶多退回老行为——直接收尾；判错了才真闹心，
/// 所以宁可保守，也不要把每次问答都当成画画。
fn asks_for_artwork(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    // 问句是在等解释，不是在等一个工具调用。
    if trimmed.contains('?') || trimmed.contains('？') {
        return false;
    }
    // 「画」字在中文里太常见（计划、蓝图、画布、策划），只在它不属于这些词时才认。
    for (i, _) in trimmed.match_indices('画') {
        let prev: Option<char> = trimmed[..i].chars().next_back();
        let next: Option<char> = trimmed[i + '画'.len_utf8()..].chars().next();
        let borrowed = matches!(
            prev,
            Some('计') | Some('谋') | Some('策') | Some('蓝') | Some('构') | Some('规')
        ) || next == Some('布');
        if !borrowed {
            return true;
        }
    }
    const ZH: [&str; 14] = [
        "绘制",
        "涂",
        "描一",
        "做个",
        "做一张",
        "来一张",
        "来一幅",
        "生成",
        "改成",
        "换个",
        "加一",
        "去掉",
        "删掉",
        "重画",
    ];
    for word in ZH {
        if trimmed.contains(word) {
            return true;
        }
    }
    const EN: [&str; 11] = [
        "draw", "paint", "sketch", "render", "generate", "colour", "color", "make", "create",
        "add", "replace",
    ];
    let lower = trimmed.to_lowercase();
    let bytes = lower.as_bytes();
    for word in EN {
        let mut from = 0usize;
        while let Some(at) = lower[from..].find(word) {
            let start = from + at;
            let end = start + word.len();
            let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return true;
            }
            from = start + 1;
        }
    }
    false
}

/// 这一串字是不是在问用户问题。
///
/// 澄清提问是合法收尾，不该被催。判据刻意放宽：宁可漏催一次（回到
/// 老行为，直接收尾），也不能把用户真正需要回答的问题一口吞掉。
fn looks_like_a_question(text: &str) -> bool {
    text.contains('?') || text.contains('？')
}

fn nudging_tools_status() -> UiText {
    UiText::new(
        "agent.nudging_tools",
        "这个模型光说话没动手，已催它直接调工具",
    )
}

/// 「以为被掐断了，其实已经写完」的状态条。
fn finished_whole_status() -> UiText {
    UiText::new(
        "agent.finished_whole",
        "the reply already looks complete, nothing more to continue",
    )
}

/// 请求彻底失败时摊在用户面前的那一条。Rust 只说键和原始原因，措辞走字典。
fn request_failed(reason: &str) -> UiText {
    // 报错里常裹着一整条 URL 或响应体，截一刀免得红卡片把聊天区撑爆。
    let short: String = reason.chars().take(200).collect();
    let short = if reason.chars().count() > 200 {
        format!("{short}...")
    } else {
        short
    };
    UiText::new("agent.request_failed", "the request failed: {reason}").with("reason", short)
}

/// 「一直想、始终不动笔」的收场。
///
/// 这类失败最容易被用户误解成「工具坏了」，所以除了「只吐了思考」，
/// 还要给出两条能自己走出去的路：把输出上限调大，或者换个更早动笔的模型。
fn stalled_thinking_message(rounds: usize) -> UiText {
    UiText::new(
        "agent.thinking_only",
        "after {rounds} continuation(s) the model has spent the whole output budget on reasoning without writing any text or making a tool call; raise Max tokens in model settings, or switch to a model that acts sooner",
    )
    .with("rounds", rounds as u64)
}

/// 工具结果首行摘要，用于 UI 工具卡片标题。
fn summarize(content: &str) -> String {
    let first = content.lines().next().unwrap_or("").trim();
    let count = first.chars().count();
    if count == 0 {
        return "(no output)".into();
    }
    if count <= 180 {
        return first.to_string();
    }
    let mut out: String = first.chars().take(180).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixel_core::document::Document;

    use super::super::providers::ProviderError;

    #[test]
    fn idle_watch_only_expires_after_the_limit() {
        let mut watch = IdleWatch::new(Duration::from_millis(20));
        assert!(!watch.expired(), "刚建好就判死刑是误伤");
        std::thread::sleep(Duration::from_millis(40));
        assert!(watch.expired(), "真静默了就得认");
        // 收到字节就重打表：思考再久也不能算卡住。
        watch.touch();
        assert!(!watch.expired());
    }

    #[test]
    fn only_art_requests_count_as_art_requests() {
        // 要图的：中英双语、带不带主语都算。
        for yes in [
            "画一只猫",
            "给我画5帧橘猫奔跑",
            "重新画这个头盔",
            "绘制一个宝箱",
            "涂个背景",
            "做个五帧的行走循环",
            "来一张西瓜",
            "生成一把剑",
            "draw a 32x32 knight",
            "DRAW 4 walking frames",
            "make me a run cycle",
            "add a shadow under it",
            "把配色改成冷色",
        ] {
            assert!(asks_for_artwork(yes), "这句在要图：{yes}");
        }
        // 不要图的：纯聊天、提问、以及「画」字被另用的词。
        for no in [
            "",
            "   ",
            "解释一下这个面板怎么用",
            "这段配色什么意思",
            "怎么导出 aseprite",
            "画布上现在有什么",
            "下一版准备怎么规划",
            "这是不是一个蓝图",
            "你觉得用什么颜色好？",
            "现在几点了？",
            "withdraw that change",
            "what does this drawer do",
            "帮我看看这个文件",
        ] {
            assert!(!asks_for_artwork(no), "这句不在要图：{no}");
        }
    }

    #[test]
    fn only_the_output_limit_counts_as_a_cut_off_reply() {
        // 撞线的两种口径：Anthropic 报 max_tokens，OpenAI 报 length。
        assert_eq!(stop_kind("max_tokens"), StopKind::Truncated);
        assert_eq!(stop_kind("length"), StopKind::Truncated);
        // 其余都是模型自己把话说完了，再「续写」就是硬逼它复读。
        for reason in ["end_turn", "tool_use", "stop_sequence", "stop", ""] {
            assert_eq!(
                stop_kind(reason),
                StopKind::Finished,
                "{reason} 不该判成掐断"
            );
        }
    }

    #[test]
    fn retry_backoff_doubles_then_stops_growing() {
        assert_eq!(retry_backoff(1), Duration::from_millis(1000));
        assert_eq!(retry_backoff(2), Duration::from_millis(2000));
        assert_eq!(retry_backoff(3), Duration::from_millis(4000));
        assert_eq!(retry_backoff(4), Duration::from_millis(8000));
        // 再往后也没有更长：用户不该为一次失败等上半天。
        assert_eq!(retry_backoff(5), Duration::from_millis(8000));
        assert_eq!(retry_backoff(99), Duration::from_millis(8000));
    }

    #[test]
    fn a_round_with_nothing_in_it_is_not_worth_resuming() {
        let empty = RoundRaw::empty();
        assert!(!empty.resumable(), "一个字都没有，重发才是正解");
        assert!(empty.blocks().is_empty());

        let mut thought_only = RoundRaw::empty();
        thought_only.reasoning = "先想想画什么".into();
        assert!(thought_only.resumable(), "思考也算说了一半");
        assert_eq!(thought_only.blocks().len(), 1);

        let mut full = RoundRaw::empty();
        full.text = "这样画".into();
        assert!(full.resumable());
        // 推理在前、正文在后，跟流里的到达顺序一致。
        let blocks = full.blocks();
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0], ContentBlock::Text { .. }));
    }

    #[test]
    fn a_truncated_round_can_replay_its_text_as_history() {
        let mut raw = RoundRaw::empty();
        raw.text = "前一半".into();
        raw.reasoning = "想了想".into();
        raw.stop = StopKind::Truncated;

        let blocks = raw.blocks();
        assert_eq!(blocks.len(), 2);
        match (&blocks[0], &blocks[1]) {
            (ContentBlock::Reasoning { text }, ContentBlock::Text { text: t }) => {
                assert_eq!(text, "想了想");
                assert_eq!(t, "前一半");
            }
            _ => panic!("块的顺序应该是推理在前、正文在后"),
        }
    }

    #[test]
    fn the_continue_nudge_pins_the_resume_point() {
        let carried = "上面已经写好的正文".repeat(40);
        let nudge = continue_nudge(&carried, false).to_lowercase();
        // 断点原文必须原样出现在指令里：模型只能接着那几个字往下走，
        // 没给它锚点的话它会当新问题重想一遍。
        let tail: String = carried
            .chars()
            .rev()
            .take(RESUME_TAIL_CHARS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        assert!(nudge.contains(&tail.to_lowercase()), "指令里少了断点原文");
        assert!(nudge.contains("do not repeat"), "少了「别重复」");
        assert!(nudge.contains("do not start over"), "少了「别重起」");
        assert!(
            nudge.contains("do not re-announce a plan"),
            "少了「别复述计划」"
        );
        assert!(!nudge.contains('续'), "喂模型的话不能夹中文");
    }

    #[test]
    fn the_continue_nudge_gets_harsher_once_the_model_repeats_itself() {
        let carried = "上面已经写好的正文".repeat(40);
        let plain = continue_nudge(&carried, false).to_lowercase();
        let scolded = continue_nudge(&carried, true).to_lowercase();
        // 复读过的模型光说一句「别重复」是按不住的，得点明上一发那句照搬
        // 已经被丢掉。语气加重了，锚点还在：不然模型连断点在哪都不知道。
        assert!(!plain.contains("repeating text"), "没复读不该挨骂");
        assert!(scolded.contains("repeating text"), "复读过要把话说重");
        assert!(
            scolded.contains("that repetition was discarded"),
            "得告诉它那段照搬没算数"
        );
        assert!(
            scolded.contains("do not start over"),
            "加码不能把原来的要求挤掉"
        );
        assert!(!scolded.contains('重'), "喂模型的话不能夹中文");
    }

    #[test]
    fn a_retyped_tail_is_eaten_before_the_user_can_see_it() {
        // 续写时最爱犯的毛病：把刚写完的最后几句原样再念一遍，再接真内容。
        let written = "先把这五帧行走图的姿态从头规划一遍，然后按相位写进 Lua 脚本里面去。";
        let echo = "然后按相位写进 Lua 脚本里面去。";
        let rest = "接下来按相位把每一帧的腿在什么地方写清楚。";
        let mut trim = EchoTrim::new("", written);
        // 分几段喂，跟流式到达的样子差不多。
        let mut fresh = String::new();
        for piece in echo.chars().collect::<Vec<_>>().chunks(3) {
            let chunk: String = piece.iter().collect();
            fresh.push_str(&trim.feed_text(&chunk));
        }
        fresh.push_str(&trim.feed_text(rest));
        // 最后一个字还按在手里等下一个字符表态，收尾时它会还回来。
        let (tail, _) = trim.seal();
        fresh.push_str(&tail);
        assert_eq!(fresh, rest, "复读的那截不能在界面上出现第二遍");
        assert!(trim.ate_something(), "得记下吃过复读");
    }

    #[test]
    fn a_repeat_too_short_to_be_sure_is_left_alone() {
        let written = "这段话的结尾只有短短几个字。";
        let mut trim = EchoTrim::new("", written);
        // 十几个字以内的重合，在代码和套话里太常见。删错了就是删真东西，
        // 宁可让用户多看一遍。
        let fresh = trim.feed_text("几个字。接下来是真正新写的内容");
        assert_eq!(fresh, "几个字。接下来是真正新写的内容");
        assert!(!trim.ate_something(), "这么短不该动手");
    }

    #[test]
    fn a_tail_still_held_when_the_stream_ends_is_dropped() {
        let written = "模型念到刚写完的地方就没词了，整段照搬到这里戛然而止。";
        let echo = "就没词了，整段照搬到这里戛然而止。";
        let mut trim = EchoTrim::new("", written);
        // 一个字都没多吐：手里按着的那截全是复读，收尾时咽掉。
        assert_eq!(trim.feed_text(echo), "");
        let (fresh_text, fresh_reasoning) = trim.seal();
        assert_eq!(fresh_text, "");
        assert_eq!(fresh_reasoning, "");
        assert!(trim.ate_something());
    }

    #[test]
    fn a_short_tail_still_held_comes_back_as_text() {
        // 对不齐又不够长的那截不能咽：流到这儿新内容还没来齐，
        // 咽了就是真删了用户要看的东西。
        let mut trim = EchoTrim::new("", "前半段正文已经写好了。");
        assert_eq!(trim.feed_text("前半段正文"), "");
        assert_eq!(trim.seal().0, "前半段正文");
    }

    #[tokio::test]
    async fn a_retyped_tail_never_shows_up_twice_in_a_live_run() {
        let s = session();
        let written = "先把这五帧行走图的姿态从头规划一遍，然后按相位写进 Lua 脚本里面去。";
        let echo = "然后按相位写进 Lua 脚本里面去。";
        let rest = "接下来按相位把每一帧的腿落在什么地方写清楚。";
        rewire(
            &s,
            vec![
                cut(written),
                // 续写一发：先把刚写的尾巴原样念一遍，再往下接。
                done(&format!("{echo}{rest}")),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // 纯接话不算在要图：这句话只是为了让 turn 里没有工具调用，
        // 免得 runner 抬手就催它去画画，测试就跑到第三条脚本上去了。
        s.run_turn("接着刚才的继续写".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(flow.completed, "续写接上了就该好好收尾");
        assert_eq!(flow.text, format!("{written}{rest}"));
    }

    #[test]
    fn a_model_retyping_the_same_words_makes_no_progress() {
        let carried = "前半段正文".repeat(30);
        assert!(
            !continuation_progressed(&carried, &carried),
            "把已有内容原样再吐一遍不算往下走"
        );
        assert!(
            !continuation_progressed(&carried, "嗯"),
            "只吐一两个字也不算"
        );
        // 一穷二白的时候没得可复读：接上个反引号、半个函数名都该放行，
        // 不然模型第一发只吐出几字符就会被误判成卡住。
        assert!(continuation_progressed("", "```"), "开头几个字也算推进");
        assert!(
            !continuation_progressed("", ""),
            "一个字符都不吐才是真的没动"
        );
        assert!(
            continuation_progressed(
                &carried,
                "紧接着往下写的新内容，长度要足够算一次有效的推进才行"
            ),
            "真的往下写了要认"
        );
    }

    #[test]
    fn continuation_and_retry_budgets_are_capped() {
        // 上限放到二十次：按 64k 的输出算约一百三十万 token，对一段分镜脚本
        // 就是「写不完不收手」。真正的刹车是原地复读检测。
        assert_eq!(MAX_CONTINUATIONS, 20);
        assert_eq!(MAX_ROUND_RETRIES, 5, "重试上限就是五次");
        // 纯思考的闸比正文续写紧得多：这是「模型不想动笔」时的第一道刹车。
        assert_eq!(MAX_REASONING_CONTINUATIONS, 2);
    }

    #[test]
    fn the_status_carry_the_counts_the_ui_needs() {
        let text = continuing_status(2, MAX_CONTINUATIONS);
        assert_eq!(text.key, "agent.continuing");
        assert_eq!(text.vars["done"], serde_json::json!(2));
        assert_eq!(text.vars["max"], serde_json::json!(MAX_CONTINUATIONS));
        assert_eq!(
            text.fallback,
            "the reply hit the output limit, continuing ({done} of {max})"
        );

        let text = retrying_status("connection refused", 1, MAX_ROUND_RETRIES);
        assert_eq!(text.key, "agent.retrying");
        assert_eq!(text.vars["attempt"], serde_json::json!(1));
        assert_eq!(text.vars["reason"], serde_json::json!("connection refused"));
    }

    /// 假 provider：按脚本顺序吐出事件。request 一次取一截，取完就闭嘴
    /// （空流一轮就收场），这样能把「掐断之后重发、重发完接着问」走到底。
    struct ScriptedProvider {
        scripts: Mutex<Vec<Vec<Result<LlmEvent, ProviderError>>>>,
        /// 每一发请求带过来的关思考开关，按发车顺序记。要断言「光思考不干活
        /// 时自动翻盘」就得看这个。
        seen: Mutex<Vec<bool>>,
        /// 每一发请求的系统提示词，按发车顺序记。要断言「模型纠正分流之后，
        /// 下一发真的带着新约束上路」就得看这个。
        systems: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl providers::LlmProvider for ScriptedProvider {
        async fn request(
            &self,
            req: &ChatRequest,
        ) -> Result<providers::EventStream, ProviderError> {
            let script = self.scripts.lock().unwrap().remove(0);
            self.seen.lock().unwrap().push(req.disable_thinking);
            self.systems.lock().unwrap().push(req.system.clone());
            Ok(Box::pin(futures_util::stream::iter(script)))
        }
    }

    /// 把会话的 provider 换成假货。engine 的 ModelConfig 原样留着：max_tokens
    /// 之类的差异不该影响续写逻辑本身。
    fn rewire(s: &AgentSession, scripts: Vec<Vec<Result<LlmEvent, ProviderError>>>) {
        rewire_watch(s, scripts);
    }

    /// 同上，但把假 provider 交出来：测试要能回看每发请求带了什么。
    fn rewire_watch(
        s: &AgentSession,
        scripts: Vec<Vec<Result<LlmEvent, ProviderError>>>,
    ) -> Arc<ScriptedProvider> {
        let provider = Arc::new(ScriptedProvider {
            scripts: Mutex::new(scripts),
            seen: Mutex::new(Vec::new()),
            systems: Mutex::new(Vec::new()),
        });
        let watched = Arc::clone(&provider);
        let config = s.engine.lock().unwrap().config.clone();
        *s.engine.lock().unwrap() = Engine { config, provider };
        watched
    }

    /// 收集一场 turn 的全部出口：正文、状态键、工具名、是否收尾、报错。
    struct Flow {
        text: String,
        statuses: Vec<String>,
        tools: Vec<String>,
        completed: bool,
        error: Option<String>,
        /// 报错带的变量拼成的文本，用来看「这句报错到底交代了什么」。
        error_detail: Option<String>,
    }

    fn drain(mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>) -> Flow {
        let mut flow = Flow {
            text: String::new(),
            statuses: Vec::new(),
            tools: Vec::new(),
            completed: false,
            error: None,
            error_detail: None,
        };
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Token { text } => flow.text.push_str(&text),
                AgentEvent::Reasoning { text } => flow.text.push_str(&text),
                AgentEvent::Status { message } => flow.statuses.push(message.key),
                AgentEvent::ToolCall { name, .. } => flow.tools.push(name),
                AgentEvent::Completed { .. } => flow.completed = true,
                AgentEvent::Error { message } => {
                    flow.error = Some(message.key.clone());
                    let detail = message
                        .vars
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !detail.is_empty() {
                        flow.error_detail = Some(detail);
                    }
                }
                _ => {}
            }
        }
        flow
    }

    /// 一截被输出上限掐断的回复。
    fn cut(chunk: &str) -> Vec<Result<LlmEvent, ProviderError>> {
        vec![
            Ok(LlmEvent::Token(chunk.into())),
            Ok(LlmEvent::Done {
                stop_reason: "max_tokens".into(),
            }),
        ]
    }

    /// 一截说完了的回复。
    fn done(chunk: &str) -> Vec<Result<LlmEvent, ProviderError>> {
        vec![
            Ok(LlmEvent::Token(chunk.into())),
            Ok(LlmEvent::Done {
                stop_reason: "end_turn".into(),
            }),
        ]
    }

    /// 一整轮都耗在思考上，正文一个字都没吐出来——推理模型被掐断时最常见的形态。
    fn cut_after_thinking(thought: &str) -> Vec<Result<LlmEvent, ProviderError>> {
        vec![
            Ok(LlmEvent::Reasoning(thought.into())),
            Ok(LlmEvent::Done {
                stop_reason: "max_tokens".into(),
            }),
        ]
    }

    #[tokio::test]
    async fn a_round_that_only_thought_is_still_progress() {
        let s = session();
        // 推理模型最常见的形态：整轮预算都烧在思考上，正文一个字都没吐。
        // 这种轮次必须接着续，不能因为「正文为空」就判定它卡死了。
        rewire(
            &s,
            vec![
                cut_after_thinking("先把这个五帧行走的橘猫从头想一遍，想得很长很长很长"),
                cut("接着写正文，这一轮终于落到脚本上了，长度足够算推进"),
                done("写完了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx)
            .await;
        let flow = drain(rx);

        assert!(flow.completed, "两轮续写之后该正常收尾：{:?}", flow.error);
        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert_eq!(flow.statuses.len(), 2, "两轮掐断就是两次续写提示");
    }

    #[tokio::test]
    async fn a_cut_off_reply_is_continued_until_the_model_finishes() {
        let s = session();
        rewire(&s, vec![cut("前半段"), done("后半段")]);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx)
            .await;
        let flow = drain(rx);

        // 两截该原样接上，中间的续写指令只走历史，不能挤到用户眼前。
        assert_eq!(flow.text, "前半段后半段");
        assert!(flow.completed, "续写之后这一轮该正常收尾");
        assert!(flow.error.is_none());
        assert_eq!(flow.statuses, vec!["agent.continuing".to_string()]);

        let history = s.messages.lock().unwrap().clone();
        let texts: Vec<String> = history.iter().map(|m| m.text_of()).collect();
        assert_eq!(texts.len(), 4, "原话、半截回复、续写指令、完整回复");
        assert_eq!(texts[1], "前半段", "半截回复要原样进历史");
        assert!(
            texts[2].contains("---前半段\n---"),
            "续写指令得把断点原文按在模型眼前：{texts:?}"
        );
        assert!(
            texts[2].contains("Continue from that exact point"),
            "还得明确要求它接着写：{texts:?}"
        );
    }

    #[tokio::test]
    async fn a_failed_request_is_retried_before_giving_up() {
        let s = session();
        rewire(
            &s,
            vec![
                vec![Err(ProviderError::Network("connection refused".into()))],
                done("通了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx)
            .await;
        let flow = drain(rx);

        assert_eq!(flow.text, "通了", "重发一次就该拿到正文");
        assert!(flow.completed);
        assert!(flow.error.is_none());
        assert_eq!(flow.statuses, vec!["agent.retrying".to_string()]);
    }

    #[tokio::test]
    async fn an_unauthorised_request_is_not_retried_at_all() {
        let s = session();
        rewire(
            &s,
            vec![
                vec![Err(ProviderError::Http {
                    status: 401,
                    body: "invalid api key".into(),
                })],
                done("这本不该被拿到"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        // 401 重发五次也是同一个 401，还得白等 31 秒。直接现形，让用户去改 key。
        assert!(!flow.completed);
        assert!(flow.text.is_empty());
        assert!(flow.statuses.is_empty(), "不该有重试提示");
        let detail = flow.error_detail.unwrap_or_default();
        assert!(detail.contains("401"), "{detail}");
        assert_eq!(flow.error.as_deref(), Some("agent.request_failed"));
        assert_eq!(s.messages.lock().unwrap().len(), 1, "只有用户那一句");
    }

    #[test]
    fn only_the_transient_failures_are_worth_another_try() {
        // 换个时间就能好的：网络、解析、限流、超时、服务端 5xx。
        for err in [
            ProviderError::Network("reset".into()),
            ProviderError::Decode("truncated chunk".into()),
            ProviderError::Http {
                status: 408,
                body: String::new(),
            },
            ProviderError::Http {
                status: 429,
                body: String::new(),
            },
            ProviderError::Http {
                status: 503,
                body: String::new(),
            },
        ] {
            assert!(retryable(&err), "{err} 该重发");
        }
        // 请求本身不被接受：重发只是把同一个报错看五遍。
        for err in [
            ProviderError::Http {
                status: 400,
                body: String::new(),
            },
            ProviderError::Http {
                status: 401,
                body: String::new(),
            },
        ] {
            assert!(!retryable(&err), "{err} 不该重发");
        }
        // 403 看着像「服务器不答应」，实际多半是限流窗口或套餐切换：等一等就放行。
        for err in [
            ProviderError::Http {
                status: 403,
                body: r#"{"error":{"code":"7","type":"permission_denied_error"}}"#.into(),
            },
            ProviderError::Http {
                status: 409,
                body: String::new(),
            },
            ProviderError::Http {
                status: 425,
                body: String::new(),
            },
        ] {
            assert!(retryable(&err), "{err} 该重发");
        }
        // 剩下的 4xx 是请求本身写错了：重发多少次都是同一句报错。
        for err in [
            ProviderError::Http {
                status: 404,
                body: String::new(),
            },
            ProviderError::Http {
                status: 413,
                body: String::new(),
            },
            ProviderError::Http {
                status: 422,
                body: String::new(),
            },
            ProviderError::Config("no base url".into()),
        ] {
            assert!(!retryable(&err), "{err} 不该重发");
        }
    }

    /// 403「套餐不覆盖这个模型」要按重试对待：网关侧的限流窗口、套餐切换常以
    /// 403 的形式出现，等一等就放行。重试满五次还不行才摊到用户脸上。
    #[tokio::test]
    async fn a_permission_denied_is_retried_before_it_is_shown_to_the_user() {
        let s = session();
        // ProviderError 不 Clone，用闭包现造：每一发都是同一个 403。
        let denied = || {
            ProviderError::Http {
            status: 403,
            body: r#"{"error":{"message":"model is not available in the current token plan","type":"permission_denied_error","code":"7"}}"#.into(),
        }
        };
        rewire(
            &s,
            // 首发 + 五次重发：第六发才发现次数用尽，所以要备六份剧本。
            (0..=MAX_ROUND_RETRIES)
                .map(|_| vec![Err(denied())])
                .collect(),
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert_eq!(
            flow.statuses.len(),
            MAX_ROUND_RETRIES,
            "每一次重试都要让用户看见"
        );
        assert!(
            flow.statuses.iter().all(|k| k == "agent.retrying"),
            "{:?}",
            flow.statuses
        );
        assert!(!flow.completed, "五次都没成，不许悄悄收尾");
        assert_eq!(flow.error.as_deref(), Some("agent.request_failed"));
        let detail = flow.error_detail.unwrap_or_default();
        assert!(detail.contains("403"), "报错要带上原文：{detail}");
    }

    /// 无限思考的刹车：连续几轮只吐推理、正文和工具调用一个都没有，就收摊。
    /// 不然模型会把二十次续写全烧在思考上，用户看到的是一个永远不动笔的思考节点。
    #[tokio::test]
    async fn a_model_that_never_stops_thinking_is_cut_off_early() {
        let s = session();
        // 钉住「开着思考」：不然第一次死思考就被自动翻盘接走了，
        // 测的就不是护栏本身。
        s.engine.lock().unwrap().config.disable_thinking = Some(false);
        let cap = s.runner_config().loop_limits.max_reasoning_continuations;
        rewire(
            &s,
            (0..cap + 2)
                .map(|i| {
                    cut_after_thinking(&format!(
                        "第 {i} 轮思考，还在想这只橘猫该怎么画，想了很久很久，一个字没写"
                    ))
                })
                .collect(),
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(!flow.completed, "死思考不许悄悄收尾");
        assert_eq!(flow.statuses.len(), cap, "只许续到护栏上限，多一轮都不给");
        assert_eq!(
            flow.error.as_deref(),
            Some("agent.thinking_only"),
            "要说清是只吐了思考：{:?}",
            flow.error
        );
        let detail = flow.error_detail.unwrap_or_default();
        assert!(detail.contains(&format!("rounds={}", cap + 1)), "{detail}");
        assert!(
            flow.tools.is_empty(),
            "思考到尾也没有工具调用：{:?}",
            flow.tools
        );
    }

    #[tokio::test]
    async fn a_stalled_stream_is_retried_and_the_reason_is_reported() {
        let s = session();
        // 空流一秒就收场，所以这里用「报错」替假死：两条路都汇到 failure。
        rewire(
            &s,
            vec![
                vec![Err(ProviderError::Decode("garbled chunk".into()))],
                done("换了条路"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx)
            .await;
        let flow = drain(rx);

        assert_eq!(flow.text, "换了条路");
        assert!(flow.completed);
        assert_eq!(flow.statuses, vec!["agent.retrying".to_string()]);
    }

    #[tokio::test]
    async fn twenty_continuations_still_hit_the_wall() {
        let s = session();
        // 二十次续写 = 二十一发请求。每一发都得吐出点新东西，不然第一发
        // 就被复读检测拦下，走不到配额用尽这一步。
        let scripts = (0..=MAX_CONTINUATIONS)
            .map(|i| {
                cut(
                    &format!("第 {i} 段被掐断的正文，模型确实在往下写，长度足够算一次推进")
                        .repeat(2),
                )
            })
            .collect::<Vec<_>>();
        rewire(&s, scripts);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(!flow.completed, "配额用尽不许悄悄收尾");
        assert_eq!(
            flow.statuses.len(),
            MAX_CONTINUATIONS,
            "每续一次都要给用户一个进度提示"
        );
        assert!(
            flow.statuses.iter().all(|k| k == "agent.continuing"),
            "{:?}",
            flow.statuses
        );
        assert_eq!(
            flow.error.as_deref(),
            Some("agent.output_limit"),
            "报错要说清是输出上限"
        );
        assert_eq!(
            flow.statuses.len(),
            MAX_CONTINUATIONS,
            "每续一次都要给用户一个进度提示"
        );
        // 用户那一句 + 二十套「半截回复 + 续写指令」；第二十一发是收摊前那一问，
        // 不再回灌，所以总数停在 41 条。用户能整段复制走自己续。
        assert_eq!(s.messages.lock().unwrap().len(), 41);
    }

    #[tokio::test]
    async fn a_model_that_retypes_the_same_words_is_cut_off_early() {
        let s = session();
        let repeated = "这段正文模型早就写过了，原样再吐一遍不算任何推进";
        // 第一发写完好整一段，之后每发都只把它复读出来——与其要二十次，
        // 不如当场告诉用户断在哪。
        rewire(
            &s,
            vec![
                cut(repeated),
                cut(repeated),
                cut(repeated),
                done("这发根本不该被打断"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(!flow.completed, "卡死了不许悄悄收尾");
        // 复读在流式入口就被咽了，用户只看得到第一发那一遍。
        assert_eq!(flow.text, repeated);
        // 只续了第一次就不再要了：后面那两发连同「说完了」都没发生。
        assert_eq!(flow.statuses, vec!["agent.continuing".to_string()]);
        assert_eq!(
            flow.error.as_deref(),
            Some("agent.stalled"),
            "要说清是回复不再推进"
        );
        // 用户那一句 + 第一次续写回灌的「半截回复 + 续写指令」，共三条。
        assert_eq!(s.messages.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_half_written_tool_call_never_reaches_history() {
        let s = session();
        rewire(
            &s,
            vec![
                vec![
                    Ok(LlmEvent::Token("先写脚本".into())),
                    Ok(LlmEvent::ToolUseStart {
                        index: 0,
                        id: "c1".into(),
                        name: "pixel_apply_operations".into(),
                    }),
                    Ok(LlmEvent::ToolInputDelta {
                        index: 0,
                        json_partial: "{\"ops\":[{\"op\":\"set".into(),
                    }),
                    Ok(LlmEvent::Done {
                        stop_reason: "max_tokens".into(),
                    }),
                ],
                vec![
                    Ok(LlmEvent::ToolUseStart {
                        index: 0,
                        id: "c2".into(),
                        name: "pixel_read_canvas".into(),
                    }),
                    Ok(LlmEvent::ToolInputDelta {
                        index: 0,
                        json_partial: "{}".into(),
                    }),
                    Ok(LlmEvent::Done {
                        stop_reason: "tool_use".into(),
                    }),
                ],
                done("画好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(flow.completed, "工具跑完接着问，这一轮要能收尾");
        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert_eq!(flow.text, "先写脚本画好了");
        // 半截入参不可信：进历史会被 provider 整包拒掉，续写那次就白发。
        assert_eq!(flow.tools, vec!["pixel_read_canvas".to_string()]);
        let history = s.messages.lock().unwrap().clone();
        let ids: Vec<String> = history
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["c2".to_string()], "残缺的 c1 不该留下任何痕迹");
    }

    fn session() -> AgentSession {
        AgentSession::new(
            "s1",
            ModelConfig {
                id: "m1".into(),
                label: "m1".into(),
                protocol: super::super::models::Protocol::Anthropic,
                base_url: "https://example.invalid".into(),
                api_key: String::new(),
                model: "test".into(),
                max_tokens: None,
                temperature: None,
                disable_thinking: None,
                capabilities: Default::default(),
            },
            Document::new("t", 8, 8).unwrap(),
        )
    }

    /// 换一把钉死关思考的会话：用户要的是「别想，直接画」。
    fn session_with_thinking_off() -> AgentSession {
        let s = session();
        let mut config = s.engine.lock().unwrap().config.clone();
        config.disable_thinking = Some(true);
        let provider = s.engine.lock().unwrap().provider.clone();
        *s.engine.lock().unwrap() = Engine { config, provider };
        s
    }

    /// 一整轮都烧在思考上、一个工具都没调：替用户翻一次盘，关掉思考再问。
    /// 这是 LongCat、DeepSeek-R1 一系的常态——不翻这一下，用户盯到的是
    /// 一个永远在思考的节点，画布上连一笔都没有。
    #[tokio::test]
    async fn a_thinking_only_turn_retries_once_with_thinking_off() {
        let s = session();
        let watched = rewire_watch(
            &s,
            vec![
                cut_after_thinking("先把整只猫在脑子里过一遍"),
                vec![
                    Ok(LlmEvent::ToolUseStart {
                        index: 0,
                        id: "c1".into(),
                        name: "pixel_run_shader".into(),
                    }),
                    Ok(LlmEvent::ToolInputDelta {
                        index: 0,
                        json_partial: "{}".into(),
                    }),
                    Ok(LlmEvent::Done {
                        stop_reason: "tool_use".into(),
                    }),
                ],
                done("画好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert_eq!(
            watched.seen.lock().unwrap().as_slice(),
            // 第一发照模型默认来，翻盘后那发带关思考；第三发是工具结果回灌后的
            // 追问，翻盘在本 turn 内一直生效。
            &[false, true, true],
            "第一发照模型默认来，翻盘后那发才带关思考",
        );
        assert!(
            flow.statuses
                .contains(&"agent.thinking_off_retry".to_string()),
            "用户该知道我们关了思考：{:?}",
            flow.statuses,
        );
        // 「画一只猫」什么都没定出来，就不该摆分流节点：空节点只是噪声。
        assert_eq!(flow.tools, vec!["pixel_run_shader".to_string()]);
        assert!(flow.completed, "{:?}", flow.error);
    }

    /// 平台不报截断的翻盘：stop reason 说「说完啦」，其实整轮预算都烧在推理里。
    /// 只按 stop reason 判断的话，这种平台每次都要先白烧一轮，用户看着就是
    /// 永远在思考、永远不动笔。
    #[tokio::test]
    async fn a_finished_round_that_only_thought_also_retries_with_thinking_off() {
        let s = session();
        let long_thought = "想".repeat(REASONING_ONLY_CHARS + 40);
        let watched = rewire_watch(
            &s,
            vec![
                vec![
                    Ok(LlmEvent::Reasoning(long_thought)),
                    Ok(LlmEvent::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ],
                vec![
                    Ok(LlmEvent::ToolUseStart {
                        index: 0,
                        id: "c1".into(),
                        name: "pixel_run_shader".into(),
                    }),
                    Ok(LlmEvent::ToolInputDelta {
                        index: 0,
                        json_partial: "{}".into(),
                    }),
                    Ok(LlmEvent::Done {
                        stop_reason: "tool_use".into(),
                    }),
                ],
                done("画好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(
            flow.statuses
                .contains(&"agent.thinking_off_retry".to_string()),
            "不报截断的长思考也该关掉思考重问：{:?}",
            flow.statuses,
        );
        assert_eq!(
            watched.seen.lock().unwrap().as_slice(),
            &[false, true, true],
            "翻盘后该带着关思考重发",
        );
        assert_eq!(flow.tools, vec!["pixel_run_shader".to_string()]);
        assert!(flow.completed, "{:?}", flow.error);
    }

    /// 想清楚了就动笔的模型不许被误伤：短推理 + 正常收尾，不动它的思考开关。
    #[tokio::test]
    async fn a_short_reasoning_round_is_left_alone() {
        let s = session();
        let watched = rewire_watch(
            &s,
            vec![vec![
                Ok(LlmEvent::Reasoning("这个问题是在问面板，不是在要图".into())),
                Ok(LlmEvent::Done {
                    stop_reason: "end_turn".into(),
                }),
            ]],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("这个面板怎么用？".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert_eq!(
            watched.seen.lock().unwrap().as_slice(),
            &[false],
            "照模型默认来，不许偷偷改开关：{:?}",
            watched.seen.lock().unwrap(),
        );
        assert!(
            !flow
                .statuses
                .contains(&"agent.thinking_off_retry".to_string()),
            "没烧预算就不该翻盘：{:?}",
            flow.statuses,
        );
    }

    /// 光说话不干活：模型讲两句「已完成」就想收场，画布上一笔没有。
    /// 这种轮次必须当场催一次，催到它真调工具为止。
    #[tokio::test]
    async fn a_turn_that_only_talked_is_nudged_into_acting() {
        let s = session();
        rewire(
            &s,
            vec![
                done("I'll draw a 5-frame orange cat run cycle."),
                vec![
                    Ok(LlmEvent::ToolUseStart {
                        index: 0,
                        id: "c1".into(),
                        name: "pixel_run_shader".into(),
                    }),
                    Ok(LlmEvent::ToolInputDelta {
                        index: 0,
                        json_partial: "{}".into(),
                    }),
                    Ok(LlmEvent::Done {
                        stop_reason: "tool_use".into(),
                    }),
                ],
                done("五帧奔跑画完了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画5帧橘猫奔跑".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(
            flow.statuses.contains(&"agent.nudging_tools".to_string()),
            "催过就得让用户看见：{:?}",
            flow.statuses,
        );
        // 「5帧」先被定成 sprite 意图，分流节点跟着这一轮一起摆出去，
        // 然后才是模型自己调的绘图工具。
        assert_eq!(
            flow.tools,
            vec!["pixel_plan".to_string(), "pixel_run_shader".to_string()]
        );
        assert!(flow.completed, "{:?}", flow.error);
        // 催问自己也进历史，不然模型看不到「你刚才什么都没做」。
        let pushed = s
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|m| matches!(m.role, super::super::models::Role::User))
            .count();
        assert!(pushed >= 2, "除了用户原话，还得有一条催问：{pushed}");
    }

    /// 一张贴进来的参考图。
    fn reference_image() -> Attachment {
        Attachment {
            role: crate::models::AttachmentRole::Reference,
            media_type: "image/png".into(),
            data_base64: "AAAA".into(),
        }
    }

    /// 一张只是上下文的快照：不参与参照定性。
    fn snapshot_image() -> Attachment {
        Attachment {
            role: crate::models::AttachmentRole::Snapshot,
            media_type: "image/png".into(),
            data_base64: "AAAA".into(),
        }
    }

    /// 参照图在场的一轮：分流节点必须抢在绘图工具前面摆出去。
    /// 反过来的话，用户只能盯着模型画完一整张复刻图，才知道自己只要了个配色。
    #[tokio::test]
    async fn a_reference_turn_routes_before_the_model_draws() {
        let s = session();
        rewire(
            &s,
            vec![
                vec![
                    Ok(LlmEvent::ToolUseStart {
                        index: 0,
                        id: "c1".into(),
                        name: "pixel_run_shader".into(),
                    }),
                    Ok(LlmEvent::ToolInputDelta {
                        index: 0,
                        json_partial: "{}".into(),
                    }),
                    Ok(LlmEvent::Done {
                        stop_reason: "tool_use".into(),
                    }),
                ],
                done("按这个配色换好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("照这个画风换色".into(), vec![reference_image()], tx)
            .await;
        let flow = drain(rx);

        assert_eq!(
            flow.tools.first().map(String::as_str),
            Some("pixel_plan"),
            "分流节点排在绘图工具前面：{:?}",
            flow.tools,
        );
        assert!(
            flow.tools.contains(&"pixel_run_shader".to_string()),
            "{:?}",
            flow.tools
        );
        assert!(flow.completed, "{:?}", flow.error);
    }

    /// 模型纠正分流：原话读出来是图标，它说其实是场景。纠正对下一发请求
    /// 立刻生效——不生效的话，模型还得等用户再补一句话才按新约束画。
    #[tokio::test]
    async fn a_plan_correction_rides_the_next_request() {
        let s = session();
        let watched = rewire_watch(
            &s,
            vec![
                vec![
                    Ok(LlmEvent::ToolUseStart {
                        index: 0,
                        id: "c1".into(),
                        name: PLAN_TOOL.into(),
                    }),
                    Ok(LlmEvent::ToolInputDelta {
                        index: 0,
                        json_partial: r#"{"intent":"scene"}"#.into(),
                    }),
                    Ok(LlmEvent::Done {
                        stop_reason: "tool_use".into(),
                    }),
                ],
                done("按场景画好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画个图标".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(flow.completed, "{:?}", flow.error);
        let systems = watched.systems.lock().unwrap();
        assert!(
            systems.len() >= 2,
            "纠正之后还得再发一发才好接着画：{:?}",
            systems.len()
        );
        assert!(
            systems[0].contains("deliverable: icon"),
            "第一发照用户原话来：{}",
            systems[0]
        );
        assert!(
            systems[1].contains("deliverable: scene"),
            "纠正之后的一发带着新约束：{}",
            systems[1]
        );
        assert!(
            systems[1].contains("far, mid and near planes"),
            "场景的规则得跟着走，不然等于没纠正：{}",
            systems[1]
        );
    }

    /// 模型重述一条已经生效的分流：这一发什么都没改，下一发请求必须原样不动。
    /// 否则「先登记再画」被当成真纠正，模型白赚一个来回，还学着每次都先登记。
    #[tokio::test]
    async fn restating_the_routing_is_a_no_op() {
        let s = session();
        let watched = rewire_watch(
            &s,
            vec![
                vec![
                    Ok(LlmEvent::ToolUseStart {
                        index: 0,
                        id: "c1".into(),
                        name: PLAN_TOOL.into(),
                    }),
                    Ok(LlmEvent::ToolInputDelta {
                        index: 0,
                        json_partial: r#"{"intent":"icon"}"#.into(),
                    }),
                    Ok(LlmEvent::Done {
                        stop_reason: "tool_use".into(),
                    }),
                ],
                vec![
                    Ok(LlmEvent::ToolUseStart {
                        index: 0,
                        id: "c2".into(),
                        name: "pixel_run_shader".into(),
                    }),
                    Ok(LlmEvent::ToolInputDelta {
                        index: 0,
                        json_partial: r#"{"script":"stamp({'..aa..','.a..a.'},{a=pal(1)},0,0)"}"#
                            .into(),
                    }),
                    Ok(LlmEvent::Done {
                        stop_reason: "tool_use".into(),
                    }),
                ],
                done("画好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画个图标".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(flow.completed, "{:?}", flow.error);
        let systems = watched.systems.lock().unwrap();
        assert!(systems.len() >= 2, "{:?}", systems.len());
        assert_eq!(
            systems[0], systems[1],
            "重述一条已生效的分流，不该改动系统提示词"
        );
        // 空转被顶回去之后就该画：第二发带着真家伙。
        assert!(
            flow.tools.contains(&"pixel_run_shader".to_string()),
            "被顶回去之后还是得画：{:?}",
            flow.tools
        );
    }

    /// 纯快照的一轮不发分流节点：没有参照图可定性，摆一张空卡片只是噪声。
    #[tokio::test]
    async fn a_snapshot_only_turn_stays_quiet() {
        let s = session();
        // 这一轮是在要图，光说话会被催满两次才收摊，脚本得多备几条。
        rewire(
            &s,
            vec![
                done("看完了"),
                done("还在看"),
                done("真的看完了"),
                done("收尾"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), vec![snapshot_image()], tx)
            .await;
        let flow = drain(rx);

        assert!(
            !flow.tools.contains(&"pixel_plan".to_string()),
            "快照不定性，别摆节点：{:?}",
            flow.tools
        );
    }

    /// 澄清提问是合法收尾：问完就该停下等用户回话，不能催。
    #[tokio::test]
    async fn a_clarifying_question_is_left_alone() {
        let s = session();
        rewire(&s, vec![done("想要侧视还是四分之三视角？")]);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画只猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(
            !flow.statuses.contains(&"agent.nudging_tools".to_string()),
            "提问不该被催：{:?}",
            flow.statuses,
        );
        assert!(flow.completed, "{:?}", flow.error);
    }

    /// 催也有尽头：催满两次还不动手，就照原样收尾报给用户，
    /// 别把一个轴模型惯成无限循环。
    #[tokio::test]
    async fn a_turn_that_never_acts_stops_after_the_nudge_budget() {
        let s = session();
        rewire(
            &s,
            vec![
                done("I'll draw it."),
                done("Now the cycle."),
                done("It is done."),
                vec![
                    Ok(LlmEvent::Token("算了，直接收尾".into())),
                    Ok(LlmEvent::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ],
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画5帧橘猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        let nudges = flow
            .statuses
            .iter()
            .filter(|k| *k == "agent.nudging_tools")
            .count();
        assert_eq!(nudges, 2, "催满两次就收手：{:?}", flow.statuses);
        assert!(flow.completed, "{:?}", flow.error);
    }

    /// 钉死的选择不翻案：用户要关思考，第一发就得带着这个开关过去，
    /// 也别弹那句「替你关了」的状态条——那本来就是他选的。
    #[tokio::test]
    async fn a_pinned_thinking_off_choice_reaches_the_first_request() {
        let s = session_with_thinking_off();
        let watched = rewire_watch(&s, vec![done("不思考直接画")]);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx)
            .await;
        let flow = drain(rx);

        assert_eq!(watched.seen.lock().unwrap().as_slice(), &[true]);
        assert!(
            !flow
                .statuses
                .contains(&"agent.thinking_off_retry".to_string()),
            "用户自己选的不该再弹状态条：{:?}",
            flow.statuses,
        );
        assert!(flow.completed, "{:?}", flow.error);
    }

    /// 纯聊天用文字收尾是天经地义：问用法、问配色都不能催。
    #[tokio::test]
    async fn a_plain_chat_reply_is_left_alone() {
        let s = session();
        rewire(
            &s,
            vec![done("右上角那个图标是导出，点它有 aseprite 和 PNG 两种。")],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("导出按钮在哪".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert!(
            !flow.statuses.contains(&"agent.nudging_tools".to_string()),
            "纯聊天不该被催：{:?}",
            flow.statuses,
        );
        assert!(flow.completed, "{:?}", flow.error);
    }

    /// 反过来：用户想看思考过程，就别替他关。撞上限也走续写那条老路。
    #[tokio::test]
    async fn a_thinking_on_choice_is_never_overridden() {
        let s = session();
        s.engine.lock().unwrap().config.disable_thinking = Some(false);
        let watched = rewire_watch(
            &s,
            vec![
                cut_after_thinking("想"),
                cut_after_thinking("接着想"),
                cut_after_thinking("还在想"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx).await;
        let flow = drain(rx);

        assert_eq!(
            watched.seen.lock().unwrap().as_slice(),
            &[false, false],
            "钉了「开着思考」就不该翻盘",
        );
        assert!(flow.error.is_some(), "光思考不干活最终要有个交代");
    }

    fn model_with(id: &str, caps: Capabilities) -> ModelConfig {
        ModelConfig {
            id: id.into(),
            label: format!("{id} label"),
            protocol: super::super::models::Protocol::OpenAiCompat,
            base_url: "https://example.invalid".into(),
            api_key: String::new(),
            model: "test".into(),
            max_tokens: None,
            temperature: None,
            disable_thinking: None,
            capabilities: caps,
        }
    }

    #[test]
    fn a_session_without_role_models_answers_with_the_session_model() {
        let s = session();
        for role in ModelRole::all() {
            assert_eq!(s.model_for_role(role).id, "m1", "{role:?} should fall back");
        }
        // 回落时 UI 要显示「正在用主模型凑」，所以 detached 必须是 false。
        for binding in s.role_bindings() {
            assert!(!binding.detached, "{:?} is not detached", binding.role);
            assert_eq!(binding.model_id, "m1");
        }
        assert_eq!(s.effective_capabilities(), Capabilities::default());
    }

    #[test]
    fn binding_a_role_moves_only_that_role() {
        let s = session();
        s.rebind_role(
            ModelRole::ImageGen,
            model_with(
                "img",
                Capabilities {
                    image_gen: true,
                    ..Default::default()
                },
            ),
        );

        assert_eq!(s.model_for_role(ModelRole::ImageGen).id, "img");
        // 另外三个角色没被惊动，还指着主模型。
        assert_eq!(s.model_for_role(ModelRole::Chat).id, "m1");
        assert_eq!(s.model_for_role(ModelRole::Vision).id, "m1");
        assert_eq!(s.model_for_role(ModelRole::Video).id, "m1");

        let bindings = s.role_bindings();
        let gen = bindings
            .iter()
            .find(|b| b.role == ModelRole::ImageGen)
            .unwrap();
        assert!(gen.detached);
        assert_eq!(gen.model_id, "img");
        assert_eq!(gen.model_label, "img label");
    }

    #[test]
    fn effective_capabilities_unions_every_role() {
        let s = session();
        assert!(!s.effective_capabilities().any());

        s.rebind_role(
            ModelRole::ImageGen,
            model_with(
                "img",
                Capabilities {
                    image_gen: true,
                    ..Default::default()
                },
            ),
        );
        s.rebind_role(
            ModelRole::Video,
            model_with(
                "vid",
                Capabilities {
                    video: true,
                    ..Default::default()
                },
            ),
        );

        let caps = s.effective_capabilities();
        assert!(caps.image_gen);
        assert!(caps.video);
        // 没人认领识图，所以它还是关着：并集不是无条件全开。
        assert!(!caps.vision);
    }

    #[test]
    fn rebinding_the_same_role_replaces_the_model() {
        let s = session();
        s.rebind_role(ModelRole::Vision, model_with("v1", Capabilities::default()));
        s.rebind_role(ModelRole::Vision, model_with("v2", Capabilities::default()));
        assert_eq!(s.model_for_role(ModelRole::Vision).id, "v2");
        assert_eq!(
            s.role_bindings()
                .iter()
                .find(|b| b.role == ModelRole::Vision)
                .unwrap()
                .model_id,
            "v2"
        );
    }

    #[test]
    fn clearing_a_role_falls_back_to_the_session_model() {
        let s = session();
        s.rebind_role(
            ModelRole::ImageGen,
            model_with(
                "img",
                Capabilities {
                    image_gen: true,
                    ..Default::default()
                },
            ),
        );
        assert!(s.effective_capabilities().image_gen);

        s.clear_role(ModelRole::ImageGen);
        assert_eq!(s.model_for_role(ModelRole::ImageGen).id, "m1");
        assert!(!s.effective_capabilities().image_gen);
        for binding in s.role_bindings() {
            assert!(!binding.detached);
        }
    }

    #[test]
    fn the_chat_role_cannot_be_rebound_through_the_role_door() {
        // 主模型只能整体 rebind_provider；从角色门改它会让 UI 以为有两个主模型。
        let s = session();
        s.rebind_role(
            ModelRole::Chat,
            model_with("other", Capabilities::default()),
        );
        assert_eq!(s.model_for_role(ModelRole::Chat).id, "m1");
        assert!(s.role_bindings().iter().all(|b| !b.detached));

        // 清一个没绑过的角色也不该炸。
        s.clear_role(ModelRole::Video);
        assert_eq!(s.model_for_role(ModelRole::Video).id, "m1");
    }

    #[test]
    fn auto_lets_everything_through_ask_gates_everything() {
        assert!(!needs_approval(
            PermissionMode::Auto,
            "pixel_apply_operations"
        ));
        assert!(!needs_approval(PermissionMode::Auto, "pixel_run_shader"));
        assert!(needs_approval(
            PermissionMode::Ask,
            "pixel_apply_operations"
        ));
        assert!(needs_approval(PermissionMode::Ask, "pixel_read_canvas"));
    }

    #[test]
    fn chat_gates_writes_and_lets_reads_pass() {
        assert!(needs_approval(
            PermissionMode::Chat,
            "pixel_apply_operations"
        ));
        assert!(needs_approval(PermissionMode::Chat, "pixel_run_shader"));
        assert!(!needs_approval(PermissionMode::Chat, "pixel_read_canvas"));
    }

    #[tokio::test]
    async fn resolving_without_a_pending_request_is_an_error() {
        let s = session();
        assert!(s
            .resolve_approval("nope", ApprovalDecision::Approve)
            .is_err());
    }

    #[tokio::test]
    async fn interrupt_clears_a_pending_approval_so_the_wait_unblocks() {
        let s = session();
        let (tx, mut rx) = oneshot::channel();
        *s.approval.lock().unwrap() = Some(ApprovalSlot {
            call_id: "call-1".into(),
            tx,
        });
        s.interrupt();
        // 发送端被 take 掉，等待端收到的是「通道关闭」，也就是取消，不是放行。
        assert!(rx.try_recv().is_err());
        assert!(s.is_cancelled());
        assert!(s
            .resolve_approval("call-1", ApprovalDecision::Approve)
            .is_err());
    }

    #[tokio::test]
    async fn a_decision_reaches_the_waiting_turn() {
        let s = session();
        let (tx, rx) = oneshot::channel();
        *s.approval.lock().unwrap() = Some(ApprovalSlot {
            call_id: "call-1".into(),
            tx,
        });
        assert!(s
            .resolve_approval("call-1", ApprovalDecision::ApproveAll)
            .is_ok());
        assert_eq!(rx.await.unwrap(), ApprovalDecision::ApproveAll);
        // 槽已空，同一笔再解决一次就是「没有挂起的审批」。
        assert!(s
            .resolve_approval("call-1", ApprovalDecision::Approve)
            .is_err());
    }

    #[test]
    fn a_half_open_fence_reads_as_a_cut_off_reply() {
        // 奇数个 ``` 是「后面还有内容」最硬的证据。
        assert!(looks_cut_off("```lua\npset(1, 1, red)\n"));
        // 一行以连接符收尾：表达式、参数表、对象字面量被从中间剪了一刀。
        assert!(looks_cut_off("let palette = {"));
        assert!(looks_cut_off("draw(cat,\n"));
        assert!(looks_cut_off("if walking &&\n"));
    }

    #[test]
    fn a_reply_that_lands_cleanly_does_not_look_cut_off() {
        // 猜错一次要白跑一整轮续写，所以只认最硬的信号。
        assert!(!looks_cut_off("```lua\npset(1, 1, red)\n```\n画完了"));
        assert!(!looks_cut_off("五帧行走图已经画好，帧时长都是 100ms。"));
        assert!(!looks_cut_off(""));
        assert!(!looks_cut_off("   "));
        // 中文破折号是全角字符，碰不到那张 ASCII 表。
        assert!(!looks_cut_off("收尾之后——"));
    }

    #[test]
    fn the_smallest_token_budget_wins() {
        // 用户填过的值必须被尊重。
        assert_eq!(
            resolve_max_tokens(Some(4096), "claude-sonnet-4-5", None),
            4096
        );
        // 没填过就按模型名查表。
        assert_eq!(resolve_max_tokens(None, "deepseek-v4.1-flash", None), 65536);
        assert_eq!(
            resolve_max_tokens(None, "some-unknown-model", None),
            limits::FALLBACK_MAX_TOKENS
        );
        // provider 报过的上限是硬事实，谁小听谁。
        assert_eq!(
            resolve_max_tokens(Some(64000), "claude-sonnet-4-5", Some(8192)),
            8192
        );
        assert_eq!(
            resolve_max_tokens(None, "deepseek-v4.1-flash", Some(131072)),
            65536,
            "provider 的上限比我们想要的还大时，不该把人往上抬"
        );
    }

    /// 假 provider：一发请求一个结果，并把每发真的要到的 max_tokens 记下来。
    /// 用来验「provider 嫌上限给大了就降下来重发」这条路。
    /// 一发起飞的剧本：整轮要么报错，要么吐一串事件。
    type RoundScript = Vec<Result<LlmEvent, ProviderError>>;
    /// 剧本库：request 一次取一截，报错那一发占一股。
    type RoundScripts = Vec<Result<RoundScript, ProviderError>>;

    struct RecordingProvider {
        outcomes: Mutex<RoundScripts>,
        asked: Mutex<Vec<u32>>,
    }

    #[async_trait::async_trait]
    impl providers::LlmProvider for RecordingProvider {
        async fn request(
            &self,
            req: &ChatRequest,
        ) -> Result<providers::EventStream, ProviderError> {
            self.asked.lock().unwrap().push(req.max_tokens);
            match self.outcomes.lock().unwrap().remove(0) {
                Ok(script) => Ok(Box::pin(futures_util::stream::iter(script))),
                Err(e) => Err(e),
            }
        }
    }

    fn rewire_recording(s: &AgentSession, outcomes: RoundScripts) -> Arc<RecordingProvider> {
        let provider = Arc::new(RecordingProvider {
            outcomes: Mutex::new(outcomes),
            asked: Mutex::new(Vec::new()),
        });
        let mut config = s.engine.lock().unwrap().config.clone();
        config.model = "deepseek-v4.1-flash".into();
        config.max_tokens = None;
        *s.engine.lock().unwrap() = Engine {
            config,
            provider: provider.clone(),
        };
        provider
    }

    /// provider 说「max_tokens 太大了，最多 8192」：降下来重发，别让整轮跑黄。
    /// 用户一个字都不用改，输出短了由续写拼成一条完整回复。
    #[tokio::test]
    async fn a_rejected_ceiling_is_lowered_and_the_run_carries_on() {
        let s = session();
        let provider = rewire_recording(
            &s,
            vec![
                Err(ProviderError::Http {
                    status: 400,
                    body: r#"{"error":{"message":"max_tokens is too large: 65536, maximum allowed is 8192"}}"#.into(),
                }),
                Ok(cut("前半段")),
                Ok(done("后半段")),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx)
            .await;
        let flow = drain(rx);

        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert!(flow.completed, "降级之后该正常跑完");
        assert_eq!(
            provider.asked.lock().unwrap().as_slice(),
            &[65536, 8192, 8192],
            "被拒之后每一发都得带上新上限"
        );
        assert!(
            flow.statuses.contains(&"agent.lowering_tokens".to_string()),
            "用户该知道我们降了上限：{:?}",
            flow.statuses
        );
        assert_eq!(flow.text, "前半段后半段");
    }

    /// provider 说「说完了」，结尾却断在半截。续写发出去模型只会答「没什么要
    /// 补充的」——那就照原样收场，而不是判成失败，更不能把追问留在历史里。
    #[tokio::test]
    async fn a_finished_but_half_open_reply_gets_one_continue_then_is_kept() {
        let s = session();
        rewire(
            &s,
            vec![
                vec![
                    Ok(LlmEvent::Token("```lua\npset(1, 1, red)\n".into())),
                    Ok(LlmEvent::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ],
                done("没什么要补充的"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx)
            .await;
        let flow = drain(rx);

        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert!(flow.completed, "猜错的那发续写不该把整轮判成失败");
        assert_eq!(
            flow.statuses,
            vec!["agent.continuing", "agent.finished_whole"],
            "续了一发发现没东西可补，就该收场"
        );
        assert_eq!(flow.text, "```lua\npset(1, 1, red)\n没什么要补充的");
        let messages = s.messages.lock().unwrap();
        assert_eq!(messages.len(), 2, "追问指令该被撤掉");
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);
    }
}
