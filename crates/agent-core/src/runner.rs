// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
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
use tokio::time::timeout;

use super::artstyle;
use super::craft;
use super::imagegen::{self, ImageGenParams, LandSpot};
use super::intent;
use super::knowledge;
use super::limits;
use super::mcp::{self, McpRegistry};
use super::models::{
    ActiveContext, AgentEvent, ApprovalDecision, Attachment, Capabilities, ChatRequest,
    ContentBlock, LlmEvent, Message, ModelConfig, PermissionMode, Role, RunnerConfig, ToolSpec,
    UiText,
};
use super::plan::{self, TurnPlan, PLAN_TOOL};
use super::presets;
use super::prompt;
use super::providers::{self, LlmProvider, ProviderError};
use super::roles::{self, ModelRole, RoleBinding};
use super::tools::{self, ToolOutcome, IMAGE_GEN_TOOL};
use pixel_core::decode;
use pixel_core::document::Document;
use pixel_core::DocPatch;

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

/// 上一发跑成的 shader 的身份与当时的 revision。见 `AgentSession::last_shader`。
struct LastShader {
    /// 一次 shader 请求的身份：script 原文 + 目标 cel + 是否逐帧。
    /// 带上这几项是因为同一份 script 画到另一帧、另一个图层上就是另外
    /// 一堆像素，不能算重放；只留 script 会误伤。
    key: String,
    /// 这一发跑完之后 document 的 revision。任何别的改动都会挪它，
    /// 挪开了这份记忆就自然失效——包括用户在编辑器里动过的手笔。
    revision: u64,
    /// 这一发跑成了没有。跑挂的那份同样值得记住：原样重发一遍坏的脚本，
    /// 结果只会再坏一次，而完整问一轮模型要几十秒到两分钟。
    ok: bool,
}

/// 整个响应流允许静默多久。
///
/// 想得久一点没关系——推理增量、心跳、工具入参分段都会带来字节；
/// 这么久一个字节都没有，基本就是连接假死（代理把流吞了、对端不吭声挂了）。
/// 不设这条线的话 `stream.next()` 会永远挂住，前端只剩一个思考节点空转。
const STREAM_IDLE_LIMIT: Duration = Duration::from_secs(300);

/// 发出去的请求多久之内必须见到响应头。
///
/// `provider.request()` 里那段 `.send().await` 在拿到响应头之前不算流：
/// 流静默看门狗罩不住它，而 reqwest 的连接超时也罩不住「连接建好了、
/// 网关却不吭声」这一段。中转站把请求挂在半开连接上是常态，没有这条线
/// 的话，重试状态发出去之后整个 turn 就静默死掉了。超时按网络故障重发，
/// 等一个退避再试常常就通。
const HEADERS_TIMEOUT: Duration = Duration::from_secs(45);

/// 生图一次最多等多久。
///
/// 出图本就比聊天慢，但端点挂住不返图时整轮就静默死掉。超时不重发——
/// 图都画不出来，原样再要一次大概率还是不出来；把丑话回给模型，它换个
/// 写法或者如实告诉用户，都比让 turn 卡死强。
const IMAGE_GEN_TIMEOUT: Duration = Duration::from_secs(180);

/// 取消标志的轮询间隔。停止按钮的手感全看这一条：间隔越长，用户点了
/// 停止之后界面「没反应」的时间就越长。await_approval、流式收尾、
/// 以及长工具的可中断等待共用它，别各处写死一个数。
const CANCEL_POLL: Duration = Duration::from_millis(120);

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

/// 同一份 shader 原样重跑时回给模型的话。要说清「跳过不等于失败」：
/// 不然它会以为工具坏了，换个写法再发一遍同样的东西。
const SHADER_REPLAY_REFUSAL: &str = "this exact shader script already ran and painted these pixels; the canvas has not changed since; the run was skipped. Verify with pixel_read_canvas if you need the current grid. Then either change the script (different geometry, color choices, or target cel) or finish your reply with a one-sentence summary - re-running an identical script cannot produce a different canvas";

/// 一轮提前收摊时，给没跑成的调用补的 `tool_result`。
///
/// 消息簿里每一条 `ToolUse` 都必须有配对的 `ToolResult`，否则下一轮请求会被
/// 服务端整包 400 拒掉（"tool_use ids were found without tool_result blocks"）。
/// 少了这一步，用户点一次停止、或撞上一次步数预算，这个会话就再也发不出
/// 任何消息了——只能删掉重建。
const UNFINISHED_TOOL_RESULT: &str = "this tool call was never executed because the turn ended first (stopped by you, out of tool steps, or out of approval). It painted nothing. Resend it unchanged if you still need it.";

/// 把这一批没跑完的调用补成带错误的 `tool_result`。
///
/// `from` 之后的一律没执行（跑到一半停下、连错三次收摊）。已经配过结果的一笔
/// 不多补：空结果会让模型以为工具坏了，换个写法重发一遍同样的东西。
fn unfinished_tool_results(ids: &[String], from: usize) -> Vec<Message> {
    ids[from..]
        .iter()
        .map(|id| Message::tool_result(id.clone(), UNFINISHED_TOOL_RESULT, true))
        .collect()
}

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
    // 一行以连接符或未闭合的括号收尾：表达式、参数表、对象字面量都被从中间
    // 剪了一刀。中文逗号、顿号、冒号同理——没有一句话是拿它们收尾的；半角
    // 冒号也多半是「后面还有内容」的标签。猜错不要紧，见调用处的处理。
    // 中文破折号「——」是断得干净的收尾，碰不到这张表，不会被误伤。
    const DANGLING: &[&str] = &[
        ",", "，", "、", ":", "：", "+", "&&", "||", "=>", "->", "(", "{", "[", "=",
    ];
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
/// 403 走另一条路：它看着像「服务器不答应」，可里面相当一部分是网关侧的限流
/// 窗口、套餐切换、区域策略在作怪，等一等就放行。宁可让用户等十几秒看到同一个
/// 403，也不能把一条其实能跑通的请求判死。
///
/// 响应体自己点明「这个模型不在你的套餐里」的那一半（见 `permanent_reason`）
/// 也不在这张表里问生死：它照样发满这一轮的额度，只是摊牌时说的话不一样。
fn retryable(err: &ProviderError) -> bool {
    match err {
        ProviderError::Network(_) | ProviderError::Decode(_) => true,
        ProviderError::Http { status, .. } => {
            matches!(*status, 403 | 408 | 409 | 425 | 429) || (500..=599).contains(status)
        }
        ProviderError::Config(_) => false,
    }
}

/// 命中这些词，就是「这句话不管再听几遍都一样」——但还是要听满这一轮的几遍。
///
/// 分三组，各对上一条出路：模型不给用、钱的事、钥匙不对。词全部小写比，
/// 响应体里的 type 字段（`permission_denied_error` 一伙）不带 message 时不收——
/// 光一个类型码分不清是套餐还是网关限流窗口，宁可让它走重试那条路。
///
/// 认出来不等于当场摊红字：中转站侧套餐生效有延迟、网关缓存旧策略也是等一等
/// 就好的事，所以照样先给满这一轮的重发额度。认出来的价值在收场那句话——额度
/// 见底时说的是「换模型、改套餐、改 key」，而不是一句「重试次数用尽」，
/// 用户不知道该动哪里。判 Permanent 的语义只留一半：只管说什么，不管发几次。
const PERMANENT_MARKERS: &[&str] = &[
    // 模型这个端点上根本不给用：名打错、没订阅、套餐没覆盖。
    "model is not available",
    "not available in the current",
    "token plan",
    "model_not_found",
    "model does not exist",
    "does not exist",
    "unknown model",
    "no such model",
    // 钱的事：配额见底、账单没生效。
    "insufficient_quota",
    "quota_exceeded",
    "current quota",
    "billing",
    // 钥匙不对。
    "invalid_api_key",
    "incorrect api key",
    "api key not valid",
    "access denied",
];

/// 从一段响应体里读出「这发重发了也没用」。带一句出路，用户该动哪里。
fn permanent_reason_of(body: &str) -> Option<String> {
    let low = body.to_lowercase();
    PERMANENT_MARKERS
        .iter()
        .find(|marker| low.contains(**marker))
        .map(|marker| {
            match *marker {
                "billing" | "insufficient_quota" | "quota_exceeded" | "current quota" => {
                    "the provider says this account cannot pay for the call; check the plan or credit, or switch model"
                }
                "invalid_api_key" | "incorrect api key" | "api key not valid" | "access denied" => {
                    "the provider rejected the API key; fix it in Settings > Models, or switch model"
                }
                _ => {
                    "the provider will not serve this model name; check the model id in Settings > Models, or switch model"
                }
            }
            .to_string()
        })
}

/// 一眼认出来「这句话不管再听几遍都一样」的那类报错：模型不让用、钱的事、
/// 钥匙不对。判据全在响应体里，不在状态码里——403 既可能是网关限流窗口
/// （等一等就放行），也可能是「这个模型不在你的 token 套餐里」，两者的报文
/// 长得一模一样，只有字里那半句实话分得开。
///
/// 认出来只改变一件事：摊牌时说什么。发几次照旧走满这一轮的重发额度——
/// 中转站侧套餐生效有延迟、网关缓存旧策略也是同一句 403 等一等就好的事，
/// 用户要的是「红字之前真试过五次」。五次都撞回同一句话，才把 `Refused` 连同
/// 出路（换模型 / 改套餐 / 改 key）交出去。
///
/// 真正一次都不该重发的是另一类：请求本身不被接受（400 参数错、401 钥匙格式
/// 就不对且报文里没有实话）。那类由 `retryable` 否决，和这里井水不犯河水。
fn permanent_reason(err: &ProviderError) -> Option<String> {
    let ProviderError::Http { status, body } = err else {
        return None;
    };
    if !(400..500).contains(status) {
        // 5xx 是服务端自己的事，再难看也值得再发一次。
        return None;
    }
    permanent_reason_of(body)
}

/// 同一个失败原样重来，到第几次认栽。
///
/// Provider 已经把那句话说尽了：第二遍、第三遍都是同一句、同一个码。再发下去，
/// 用户看到的是一个不动的界面加一条标语，跟「卡死了」分不开。
const IDENTICAL_FAILURE_LIMIT: usize = 3;

/// 整个逻辑轮的重试退避总时长封顶。
///
/// 网络抖一小串能拖掉半分钟：五次退避 1+2+4+8 就是 15 秒，而续写会让同一个回合里
/// 发出去几十次请求，每发都另给一份额度的话，一个回合可以有一整分钟没有新内容。
/// 聊天回合不该这样，所以整轮共用一个总时长。
const RETRY_WALL_LIMIT: Duration = Duration::from_secs(30);

/// 一次失败之后怎么办。三条出路，摊给用户的措辞各不一样。
enum RetryVerdict {
    /// 原样再发一次。带第几次，界面要显示 `attempt/max`。
    Again(usize),
    /// 重发治不好：换模型、改套餐、改 key。出路写在 payload 里。
    Refused(String),
    /// 病理上该重发，但这一轮的额度/时长/复读见顶了。同样要说明白为什么收。
    Halted(String),
    /// 请求本身不被接受（400/401/404…），照原有的红卡片摊出去。
    NotRetryable,
}

/// 一轮里「还发不发」的三条闸，外加一眼认出「重发也没用」。
///
/// 归整轮管，不归每一发管：续写会让同一个回合里发出几十次请求，每发都另给五次
/// 重发额度，用户就能对着一行 403 看三分钟——那正是「触发了重试，却像根本没动」
/// 的来路。一次成功会把次数和复读归零，但睡掉的时长不还：时间是真花掉了。
#[derive(Debug, Default, Clone)]
struct RetryGate {
    /// 已经吃掉几次重发额度。
    used: usize,
    /// 退避累计睡了多久。
    slept: Duration,
    /// 连着几次一模一样的失败文本。
    streak: usize,
    /// 上一次的失败文本，拿来比「是不是同一句」。
    last: String,
}

impl RetryGate {
    /// 拿着 `ProviderError` 裁一次。流式失败走 `judge_text`。
    fn judge(&self, err: &ProviderError, max: usize) -> RetryVerdict {
        self.judge_known(max, permanent_reason(err), retryable(err))
    }

    /// 手里只剩一句成败文案时裁一次（流半道断、模型假死都走这条）。
    ///
    /// 响应体裹在同一句里，照样认得出「这句话再听几遍也一样」：不认的话，
    /// 套餐不覆盖模型这件事摊牌时只会说「重试次数用尽」，用户不知道去换模型。
    fn judge_text(&self, failure: &str, retryable: bool, max: usize) -> RetryVerdict {
        self.judge_known(max, permanent_reason_of(failure), retryable)
    }

    /// 判定总入口。`known` 是「这句话再听几遍也一样」的出路说明。
    ///
    /// `Some` 时照旧给满 `max` 次重发，只是复读闸对它关门：同一句 403 回五遍正是
    /// 预期中的样子，按复读闸第三遍就收，用户要的那五次永远等不到。次数或时长
    /// 见底才交 `Refused`——红字得押一句能自己走出去的话，`Halted` 那句
    /// 「这一轮不再兜了」只对瞬态错有意义，说给一个 key 写错的用户听，
    /// 他只会把同一条消息再发一遍。
    ///
    /// `None` 时是老规矩：病理上不该重发的一次都不重发。
    fn judge_known(&self, max: usize, known: Option<String>, retryable: bool) -> RetryVerdict {
        let Some(why) = known else {
            if !retryable {
                return RetryVerdict::NotRetryable;
            }
            return self.judge_gate(max, false);
        };
        match self.judge_gate(max, true) {
            RetryVerdict::Again(attempt) => RetryVerdict::Again(attempt),
            _ => RetryVerdict::Refused(why),
        }
    }

    /// 三条闸：次数、复读、总时长。病理上该重发的才走到这儿。
    ///
    /// `known_permanent` 为真时关掉复读闸，理由见 `judge_known`。
    fn judge_gate(&self, max: usize, known_permanent: bool) -> RetryVerdict {
        if self.used >= max {
            return RetryVerdict::Halted(format!(
                "the {max} retries this round allows are used up"
            ));
        }
        if !known_permanent && self.streak >= IDENTICAL_FAILURE_LIMIT {
            return RetryVerdict::Halted(format!(
                "the provider answered with the same failure {IDENTICAL_FAILURE_LIMIT} times in a row"
            ));
        }
        if self.slept >= RETRY_WALL_LIMIT {
            return RetryVerdict::Halted(format!(
                "this round already spent {}s backing off",
                RETRY_WALL_LIMIT.as_secs()
            ));
        }
        RetryVerdict::Again(self.used + 1)
    }

    /// 记下这一发失败、即将再发一次。
    fn note(&mut self, attempt: usize, failure: &str, backoff: Duration) {
        self.used = attempt;
        self.slept += backoff;
        if self.last == failure {
            self.streak += 1;
        } else {
            self.last.clear();
            self.last.push_str(failure);
            self.streak = 1;
        }
    }

    /// 这一轮收到了东西：次数与复读重新算，睡掉的时长不还。
    fn succeed(&mut self) {
        self.used = 0;
        self.streak = 0;
        self.last.clear();
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
        self.push_window(written);
    }

    /// 把新写的并进窗口，头部超长就削掉；被削出去的候选「总分」作废。
    ///
    /// 候选存的是窗口里的下标。削窗口不挪它们，下一次比对就全对在错位置上：
    /// 该认的复读认不出，不该认的反而被咽掉。作废只会让下一个字走「当新内容
    /// 放行」的保守分支，不会删掉真东西，所以宁可作废也不留着错下标。
    fn push_window(&mut self, written: &str) {
        self.window.extend(written.chars());
        let drained = keep_tail(&mut self.window, ECHO_WINDOW);
        if drained > 0 {
            self.cands.retain_mut(|i| {
                if *i >= drained {
                    *i -= drained;
                    true
                } else {
                    false
                }
            });
        }
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
            self.push_window(&fresh);
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
/// 返回被削掉的字符数——调用方手上若有指向窗口的下标，得照这个数一起挪。
fn keep_tail(text: &mut Vec<char>, keep: usize) -> usize {
    let total = text.len();
    if total <= keep {
        return 0;
    }
    text.drain(..total - keep);
    total - keep
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
    // 走到这儿：没在复读，也够长了。前面两条已经把不往前走的都挡了，
    // 这里剩下的只有「算推进」一种答案。
    true
}

/// 重试退避：1s、2s、4s、8s，之后封顶。失败不该把用户晾在原地干等。
fn retry_backoff(attempt: usize) -> Duration {
    Duration::from_millis(1000u64 << attempt.saturating_sub(1).min(3))
}

/// 发一次请求，响应头要在 `budget` 之内见到，否则按网络故障交回。
///
/// `provider.request()` 里拿到响应头之前那段不算流：流静默看门狗罩不住它
/// （流还没开始），reqwest 的连接超时也罩不住「连接建好了、网关却不吭声」
/// 这一段。中转站把请求挂在半开连接上是常态——用户看到的「正在重试 1/5」
/// 之后一片死寂，绝大多数就是死在这里：没有这条线，那一发请求永远不回来。
/// 超时按 `Network` 交回，好让原有的重试路径（退避、五次封顶）原样接住，
/// 而不是在重试小循环里另开一套收尾逻辑。
async fn request_with_headers_timeout(
    provider: &dyn LlmProvider,
    request: &ChatRequest,
    budget: Duration,
) -> Result<providers::EventStream, ProviderError> {
    match timeout(budget, provider.request(request)).await {
        Ok(res) => res,
        Err(_) => Err(ProviderError::Network(format!(
            "no response headers within {}s, the connection looks stalled",
            budget.as_secs()
        ))),
    }
}

/// 重发前的退避等待。
///
/// 测试里整段跳过：一组五次重试真要等 23 秒，而假 provider 的剧本一秒就能走完。
#[cfg(test)]
async fn pause_before_retry(_attempt: usize, _cancelled: &AtomicBool) {}

#[cfg(not(test))]
async fn pause_before_retry(attempt: usize, cancelled: &AtomicBool) {
    // 退避期间用户点了停止，就别让人对着一个没反应的按钮干等八秒：
    // 拆成小片轮询取消标志，醒了立刻收。
    let left = retry_backoff(attempt);
    let mut waited = Duration::ZERO;
    while waited < left && !cancelled.load(Ordering::SeqCst) {
        let slice = (left - waited).min(Duration::from_millis(100));
        tokio::time::sleep(slice).await;
        waited += slice;
    }
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
    /// 上一发真的跑过的 shader：它的身份（script + 目标 cel + 是否逐帧）
    /// 和跑完那一刻的 revision。画布再没有任何别的动静，同一份 shader
    /// 原样重来只会画出同一堆像素——实测里模型会在「读画布→重跑→再读」
    /// 的转圈里把整轮预算烧干净，用户什么都拿不到。开新 turn 就忘掉：
    /// 用户开口要「再来一遍」时不许拿上一轮的 script 误伤。
    last_shader: Mutex<Option<LastShader>>,
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
    /// 一个 turn 一次占用。并发两个 turn 会互相踩：会话历史被双份写入、
    /// 取消标志互相顶、turn 预算被对半砍。前端挡住了重复发送，但 MCP、
    /// 脚本这些入口不受那层约束，所以这里再兜一次。
    busy: AtomicBool,
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
    /// 这一轮的提示词清单：模型在动笔前用 pixel_prompt 写下的正向/逆向两段。
    /// 开 turn 时清空——清单跟着这一单的图走，不跨轮留用。写没写是硬流程：
    /// 生图工具在它为空时第一次被挡下，第二次按用户原话兜底放行。
    craft: Mutex<Option<craft::CraftedPrompt>>,
    /// 用户在编辑器里动手的痕迹（一句话一条）。下一轮请求前冲刷成一条 user 消息，
    /// 让模型知道「画面已经被人改过了」，别照着自己上一轮的想象继续画。
    pending_edits: Mutex<Vec<String>>,
    /// 上一次广播给前端的那份文档。增量广播的基准。
    ///
    /// 前端本地持有一份完整文档（撤销要整份回传），所以这里只需记住
    /// 「它上次看到的是什么」，下次 emit 之前 diff 出变化的那几个 cel。
    /// None = 还没广播过，第一次给全量。整份 clone 约 10ms，换来的是
    /// 不再把 47MB 的 JSON 灌进 IPC。
    broadcast: Mutex<Option<Document>>,
}

/// turn 占用标记。析构即放手：turn 里哪条 return、panic、提前 drop 掉，
/// 会话都不会被永久钉在「忙」上，用户下一句还发得出去。
struct BusyGuard<'a>(&'a AtomicBool);

impl<'a> BusyGuard<'a> {
    /// 抢得到才 Some。抢不到的调用方当场回话，别两个 turn 交叉写同一份历史。
    fn try_acquire(flag: &'a AtomicBool) -> Option<Self> {
        if flag.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(Self(flag))
        }
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
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
            busy: AtomicBool::new(false),
            approval: Mutex::new(None),
            mcp: Mutex::new(None),
            title: Mutex::new(None),
            order: AtomicU64::new(0),
            ref_nodes: AtomicU64::new(0),
            plan: Mutex::new(TurnPlan::default()),
            craft: Mutex::new(None),
            pending_edits: Mutex::new(Vec::new()),
            last_shader: Mutex::new(None),
            broadcast: Mutex::new(None),
        }
    }

    pub fn with_runner_config(mut self, config: RunnerConfig) -> Self {
        *self.runner_config.get_mut().unwrap() = config;
        self
    }

    /// 给会话一个显示名。空串和纯空白都当没有：创建时用户可能清空了输入框。
    pub fn with_title(self, title: Option<String>) -> Self {
        self.set_title(title);
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
        *self
            .mcp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = registry;
    }

    /// 这个会话现在接不接外部工具。给总开关和诊断用：不把注册表本身交出去，
    /// 想摘它的人只能走 set_mcp_registry。
    pub fn mcp_attached(&self) -> bool {
        self.mcp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
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
        self.title
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_title(&self, title: Option<String>) {
        let trimmed = title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        *self
            .title
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = trimmed;
    }

    /// 记一笔「用户在编辑器里动了什么」。空文本不入簿：一条空行只会
    /// 让下一轮的上下文更啰嗦。簿子有上限，超长的历史会话不会把它撑爆。
    pub fn note_edit(&self, note: impl Into<String>) {
        let note = note.into();
        if note.trim().is_empty() {
            return;
        }
        let mut edits = self
            .pending_edits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 记到上限就把最旧的一条挤掉：越近的改动对「下一步画什么」越有参考价值。
        if edits.len() >= MAX_PENDING_EDITS {
            edits.remove(0);
        }
        edits.push(note);
    }

    /// 取走所有待冲刷的改动记录。取走即清空，没人会读第二遍。
    pub fn take_edits(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .pending_edits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// 这份 shader 身份是不是刚跑过、跑完到现在画布又没动过。命中就交回
    /// 「那一发跑成了没有」，没命中是 None。
    /// revision 对不上就是画布动过了（模型、用户、撤销都算），记忆自然失效。
    fn remembered_shader(&self, key: &str, revision: u64) -> Option<bool> {
        self.last_shader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|last| last.key == key && last.revision == revision)
            .map(|last| last.ok)
    }

    /// 刚跑的这份 shader 记下来，跑成跑挂都记。只有 `pixel_run_shader` 会走到这儿。
    fn remember_shader(&self, key: String, revision: u64, ok: bool) {
        *self
            .last_shader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(LastShader { key, revision, ok });
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(role, Engine { config, provider });
    }

    /// 取消某个角色的单独绑定，让它回落去蹭主模型。没绑过就是空操作。
    pub fn clear_role(&self, role: ModelRole) {
        self.role_engines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&role);
    }

    /// 这个角色实际该用哪个模型配置：单独绑了就用它，否则用主模型。
    /// 单模型用户什么都没配，所以拿到的永远是主模型，行为和以前一致。
    pub fn model_for_role(&self, role: ModelRole) -> ModelConfig {
        self.role_engines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&role)
            .map(|engine| engine.config.clone())
            .unwrap_or_else(|| self.model_config())
    }

    /// 四个角色各自实际在干的活，给 UI 摆「谁负责哪段流程」。
    /// 没有分工引擎的会话也照答：回落主模型，`detached` 是 false。
    pub fn role_bindings(&self) -> Vec<RoleBinding> {
        let primary = self.model_config();
        let roles = self
            .role_engines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        for engine in self
            .role_engines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|r| r.specs())
            .unwrap_or_default()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn model_config(&self) -> ModelConfig {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .config
            .clone()
    }

    pub fn runner_config(&self) -> RunnerConfig {
        self.runner_config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// 这一轮点名的风格预设。`None` = 用户没提，生图提示词该挂默认质量档。
    ///
    /// 锁只借一瞬就放：风格预设开 turn 时定一次，读它只是为了在提示词出门前
    /// 挑一段渲染规矩。await 期间持着不放会把 pixel_plan 的改写和整条主循环
    /// 一起堵在门外，而那正是用户嘴里「卡住不动」的意思。
    fn current_style(&self) -> Option<artstyle::ArtStyle> {
        self.plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .style
    }

    /// 本轮的收尾预设，和 `current_style` 一样只借一瞬就放。
    ///
    /// 这两份东西一起才构成「这一张图按什么规矩收尾」：画风管色数与描边，
    /// 预设管形体、材质、细节落在哪。生图模型那头一样都想要。
    fn current_presets(&self) -> Vec<&'static presets::Preset> {
        self.plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .presets
            .clone()
    }

    /// 生图提示词出门前挂上这一轮的渲染规矩。单独拆一个出口是为了能单测：
    /// 生图本体要打网络，这一段是这一路上唯一纯函数的判断。
    fn image_prompt(&self, prompt: &str) -> String {
        plan::image_prompt(prompt, self.current_style(), &self.current_presets())
    }

    pub fn set_runner_config(&self, config: RunnerConfig) {
        *self
            .runner_config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = config;
    }

    /// 运行时切换模型：重建 provider，保留文档与消息历史。
    pub fn rebind_provider(&self, config: ModelConfig) {
        let provider = providers::build_provider(&config);
        *self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Engine { config, provider };
    }

    /// 用外部文档整体替换当前文档（前端加载 .aip 或撤销后同步回来）。
    /// 激活图层/帧若已不存在则回落到首个，避免后续工具调用落空。
    pub fn sync_document(&self, document: Document) {
        // 前端发来的整份文档就是它本地看到的最新画面。把广播基准也换过去，
        // 否则下一次 emit 会拿旧基准 diff 出一份全量 patch，而前端刚刚才把
        // 这份文档交上来——白推几十 MB 还把它本地的选择状态冲掉。
        *self
            .broadcast
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(document.clone());
        let mut doc = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = active;
    }

    pub fn active(&self) -> ActiveContext {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_permission(&self, permission: PermissionMode) {
        self.runner_config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .permission = permission;
    }

    /// 用户对一条挂起的工具调用给出决定。call_id 对不上说明这是过期决定
    /// （新 turn 已经开始），直接拒绝而不是把它送进死队列。
    pub fn resolve_approval(
        &self,
        call_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        let slot = self
            .approval
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
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
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// 历史消息条数，不克隆内容。
    ///
    /// 上面那个 `history()` 会把整份对话（含每条工具结果）拷一份出来：会话持久化
    /// 每隔两秒就要问一次「历史有没有变长」，用它在轮询路径上等于每两秒把整场对话
    /// 复制一遍。条数这个答案足够判断「要不要落盘」了。
    pub fn history_len(&self) -> usize {
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub fn load_history(&self, messages: Vec<Message>) {
        *self
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = messages;
    }

    pub fn clear_history(&self) {
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub fn document(&self) -> Document {
        self.document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// 借出文档做只读的一次性计算（渲染垫图、导出预览）。
    /// 和 `with_document_mut` 用同一把锁，但不会 bump revision，也不会让外界改到文档。
    pub fn with_document<T>(&self, f: impl FnOnce(&Document) -> T) -> T {
        let doc = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&doc)
    }

    /// 借出文档做一次性原地修改（插帧、量化、编辑器操作），完成后返回新 revision。
    /// 主循环的 agent turn 也走同一把锁，所以这里不会和并发 turn 交织。
    pub fn with_document_mut<T>(&self, f: impl FnOnce(&mut Document) -> T) -> T {
        let mut doc = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut doc)
    }

    pub fn document_json(&self) -> Value {
        let doc = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        serde_json::to_value(&*doc).unwrap_or(Value::Null)
    }

    /// 取一份相对上一次广播的文档增量，并把基准推进到当下。
    ///
    /// 广播不是替换：前端拿 patch 合并进本地那份完整文档，所以这里只报
    /// 元数据和变化过的 cel。锁内一次 clone 当新基准，文档在两次广播之间
    /// 被改了多少次都无所谓——下一次 emit 自然把改动 diff 出来。
    ///
    /// 纯内存计算，没有会失败的序列化步骤，所以不会有「发一份坏 patch 出去」
    /// 的可能——原来那份 unwrap_or(Value::Null) 的兜底正是要消掉的东西：
    /// 前端拿到 Null 会当场 hexesOf(null) 炸掉，界面整个冻住。
    pub fn document_patch(&self) -> DocPatch {
        let mut baseline = self
            .broadcast
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let doc = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let patch = match baseline.as_ref() {
            None => DocPatch::full(&doc),
            Some(prev) => DocPatch::diff(prev, &doc),
        };
        *baseline = Some(doc.clone());
        patch
    }

    pub fn revision(&self) -> u64 {
        self.document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision
    }

    /// 序列化成 .aip v2 文本（中间文件格式不变）。
    pub fn aip_text(&self) -> Result<String, String> {
        let doc = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pixel_core::context::to_aip(&doc)
    }

    pub fn interrupt(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        // 有审批挂着的话，发送端一掉，等待中的主循环立刻收手，不会和用户赌手感。
        self.approval
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// 把一段「可能要等很久」的等待切成可中断的。
    ///
    /// 生图要等模型回图、外部 MCP 工具要等别人的服务器，两者都可能挂上几分钟。
    /// 只在外层循环查取消标志的话，用户点了停止之后界面一动不动，一直干等到
    /// 各自的超时——看上去就是停止键失灵。这里按 CANCEL_POLL 轮询取消标志，
    /// 一中断立刻回 None，调用方走 bail 收摊，消息簿照样配得上对。
    async fn until_cancelled<F>(&self, fut: F) -> Option<F::Output>
    where
        F: std::future::Future,
    {
        tokio::pin!(fut);
        let mut tick = tokio::time::interval(CANCEL_POLL);
        loop {
            tokio::select! {
                out = fut.as_mut() => return Some(out),
                _ = tick.tick() => {
                    if self.is_cancelled() {
                        return None;
                    }
                }
            }
        }
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
            let mut slot = self
                .approval
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let mut tick = tokio::time::interval(CANCEL_POLL);
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
    /// `pinned_style` 是用户在界面上钉住的风格（点了一下风格预设）：它压过
    /// 从原话里读出来的风格，且整轮有效，直到用户改主意。
    /// 不带提示词预设的一个 turn；要带预设走 `run_turn_with_preset`。
    pub async fn run_turn(
        &self,
        text: String,
        attachments: Vec<Attachment>,
        tx: UnboundedSender<AgentEvent>,
        pinned_style: Option<artstyle::ArtStyle>,
    ) {
        self.run_turn_with_preset(text, attachments, tx, pinned_style, Vec::new())
            .await;
    }

    /// 带内置提示词预设的一个 turn。预设是用户在输入区点的「这张图按什么规矩
    /// 收尾」：与画风平级，整轮有效，直到用户改主意。可以叠几条同时上路
    /// （上限见 `presets::MAX_STACKED`），叠几条都照样整轮有效。
    pub async fn run_turn_with_preset(
        &self,
        text: String,
        attachments: Vec<Attachment>,
        tx: UnboundedSender<AgentEvent>,
        pinned_style: Option<artstyle::ArtStyle>,
        pinned_presets: Vec<&'static super::presets::Preset>,
    ) {
        // 一个会话同时只跑一个 turn。抢不到就当场回话，别让两条 turn 在
        // 同一个会话里交叉写入消息历史和画布。
        let Some(_busy) = BusyGuard::try_acquire(&self.busy) else {
            emit(
                &tx,
                AgentEvent::Error {
                    message: UiText::new(
                        "agent.busy",
                        "this conversation is still busy with the previous message; wait for it to finish or press stop",
                    ),
                },
            );
            return;
        };

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
        // 改画轴先问画布：上面真有像素才谈得上「改」。空画布上「画只猫，加个项圈」
        // 词面上是改、实际得从零画，那时候该走从零画的分流，而不是带着
        // 「不许 clear()、不许重画」的禁令上路。锁顺序 document -> ...，
        // 这里单独取一次就放，不和其他锁交叉。
        let has_pixels = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has_pixels();
        let plan = TurnPlan::from_text_pinned(
            &text,
            &attachments,
            has_pixels,
            pinned_style,
            pinned_presets,
        );
        *self
            .plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = plan.clone();
        // 上一轮的提示词清单到此作废：这一单的图该配它自己的正向/逆向清单，
        // 留着旧清单等于让上一张画的 brief 约束这一张。
        *self
            .craft
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        // 上一轮那份 shader 记忆到此为止。用户开口要「照这样再来一遍」时，
        // 上一轮的 script 配上没动过的画布会被误判成重放——新开一轮就是
        // 一次重新来过的机会。turn 内的原样重跑才是真毛病，那边有护栏。
        *self
            .last_shader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
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
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Message {
                role: Role::User,
                content,
            });

        // 用户发消息前在编辑器里动过的痕迹，先冲刷成一条 user 消息。
        // 少了这一步，模型会照着自己上一轮的想象继续画，把用户的手笔当成不存在。
        let manual_edits = self.take_edits();
        if !manual_edits.is_empty() {
            self.messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: manual_edit_digest(&manual_edits),
                    }],
                });
        }

        // 进入 turn 时快照一份预算，避免中途改设置导致行为漂移。
        // mut：Ask 模式下用户点「本次放行」会把它降级成 Auto，只影响本 turn。
        let mut runner_config = self.runner_config();
        // 这句话是不是在要图。纯聊天不必催，见 intent::asks_for_artwork。
        let art_requested = intent::asks_for_artwork(&text);
        // 这一轮走不走提示词流程：要图、改画、成品意图已定都算，纯问答不算。
        // 判据只用 Rust 已经定性的东西，不猜模型待会要干什么——猜漏了顶多少
        // 一段清单，猜错了就是每句闲聊都被逼先写提示词。
        let craft_needed = craft::required(plan.editing, plan.intent, art_requested);
        // 清单写没写归会话状态管；本 turn 只记「挡下过一次没有」——第二次仍然
        // 跳过清单的生图调用放行，并如实告诉用户走了兜底。
        let mut craft_warned = false;
        let mut steps = 0usize;
        let mut last_failure: Option<(String, String)> = None;
        let mut failure_streak = 0usize;
        let mut usage_in: Option<u32> = None;
        let mut usage_out: Option<u32> = None;

        // 两个配额：逻辑轮次吃 max_turns 预算，重试封顶五次。续写的额度跟着
        // 单次回复走，见下面循环内的声明——放在 turn 头上会让上一轮用掉的次数
        // 记在这一轮头上，越往后越抠，最后任何回复都只剩半截。
        let mut logical_rounds = 0usize;
        // 整轮共用的重发闸门：次数、退避总时长、同一句报错的复读。见 `RetryGate`。
        let mut retry_gate = RetryGate::default();
        // 整轮零工具调用时催过几次。见 MAX_TOOL_NUDGES。
        let mut tool_nudges = 0usize;

        // 关思考：用户钉死（Some）就照办；没钉死（None）先按模型默认来，
        // 但保留一次自动翻盘的机会——见下面 retry 小循环里的兜底。
        let pinned_thinking = {
            let engine = self
                .engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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

            let provider = self
                .engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .provider
                .clone();
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
            // 续写额度归「这一次回复」管：每一轮重新问模型都带一份新的输出长度
            // 预算，上一轮用掉的五次不该记在这一轮头上。放在 turn 头上的话，
            // 第二轮开始任何回复都只剩半截——用户看到的就是「说到一半不动了」。
            let mut continuations = 0usize;
            // 重试与续写都收在这个小循环里。请求每一发都重装：历史里刚 push 的
            // 半截回复必须在这发请求里生效，不然就是白续一次。
            let mut raw: RoundRaw = loop {
                // 退避睡到一半用户点了中断：别等睡醒再开口，直接收。
                if self.is_cancelled() {
                    emit(&tx, AgentEvent::Interrupted);
                    return;
                }
                let request =
                    self.chat_request(&runner_config, token_ceiling, thinking_off, craft_needed);

                let sent =
                    request_with_headers_timeout(provider.as_ref(), &request, HEADERS_TIMEOUT)
                        .await;

                let stream = match sent {
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
                        // 「失败」摔在用户脸上，按退避重发更有人味。但门只对「换个
                        // 时间就能好」的错开——套餐不覆盖这个模型、key 被退的，
                        // 重发五次只是把同一句报错看五遍，用户盯着一个死界面。
                        let failure = e.to_string();
                        let verdict = retry_gate.judge(&e, runner_config.loop_limits.max_retries);
                        match verdict {
                            RetryVerdict::Again(attempt) => {
                                retry_gate.note(attempt, &failure, retry_backoff(attempt));
                                emit(
                                    &tx,
                                    AgentEvent::Status {
                                        message: retrying_status(
                                            &failure,
                                            attempt,
                                            runner_config.loop_limits.max_retries,
                                        ),
                                    },
                                );
                                pause_before_retry(attempt, &self.cancelled).await;
                                continue;
                            }
                            RetryVerdict::Refused(why) => {
                                emit(
                                    &tx,
                                    AgentEvent::Error {
                                        message: request_refused(&failure, &why, retry_gate.used),
                                    },
                                );
                                return;
                            }
                            RetryVerdict::Halted(ref why) => {
                                emit(
                                    &tx,
                                    AgentEvent::Error {
                                        message: request_halted(&failure, why),
                                    },
                                );
                                return;
                            }
                            RetryVerdict::NotRetryable => {}
                        }
                        emit(
                            &tx,
                            AgentEvent::Error {
                                message: request_failed(&failure),
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
                    // 流半道断、代理掐线、模型假死：这类该退避重发。套餐/ key / 模型名
                    // 不被接受也算在这儿——报文裹在同一句里，见 `judge_text`。
                    let verdict = retry_gate.judge_text(
                        &failure,
                        raw.retryable_failure,
                        runner_config.loop_limits.max_retries,
                    );
                    match verdict {
                        RetryVerdict::Again(attempt) => {
                            retry_gate.note(attempt, &failure, retry_backoff(attempt));
                            emit(
                                &tx,
                                AgentEvent::Status {
                                    message: retrying_status(
                                        &failure,
                                        attempt,
                                        runner_config.loop_limits.max_retries,
                                    ),
                                },
                            );
                            pause_before_retry(attempt, &self.cancelled).await;
                            continue;
                        }
                        RetryVerdict::Refused(why) => {
                            emit(
                                &tx,
                                AgentEvent::Error {
                                    message: request_refused(&failure, &why, retry_gate.used),
                                },
                            );
                            return;
                        }
                        RetryVerdict::Halted(ref why) => {
                            emit(
                                &tx,
                                AgentEvent::Error {
                                    message: request_halted(&failure, why),
                                },
                            );
                            return;
                        }
                        RetryVerdict::NotRetryable => {}
                    }
                    emit(
                        &tx,
                        AgentEvent::Error {
                            message: request_failed(&failure),
                        },
                    );
                    return;
                }
                retry_gate.succeed();
                // 这一轮确实收到了东西，失败额度重新算：下次失误仍该有五次机会。
                // 睡掉的退避时长不还——那时间是真花了的，见 `RetryGate::succeed`。

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
                    let mut engine = self
                        .engine
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
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
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
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

            // 提前收摊的四个出口：取消、步数预算耗尽、审批没人应答、同一个调用
            // 连错三次。共同点是 assistant 那条消息已经把 ToolUse 写进消息簿，
            // 而这一批调用还没跑完。直接 return 会把消息簿留在「有 ToolUse 却
            // 没有 ToolResult」的状态，下一轮请求整包 400，用户之后每句话都发
            // 不出去。所以四个出口统一记在这里，出路在循环外面收。
            let call_ids: Vec<String> = calls.iter().map(|c| c.id.clone()).collect();
            let mut bail: Option<(usize, AgentEvent)> = None;
            for (idx, call) in calls.into_iter().enumerate() {
                if self.is_cancelled() {
                    bail = Some((idx, AgentEvent::Interrupted));
                    break;
                }
                if steps >= runner_config.max_tool_steps {
                    bail = Some((
                        idx,
                        AgentEvent::Error {
                            message: UiText::new(
                                "agent.tool_budget",
                                "this turn ran out of tool steps after {steps} step(s); start a new message to keep going",
                            )
                            .with("steps", steps as u64),
                        },
                    ));
                    break;
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
                            self.messages.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Message::tool_result(
                                call.id.clone(),
                                "the user rejected this tool call. Do not retry it as-is; explain what you were about to do and ask how you should proceed.",
                                false,
                            ));
                            continue;
                        }
                        // 没人再会给这一笔发决定（中断，或挂起被新 turn 顶掉）。
                        Err(()) => {
                            bail = Some((idx, AgentEvent::Interrupted));
                            break;
                        }
                    }
                }

                // 这一发 shader 是不是上一发原样重跑。跳过执行时别推
                // DocumentUpdated：画布一个像素都没动，推上去只会让前端
                // 白刷一次。
                let mut shader_paused = false;
                let mut craft_blocked = false;
                let outcome: ToolOutcome = match &call.parse_error {
                    Some(e) => ToolOutcome {
                        content: format!(
                            "tool call rejected: arguments were not valid JSON ({e}). Resend the same tool call with corrected JSON arguments."
                        ),
                        is_error: true,
                    },
                    None => {
                        // 提示词流程的硬闸：这一轮要生图，而清单还没写。第一次挡下
                        // 并要求先写；第二次仍然跳过清单的调用按用户原话兜底一支基线
                        // 放行——再倔的模型也画得出图，同时如实告诉用户这单没走成
                        // 完整的提示词流程。锁只取一次就放，不和后面的文档锁交叉。
                        let craft_gate = craft_needed
                            && craft::is_drawing_tool(&call.name)
                            && self
                                .craft
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .as_ref()
                        .is_none_or(|crafted| crafted.is_empty());
                        if craft_gate && !craft_warned {
                            craft_warned = true;
                            craft_blocked = true;
                            ToolOutcome {
                                content: craft::craft_first_refusal(),
                                is_error: true,
                            }
                        } else {
                            if craft_gate {
                                let routing = self.plan.lock().unwrap_or_else(std::sync::PoisonError::into_inner).outcome_text();
                                *self.craft.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(craft::fallback(&text, &routing));
                                emit(
                                    &tx,
                                    AgentEvent::Status {
                                        message: craft_fallback_status(),
                                    },
                                );
                            }
                            let registry = self
                                .mcp
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .clone();
                            if call.name.starts_with(mcp::MCP_TOOL_PREFIX) {
                                // 外部工具：分流到用户自配的 MCP 服务器，不进文档锁。
                                match registry {
                                Some(registry) => {
                                    match self
                                        .until_cancelled(registry.dispatch(&call.name, &call.input))
                                        .await
                                    {
                                        // 外部服务器挂住，用户点了停止：立刻收，走 bail。
                                        None => {
                                            bail = Some((idx, AgentEvent::Interrupted));
                                            break;
                                        }
                                        Some(Ok(outcome)) => outcome,
                                        Some(Err(e)) => ToolOutcome {
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
                        } else if call.name == craft::PROMPT_TOOL {
                            // 只写下这一轮的提示词清单，不碰文档、不等模型。
                            self.run_craft(&call.input).await
                        } else if call.name == tools::IMAGE_GEN_TOOL {
                            // 生图要等模型回图，异步跑；await 期间绝不持有文档锁。
                            // 回图可能等上三分钟，中途点停止不能干等到超时才收：
                            // 包一层可中断等待，取消走 bail，消息簿照样配对。
                            match self.until_cancelled(self.run_image_gen(&call.input)).await {
                                Some(outcome) => outcome,
                                None => {
                                    bail = Some((idx, AgentEvent::Interrupted));
                                    break;
                                }
                            }
                        } else {
                            let mut doc = self.document.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            let active = self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            // 同一份 shader 原样重跑，分两种收场。跑成过的那份：
                            // 跳过执行，画布不动，回一句能操作的话；模型照抄三遍
                            // 还不收手，按「同一个调用连错三次」收摊，见下面的
                            // streak。刚跑挂的那份：一字不改再来一遍只可能再挂
                            // 一次，而完整问一轮模型要几十秒到两分钟——实测里
                            // 一个负坐标 bug 就这样白白烧掉四轮，画布还是空的。
                            // 直接收摊，把改措辞的主动权还给用户。
                            let key = shader_key(&call.input);
                            let replay = key.clone().filter(|_| call.name == tools::SHADER_TOOL).and_then(
                                |key| self.remembered_shader(&key, doc.revision).map(|ok| (key, ok)),
                            );
                            match replay {
                                Some((_, true)) => {
                                    shader_paused = true;
                                    ToolOutcome {
                                        content: SHADER_REPLAY_REFUSAL.into(),
                                        is_error: true,
                                    }
                                }
                                Some((_, false)) => {
                                    bail = Some((
                                        idx,
                                        AgentEvent::Error {
                                            message: UiText::new(
                                                "agent.shader_repeat_failure",
                                                "the model resent the exact shader script that just failed, unchanged; stopping so you can rephrase your request",
                                            ),
                                        },
                                    ));
                                    break;
                                }
                                None => {
                                    let outcome = tools::execute(
                                        &mut doc,
                                        &active,
                                        &call.name,
                                        &call.input,
                                    );
                                    // 跑成的、跑挂的都记：跑挂的那份留在记忆里，
                                    // 下一发现样重发就收摊；模型改过一个字符就不算
                                    // 重放，照样放行，所以这不会挡住正常修复。
                                    if call.name == tools::SHADER_TOOL {
                                        if let Some(key) = key {
                                            self.remember_shader(key, doc.revision, !outcome.is_error);
                                        }
                                    }
                                    outcome
                                }
                            }
                        }
                        // 提示词闸门的 else 分支到此收口：放行的调用回到原来的
                        // 分发链上，和被挡下一次的路径完全同一条。
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
                self.messages
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(Message::tool_result(call.id, content, outcome.is_error));

                if outcome.is_error {
                    let key = (call.name.clone(), call.input.to_string());
                    if last_failure.as_ref() == Some(&key) {
                        failure_streak += 1;
                    } else {
                        last_failure = Some(key);
                        failure_streak = 1;
                    }
                    if failure_streak >= 3 {
                        // 这一笔的 ToolResult 上面刚推进消息簿，所以未配对的
                        // 工具从下一笔算起。
                        bail = Some((
                            idx + 1,
                            AgentEvent::Error {
                                message: UiText::new(
                                    "agent.same_call_failed",
                                    "the same tool call failed three times in a row; stopping so you can adjust the request",
                                ),
                            },
                        ));
                        break;
                    }
                } else {
                    last_failure = None;
                    failure_streak = 0;
                }

                // 读操作不改文档，不推 DocumentUpdated。
                // MCP 工具在文档之外跑（不回推文档事件）；读操作本来也不推。
                // 被跳过的 shader 同理：一个像素都没动。
                // 被挡下补写清单的生图调用同理：那一发根本没动笔。
                if !shader_paused
                    && !craft_blocked
                    && call.name != "pixel_read_canvas"
                    && call.name != craft::PROMPT_TOOL
                    && !call.name.starts_with(mcp::MCP_TOOL_PREFIX)
                {
                    // guard 通过也照发：空 patch 无害且便宜，而前端要靠
                    // pendingFrameIndex 被 applyDocument 清掉，跳过空 patch
                    // 会让帧选择一直挂在「正在切」上。
                    let patch = self.document_patch();
                    let revision = patch.revision;
                    emit(&tx, AgentEvent::DocumentUpdated { revision, patch });
                }
            }
            if let Some((unfinished_from, event)) = bail {
                let leftovers = unfinished_tool_results(&call_ids, unfinished_from);
                if !leftovers.is_empty() {
                    self.messages
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .extend(leftovers);
                }
                emit(&tx, event);
                return;
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
        craft_needed: bool,
    ) -> ChatRequest {
        // 提示词清单段单独先算，只拿一次 craft 锁：没写就给「动笔前先写清单」
        // 的要求段，写了就把两段清单作为约束带上路。放在 document/active/engine
        // 三把锁之前取，锁顺序保持 document -> active -> engine -> plan 不变。
        let craft_section = {
            let crafted = self
                .craft
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if !craft_needed {
                String::new()
            } else {
                match &crafted {
                    Some(crafted) if !crafted.is_empty() => craft::bound_section(crafted),
                    _ => craft::required_section(),
                }
            }
        };
        // 分工段也要在 document/active/engine 三把锁之前取：role_bindings()
        // 内部要拿 engine 锁，在锁里再取就是自己锁自己。锁顺序维持
        // document -> active -> engine -> plan 不变。
        let roles_section = roles::prompt_section(&self.role_bindings());
        let doc = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 本轮分流、知识条目和行为准则按当前 plan 现算：模型中途纠正过，
        // 下一发请求就该带着新结论上路，缓存会把纠正吃掉。
        // 锁顺序固定 document -> active -> engine -> plan。
        let (routing, craft_notes, discipline, query) = {
            let plan = self
                .plan
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                plan.prompt_sections(),
                // 按 plan 里的 id 出段，不按原话重检：改画那一轮会把 refine
                // 硬塞进列表，重检会把它又挤掉，而它偏偏是唯一一条讲
                // 「保留已有像素」的条目。
                knowledge::section_from_ids(&plan.knowledge_ids, knowledge::DEFAULT_BUDGET),
                // 行为准则三条另走一份预算：检索那 4 个名额是给技法留的，
                // 让护栏插队的话，「画 5 帧奔跑」那一轮就带不上步态相位表。
                // 纯问答轮次不发，见 `TurnPlan::draws`。
                plan.draws()
                    .then(knowledge::discipline_section)
                    .unwrap_or_default(),
                // 原话单独带出来：两张对照表要按它裁剪，见 colornames::prompt_table_for。
                plan.prompt_query().to_string(),
            )
        };
        ChatRequest {
            system: prompt::build_system_prompt(
                &doc,
                &active.layer,
                &active.frame,
                active.color.as_deref(),
                cfg.canvas_context_chars,
                prompt::PromptExtras {
                    routing: &routing,
                    roles: &roles_section,
                    craft: &craft_section,
                    discipline: &discipline,
                    craft_notes: &craft_notes,
                    query: &query,
                },
            ),
            messages: self
                .messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
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
        let mut tick = tokio::time::interval(CANCEL_POLL);
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
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Message::user_text(nudge));
            return;
        }
        let mut messages = self
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        messages.push(Message::assistant(raw.blocks()));
        messages.push(Message::user_text(nudge));
    }

    /// 撤掉最后那条「继续」指令。
    ///
    /// 猜错了的续写才用得上：模型已经没什么要补充的，那句没被答复的追问留在
    /// 历史里，下一轮只会看到一句悬空的 user 消息。`push_resume` 永远以这条
    /// 指令收尾，所以退一条就够。
    fn undo_resume(&self) {
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop();
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
            && plan.presets.is_empty()
            && plan.knowledge_ids.is_empty()
            && !plan.editing
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
        let mut plan = self
            .plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        if let Some(edit) = updates.edit {
            if plan.editing != edit {
                plan.editing = edit;
                if edit {
                    // 改成「改」时同样得把 refine 带上：它是唯一一条讲
                    // 「保留已有像素」的条目，缺了它「接着改」会被读成重画。
                    plan.editing_hit = Some("the user asked to edit".to_string());
                    if !plan.knowledge_ids.iter().any(|id| id == "refine") {
                        plan.knowledge_ids.insert(0, "refine".to_string());
                        plan.knowledge_ids.truncate(4);
                    }
                    changed.push("this turn now edits what is on the canvas".to_string());
                } else {
                    plan.editing_hit = None;
                    changed.push("this turn now draws from scratch".to_string());
                }
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

    /// 模型写下这一轮的提示词清单。只改会话里的 craft：系统提示词的
    /// CRAFTED PROMPTS 段由它推导，所以写下的那一刻下一发请求就带着清单上路，
    /// 生图工具也立刻放行——流程的松紧全凭这一个字段。
    async fn run_craft(&self, input: &Value) -> ToolOutcome {
        let crafted = match craft::parse(input) {
            Ok(crafted) => crafted,
            Err(e) => {
                return ToolOutcome {
                    content: e,
                    is_error: true,
                }
            }
        };
        *self
            .craft
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(crafted.clone());
        ToolOutcome {
            content: craft::recorded_text(&crafted),
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
            // 生图模型那头没有我们的沙箱规矩，唯一能约束它的就是这段文字。
            // 不挂的后果是按它自己的训练分布交差：动漫脸、过曝高光、塑料渐变，
            // 栅格化落进画布之后，再干净的网格也带着一股不属于像素画的味。
            // 点名了风格就挂风格规矩，没点名就挂默认质量档——两套同时上身，
            // 生图模型只会在两段打架的要求里猜权重。
            prompt: self.image_prompt(&params.prompt),
            size: params.size.clone(),
            reference,
        };
        // 生图比聊天慢得多，但慢也有个头：端点挂住不返图时整轮就静默死掉。
        // 超时按工具错误回给模型，它会改提示词重画或者如实告诉用户。
        let image = match timeout(IMAGE_GEN_TIMEOUT, generator.generate(&request)).await {
            Ok(Ok(img)) => img,
            Ok(Err(e)) => {
                return ToolOutcome {
                    content: format!("{IMAGE_GEN_TOOL}: image generation failed: {e}"),
                    is_error: true,
                }
            }
            Err(_) => {
                return ToolOutcome {
                    content: format!(
                        "{IMAGE_GEN_TOOL}: no image came back within {}s; the endpoint looks stalled - retry with a simpler prompt or a different image model",
                        IMAGE_GEN_TIMEOUT.as_secs()
                    ),
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

    /// 主循环的守门员：跑完、或者崩在半路，都要给这一回合一个收口。
    ///
    /// 前端把 `running` 押在「completed / error / interrupted 三选一必到」上。
    /// 主循环里一个 `unwrap` 崩在边角，那个 future 会就地蒸发，之后没有任何人
    /// 再发事件：占位节点一直闪着光标，点停止只会等到「这个会话还在忙」，
    /// 下一句发什么都撞忙——用户看到的就是「按了没反应、也停不下来」。
    /// panic 的 future 由这里接住，补一个错误收尾，界面当场回到能用的状态。
    pub async fn run_turn_guarded(
        &self,
        text: String,
        attachments: Vec<Attachment>,
        tx: UnboundedSender<AgentEvent>,
        pinned_style: Option<artstyle::ArtStyle>,
    ) {
        let fallback = tx.clone();
        let crashed = futures_util::future::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            self.run_turn_with_preset(text, attachments, tx, pinned_style, Vec::new()),
        ))
        .await;
        if crashed.is_err() {
            emit(
                &fallback,
                AgentEvent::Error {
                    message: UiText::new(
                        "agent.turn_crashed",
                        "that turn stopped unexpectedly and nothing was saved; send it again",
                    ),
                },
            );
        }
    }

    /// 守门员那条路，带上内置提示词预设。界面发消息走这一条：用户点的预设
    /// 必须在同一回合内生效，晚一拍就变成约束了上一张图。
    pub async fn run_turn_guarded_with_preset(
        &self,
        text: String,
        attachments: Vec<Attachment>,
        tx: UnboundedSender<AgentEvent>,
        pinned_style: Option<artstyle::ArtStyle>,
        pinned_presets: Vec<&'static super::presets::Preset>,
    ) {
        let fallback = tx.clone();
        let crashed = futures_util::future::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            self.run_turn_with_preset(text, attachments, tx, pinned_style, pinned_presets),
        ))
        .await;
        if crashed.is_err() {
            emit(
                &fallback,
                AgentEvent::Error {
                    message: UiText::new(
                        "agent.turn_crashed",
                        "that turn stopped unexpectedly and nothing was saved; send it again",
                    ),
                },
            );
        }
    }
}

fn doc_has_layer(doc: &Document, id: &str) -> bool {
    doc.layers.iter().any(|l| l.id == id)
}

fn doc_has_frame(doc: &Document, id: &str) -> bool {
    doc.frames.iter().any(|f| f.id == id)
}

/// 一次 `pixel_run_shader` 请求的身份。不是整套入参：script 才是作画的全部，
/// 但目标 cel（frame / layer）和逐帧跑法（animate）换个对象就是另一堆像素，
/// 只按 script 判重放会误伤「同一份 script 画到另一个图层」。工具本身没有
/// frame 入参，落哪帧由激活帧说了算，而换激活帧必然惊动 revision。
/// 别的工具没有身份，返回 None，重放护栏对它不生效。
fn shader_key(input: &Value) -> Option<String> {
    let script = input.get("script").and_then(|s| s.as_str())?;
    let field = |key: &str| input.get(key).map(|v| v.to_string()).unwrap_or_default();
    Some(format!(
        "{script}|layer={}|animate={}",
        field("layer"),
        field("animate"),
    ))
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
        // pixel_prompt 只是写两段清单文本，碰不到画面，和读回一个性质：
        // 放行。弹审批卡会逼用户为一个文本决定反复点同意。
        PermissionMode::Chat => {
            name != "pixel_read_canvas" && name != PLAN_TOOL && name != craft::PROMPT_TOOL
        }
    }
}

/// 模型第二次仍然不写提示词、按用户原话兜底放行时的状态条。
fn craft_fallback_status() -> UiText {
    UiText::new(
        "agent.craft_fallback",
        "模型没有先写提示词，已按你的原话兜底生成，继续作画",
    )
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

/// 报错里常裹着一整条 URL 或响应体，截一刀免得红卡片把聊天区撑爆。
fn trim_reason(reason: &str) -> String {
    let short: String = reason.chars().take(200).collect();
    if reason.chars().count() > 200 {
        format!("{short}...")
    } else {
        short
    }
}

/// 请求彻底失败时摊在用户面前的那一条。Rust 只说键和原始原因，措辞走字典。
fn request_failed(reason: &str) -> UiText {
    UiText::new("agent.request_failed", "the request failed: {reason}")
        .with("reason", trim_reason(reason))
}

/// 「这发重发了也没用」：报错原样摊出去，另外押一句出路。
///
/// 单纯摔一句 `http 403` 在用户脸上，他下一步只会把同一条消息再发一遍。
/// 带上「换模型、改套餐、改 key」，这一轮才算真的结束。
/// 重试了几次也要带上：不说的话，用户会以为我们一次都没试就把这句话摊出来了
/// ——「触发了重试却像根本没动」的怨气正是这么来的。数字取闸门真花掉的几次，
/// 提前收摊时说的是实际那几次，不是额度本上的那个五。
fn request_refused(reason: &str, why: &str, attempts: usize) -> UiText {
    UiText::new(
        "agent.request_refused",
        "the provider will not run this request ({attempts} retry(s) made): {reason}; {why}",
    )
    .with("reason", trim_reason(reason))
    .with("why", why)
    .with("attempts", attempts as u64)
}

/// 「该重发，但这一轮的闸门见顶了」。说清是哪条闸，外加最后一句报错。
///
/// 用户看到这条该知道两件事：不是模型坏了，是这一轮不再替它兜了；出路通常是
/// 重发，或者去设置里把重发额度调大。
fn request_halted(reason: &str, why: &str) -> UiText {
    UiText::new(
        "agent.request_halted",
        "stopped retrying this round: {why}; last failure: {reason}",
    )
    .with("reason", trim_reason(reason))
    .with("why", why)
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

    /// 候选正比对到一半时把窗口削了，新内容不许被当成复读咽掉。
    ///
    /// 放行出去的字要并回窗口，一超过 ECHO_WINDOW 头部就被削掉。削的时候
    /// 不动 cands，旧下标就往前多指了整段被削的量：下一个字来的时候，'正好
    /// 连到窗口末尾' 这个条件被错位顶成立，于是按住的整截新内容被判成复读
    /// 吃掉——用户看到的就是文字莫名少了一截。
    #[test]
    fn a_trim_while_comparing_must_not_eat_the_text_just_released() {
        let mut stream = EchoStream::new();
        stream.absorb(&"a".repeat(ECHO_WINDOW));
        let mut eaten = 0usize;
        // 第一发：一百个 'Z' 窗口里没有，逐个放行；紧跟的十六个 'a' 在窗口里
        // 开出一批候选就卡住不动。收尾时 'Z' 并回窗口把头部削掉一百个，
        // 而这批候选正是在这一瞬间被削出错的。
        let fresh = stream.feed(
            &format!("{}{}", "Z".repeat(100), "a".repeat(ECHO_MIN)),
            &mut eaten,
        );
        assert_eq!(fresh, "Z".repeat(100), "卡住的那截还没定论，先不放");
        assert_eq!(stream.window.len(), ECHO_WINDOW, "窗口该停在长度上限");
        assert!(!stream.cands.is_empty(), "十六个 'a' 该留下候选");
        // 第二发：对不上的一个字让那截见分晓。新内容必须一字不少地还回来。
        let fresh = stream.feed("Q", &mut eaten);
        let expect = format!("{}Q", "a".repeat(ECHO_MIN));
        assert_eq!(fresh, expect, "削窗口削歪了会把新内容当复读咽掉");
        assert_eq!(eaten, 0, "这一段里没有一个字是复读");
    }

    /// 窗口被削过头之后，候选下标必须还是对的。
    /// 一条很长的回复分小块流进来，一个字都不许被复读过滤器啃掉。
    ///
    /// 真实流式就是几十个字一个 chunk 连着来，削窗口在这条路上反复发生。
    /// 每次削窗口都要顺手改掉一批候选下标，错一次就是整段内容少一截。
    /// 文本用单调递增的四位编号拼：任何一段都是独一份的，过滤器本该
    /// 一个字都不动手——所以这里 assert 的就是「一个都没动手」。
    ///
    /// 和上一条分工不同：上一条把 chunk 边界停在「候选正按住不放」的那一瞬，
    /// 钉的是削窗口错位这一个动作；这一条管的是长文流全程，任何一处开始
    /// 误删都拦得住，改哪儿都不该让它变红。
    #[test]
    fn a_long_fresh_stream_survives_every_window_trim_untouched() {
        let mut stream = EchoStream::new();
        stream.absorb("这是上一轮已经写下的内容，垫满窗口好让后面削得动。");
        let mut eaten = 0usize;
        let mut fresh = String::new();
        for i in 0..2400 {
            let piece = format!("{i:04},");
            let chars: Vec<char> = piece.chars().collect();
            // 61 个字一块，故意不和 5 个字一组的分组对齐：chunk 边界
            // 正是候选被按住、窗口被削掉的地方。
            for chunk in chars.chunks(61) {
                let chunk: String = chunk.iter().collect();
                fresh.push_str(&stream.feed(&chunk, &mut eaten));
            }
        }
        let expect: String = (0..2400).map(|i| format!("{i:04},")).collect();
        fresh.push_str(&stream.seal(&mut eaten));
        assert_eq!(fresh, expect, "长文流被复读过滤器啃掉了一截");
        assert_eq!(eaten, 0, "满满一条新内容里没有一个字是复读");
        assert_eq!(stream.window.len(), ECHO_WINDOW, "窗口该停在上限");
    }

    ///
    /// 这条钉住不变量：每个候选都必须从「真的对得上手上这截」的位置开头。
    /// 下标错位的后果不一定是立刻少字——更多时候是下一次比对全对在错位置上，
    /// 该认的复读认不出、不该认的反被咽掉，所以直接钉住下标本身。
    #[test]
    fn candidates_survive_a_window_trim_staying_pointing_at_the_right_text() {
        let mut stream = EchoStream::new();
        // 先垫满一整个窗口：后面再放行内容出去，头部就非削不可。
        stream.absorb(&"a".repeat(ECHO_WINDOW));
        let mut eaten = 0usize;
        // 前一百个字窗口里一个都没有，逐个放行；最后三个 'a' 开出的候选
        // 就跨过了这一次削窗口。
        let fresh = stream.feed(&format!("{}{}", "Z".repeat(100), "aaa"), &mut eaten);
        assert_eq!(fresh, "Z".repeat(100));
        assert_eq!(stream.window.len(), ECHO_WINDOW, "窗口该停在长度上限");
        assert!(!stream.cands.is_empty(), "三个 'a' 该留下候选");
        let held: Vec<char> = stream.held.chars().collect();
        for &i in &stream.cands {
            let at: Vec<char> = stream.window[i..]
                .iter()
                .copied()
                .take(held.len())
                .collect();
            assert_eq!(at, held, "候选 {i} 对上的内容和手上这截不是一回事");
        }
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
        s.run_turn("接着刚才的继续写".into(), Vec::new(), tx, None)
            .await;
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

    /// 一条 ToolUse 必须有配对的 ToolResult，否则下一轮请求会被服务端整包 400
    /// 拒掉（"tool_use ids were found without tool_result blocks"）。
    ///
    /// 撞上步数预算时最后那几条调用没跑，老代码直接 return，消息簿就停在那个
    /// 残缺状态：这个会话之后每句话都发不出去，只能删掉重建。现在没跑的调用
    /// 会补一条报错的 ToolResult，消息簿始终配对。
    #[tokio::test]
    async fn a_turn_that_runs_out_of_tool_steps_still_pairs_every_tool_use() {
        let s = session().with_runner_config(RunnerConfig {
            max_tool_steps: 2,
            ..RunnerConfig::default()
        });
        rewire(
            &s,
            vec![tool_calls(&[
                ("call_1", "pixel_read_canvas", json!({"what": "grid"})),
                ("call_2", "pixel_read_canvas", json!({"what": "grid"})),
                ("call_3", "pixel_read_canvas", json!({"what": "grid"})),
            ])],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画布上现在有什么".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        assert_eq!(flow.tools.len(), 3, "模型要了三笔就该让用户看见三笔");
        assert_eq!(
            flow.error.as_deref(),
            Some("agent.tool_budget"),
            "撞了步数预算要如实说"
        );

        let messages = s
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 用户一句 + assistant 三条 ToolUse + 三条 ToolResult。
        assert_eq!(messages.len(), 5, "每一笔调用都要有结果：{messages:?}");
        let results: Vec<&str> = messages
            .iter()
            .filter_map(|m| match m.content.first() {
                Some(ContentBlock::ToolResult { tool_use_id, .. }) => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(results, vec!["call_1", "call_2", "call_3"]);
        match &messages[4].content[0] {
            ContentBlock::ToolResult {
                is_error, content, ..
            } => {
                assert!(*is_error, "没跑成的调用不许报成成功");
                assert!(
                    content.contains("never executed"),
                    "要说清这一笔根本没落笔：{content}"
                );
            }
            other => panic!("补的应该是 ToolResult，拿到 {other:?}"),
        }
    }

    /// 一条消息簿里的 ToolUse 有没有配对的 ToolResult，直接决定下一发请求会不会
    /// 被 400 拒掉。上面的场景测试只看一条路径，这里把配对规则本身钉死：
    /// 跑完的不补、没跑的补、补的全是错误结果。
    #[test]
    fn every_unfinished_tool_call_gets_a_paired_error_result() {
        let ids = vec![
            "call_a".to_string(),
            "call_b".to_string(),
            "call_c".to_string(),
        ];

        assert_eq!(
            unfinished_tool_results(&ids, 0).len(),
            3,
            "一笔没跑就三条全补"
        );
        assert_eq!(
            unfinished_tool_results(&ids, 2).len(),
            1,
            "跑完两笔就只补最后一笔"
        );
        assert!(
            unfinished_tool_results(&ids, 3).is_empty(),
            "全跑完就不补：多一条空结果会让模型以为工具坏了"
        );

        for (msg, id) in unfinished_tool_results(&ids, 0).iter().zip(ids.iter()) {
            match &msg.content[0] {
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => {
                    assert_eq!(tool_use_id, id, "每条结果都要对得上那一笔调用");
                    assert!(*is_error, "没跑的调用不能报成成功");
                }
                other => panic!("补的应该是 ToolResult，拿到 {other:?}"),
            }
        }
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
            let script = self
                .scripts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(0);
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(req.disable_thinking);
            self.systems
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(req.system.clone());
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
        let config = s
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .config
            .clone();
        *s.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Engine { config, provider };
        watched
    }

    /// 收集一场 turn 的全部出口：正文、状态键、工具名、是否收尾、报错。
    #[derive(Debug)]
    struct Flow {
        text: String,
        statuses: Vec<String>,
        tools: Vec<String>,
        /// 画布被推送给前端的次数。要断言「这一发根本没动笔」就看它。
        doc_updates: usize,
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
            doc_updates: 0,
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
                AgentEvent::DocumentUpdated { .. } => flow.doc_updates += 1,
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

    /// 一截只说了一个工具调用、正文一个字的回复。
    fn tool_call(id: &str, name: &str, input: Value) -> Vec<Result<LlmEvent, ProviderError>> {
        vec![
            Ok(LlmEvent::ToolUseStart {
                index: 0,
                id: id.into(),
                name: name.into(),
            }),
            Ok(LlmEvent::ToolInputDelta {
                index: 0,
                json_partial: input.to_string(),
            }),
            Ok(LlmEvent::Done {
                stop_reason: "tool_use".into(),
            }),
        ]
    }

    /// 一截连着说了多个工具调用的回复。`index` 必须各不相同，否则累加器会把
    /// 几条调用的参数搅成一团。要的就是「一轮里挤了三条调用」这个形态。
    fn tool_calls(calls: &[(&str, &str, Value)]) -> Vec<Result<LlmEvent, ProviderError>> {
        let mut out: Vec<Result<LlmEvent, ProviderError>> = Vec::new();
        for (index, (id, name, input)) in calls.iter().enumerate() {
            out.push(Ok(LlmEvent::ToolUseStart {
                index,
                id: (*id).into(),
                name: (*name).into(),
            }));
            out.push(Ok(LlmEvent::ToolInputDelta {
                index,
                json_partial: input.to_string(),
            }));
        }
        out.push(Ok(LlmEvent::Done {
            stop_reason: "tool_use".into(),
        }));
        out
    }

    /// 一截只写了提示词清单的回复。要图的一轮都得先过这一步。
    fn craft_call(id: &str) -> Vec<Result<LlmEvent, ProviderError>> {
        tool_call(
            id,
            craft::PROMPT_TOOL,
            json!({
                "positive": "a 64x64 red marker block on the first grid cell, flat colors",
                "negative": "no stray pixels, no smudged edges, no colors outside the palette",
            }),
        )
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
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx, None)
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
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        // 两截该原样接上，中间的续写指令只走历史，不能挤到用户眼前。
        assert_eq!(flow.text, "前半段后半段");
        assert!(flow.completed, "续写之后这一轮该正常收尾");
        assert!(flow.error.is_none());
        assert_eq!(flow.statuses, vec!["agent.continuing".to_string()]);

        let history = s
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
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
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        assert_eq!(flow.text, "通了", "重发一次就该拿到正文");
        assert!(flow.completed);
        assert!(flow.error.is_none());
        assert_eq!(flow.statuses, vec!["agent.retrying".to_string()]);
    }

    /// 一个永远不答话的 provider：`.request()` 挂住不回，模拟网关把请求
    /// 挂在半开连接上。
    struct HangingProvider;

    #[async_trait::async_trait]
    impl providers::LlmProvider for HangingProvider {
        async fn request(
            &self,
            _req: &ChatRequest,
        ) -> Result<providers::EventStream, ProviderError> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn a_request_that_never_answers_becomes_a_retryable_failure() {
        let provider = HangingProvider;
        let request = ChatRequest {
            system: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            max_tokens: 1024,
            temperature: None,
            disable_thinking: false,
            echo_reasoning: false,
        };

        let err = match request_with_headers_timeout(&provider, &request, Duration::from_millis(20))
            .await
        {
            Ok(_) => panic!("挂死的请求必须在预算之内交回失败，不能挂在 run_turn 里"),
            Err(e) => e,
        };

        let msg = err.to_string();
        assert!(
            msg.contains("no response headers"),
            "失败原因要说清是响应头等不来，用户才知道该怪谁: {msg}"
        );
        assert!(
            retryable(&err),
            "响应头超时和网络抖动是同一类故障，必须走退避重试而不是直接判死"
        );
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
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        // 401 重发五次也是同一个 401，还得白等 31 秒。直接现形，让用户去改 key。
        assert!(!flow.completed);
        assert!(flow.text.is_empty());
        assert!(flow.statuses.is_empty(), "不该有重试提示");
        let detail = flow.error_detail.unwrap_or_default();
        assert!(detail.contains("401"), "{detail}");
        assert_eq!(flow.error.as_deref(), Some("agent.request_failed"));
        assert_eq!(
            s.messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "只有用户那一句"
        );
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

    /// 「这发重发了也没用」全凭响应体认，状态码一律不许定罪——5xx 服务端自己的事，
    /// 403 也大半是网关限流窗口。认不出来的代价是把一条跑得通的请求判死，
    /// 认得太宽的代价是用户对着一行红字干等十几次重试，两头都得避。
    #[test]
    fn only_response_bodies_that_say_so_are_refused_outright() {
        // 这些自己把话说尽了：换模型 / 改套餐 / 改 key 才能好。
        for (err, expect) in [
            (
                ProviderError::Http {
                    status: 403,
                    body: r#"{"error":{"message":"model is not available in the current token plan","type":"permission_denied_error"}}"#.into(),
                },
                "switch model",
            ),
            (
                ProviderError::Http {
                    status: 429,
                    body: r#"{"error":{"message":"You exceeded your current quota"}}"#.into(),
                },
                "plan or credit",
            ),
            (
                ProviderError::Http {
                    status: 401,
                    body: r#"{"error":{"message":"Incorrect API key provided"}}"#.into(),
                },
                "API key",
            ),
            (
                ProviderError::Http {
                    status: 404,
                    body: r#"{"error":{"message":"The model `gpt-4o` does not exist"}}"#.into(),
                },
                "model id",
            ),
        ] {
            let why = permanent_reason(&err).unwrap_or_else(|| panic!("{err} 该一眼现形"));
            assert!(why.contains(expect), "{why} 里该有 {expect}");
            // 一眼现形的另一面：retryable 那关也得同时否掉，不然白认。
            assert!(!retryable(&err) || permanent_reason(&err).is_some(), "{err}");
        }

        // 这些一个字都不能冤枉：限流窗口、网关限流、只带类型码的 403、5xx。
        for err in [
            ProviderError::Http {
                status: 403,
                body: r#"{"error":{"code":"7","type":"permission_denied_error"}}"#.into(),
            },
            ProviderError::Http {
                status: 429,
                body: r#"{"error":{"message":"slow down"}}"#.into(),
            },
            ProviderError::Http {
                status: 503,
                body: String::new(),
            },
            ProviderError::Network("reset".into()),
        ] {
            assert!(
                permanent_reason(&err).is_none(),
                "{err} 该照旧走重试：{:?}",
                permanent_reason(&err)
            );
        }
    }

    /// 闸门本身：次数、复读、总时长三条，互不顶替。
    #[test]
    fn the_retry_gate_caps_count_repetition_and_wall_clock() {
        let gate = RetryGate::default();
        let failure = "http 429: slow down";

        assert!(matches!(gate.judge_gate(5, false), RetryVerdict::Again(1)));

        // 次数封顶。
        let mut exhausted = gate.clone();
        exhausted.used = 5;
        assert!(
            matches!(&exhausted.judge_gate(5, false), RetryVerdict::Halted(why) if why.contains("used up")),
            "五次用尽就该收"
        );

        // 同一句报错第三遍。
        let mut repeated = gate.clone();
        repeated.note(1, failure, Duration::ZERO);
        repeated.note(2, failure, Duration::ZERO);
        repeated.note(3, failure, Duration::ZERO);
        assert!(
            matches!(&repeated.judge_gate(5, false), RetryVerdict::Halted(why) if why.contains("same failure")),
            "复读第三遍就该收"
        );
        // 认过相的「这句话再听几遍也一样」不吃复读闸：用户要的是真听满五次，
        // 第三遍就收，那第五次永远等不到。
        assert!(
            matches!(repeated.judge_gate(5, true), RetryVerdict::Again(4)),
            "一眼认出来的错照样要给满次数"
        );

        // 退避总时长。一次成功只还次数，不还时间——时间是真花掉的。
        let mut spent = gate.clone();
        spent.note(1, failure, Duration::from_secs(20));
        spent.succeed();
        spent.note(1, "http 502: gateway", Duration::from_secs(11));
        assert_eq!(spent.slept, Duration::from_secs(31));
        assert!(
            matches!(&spent.judge_gate(5, false), RetryVerdict::Halted(why) if why.contains("backing off")),
            "睡够三十秒也该收"
        );

        // 包装层：手里只有一句成败文案时，照样认得出「重发也没用」。
        let refused = "stream error: http 403: {\"error\":{\"message\":\"model is not available in the current token plan\"}}";
        // 但不再零重试：第一次照样是「再发一次」，出路留到额度见底那一刻说。
        assert!(
            matches!(gate.judge_text(refused, true, 5), RetryVerdict::Again(1)),
            "认过相的错也要先给满这一轮的额度"
        );
        let mut spent_on_it = gate.clone();
        for attempt in 1..=5 {
            assert!(
                matches!(spent_on_it.judge_text(refused, true, 5), RetryVerdict::Again(n) if n == attempt),
                "第 {attempt} 次仍该是重发"
            );
            spent_on_it.note(attempt, refused, Duration::ZERO);
        }
        assert!(
            matches!(spent_on_it.judge_text(refused, true, 5), RetryVerdict::Refused(why) if why.contains("switch model")),
            "五次都撞回同一句，才该连出路一起摊出去"
        );
        let stalled = "no data from the model for 90s, the stream looks stalled";
        assert!(
            matches!(gate.judge_text(stalled, true, 5), RetryVerdict::Again(1)),
            "假死该照旧重发"
        );
    }

    /// 403「套餐不覆盖这个模型」也要先听满五次，再把出路摊给用户。
    ///
    /// 中转站侧套餐生效有延迟、网关缓存旧策略，都是同一句 403 等一等就好的事；
    /// 零重试直接摊红字，用户看到的是「明明刚订阅过却不能用」。所以次数照旧给满，
    /// 变的是收场那句话：五次都撞回同一句，才说「换模型 / 改套餐 / 改 key」，
    /// 而不是一句「这一轮不再兜了」。
    #[tokio::test]
    async fn a_403_the_provider_will_not_waive_still_gets_five_retries_first() {
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
            (0..=MAX_ROUND_RETRIES)
                .map(|_| vec![Err(denied())])
                .collect(),
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert!(
            flow.statuses.len() == MAX_ROUND_RETRIES,
            "红字之前要真听满五次：{:?}",
            flow.statuses
        );
        assert!(
            flow.statuses.iter().all(|k| k == "agent.retrying"),
            "每一次重发都得让人看见：{:?}",
            flow.statuses
        );
        assert!(!flow.completed, "不成不许悄悄收尾");
        // 五次都撞回同一句，收场说的是「这发重发了也没用」加出路，不是「次数用尽」。
        assert_eq!(flow.error.as_deref(), Some("agent.request_refused"));
        let detail = flow.error_detail.unwrap_or_default();
        assert!(detail.contains("403"), "报错要带上原文：{detail}");
        assert!(
            detail.contains("switch model"),
            "报错要押一条出路：{detail}"
        );
    }

    /// 只带类型码的 403 仍旧按重试对待：它分不清是网关限流窗口还是套餐切换，
    /// 宁可让用户等十几秒看到同一个 403，也不能把一条其实能跑通的请求判死。
    #[tokio::test]
    async fn a_bare_403_is_still_retried_before_it_is_shown_to_the_user() {
        let s = session();
        // 报文每一发换个说法：网关侧的限流窗口就是这样，同一个 403、不同附言。
        // 复读闸只拦「一模一样的那句话」，这种照旧拿满五次。
        let window = |n: usize| ProviderError::Http {
            status: 403,
            body: format!(
                r#"{{"error":{{"code":"7","type":"permission_denied_error","message":"gateway window {n}, try again"}}}}"#
            ),
        };
        rewire(
            &s,
            (0..=MAX_ROUND_RETRIES)
                .map(|n| vec![Err(window(n))])
                .collect(),
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert_eq!(
            flow.statuses.len(),
            MAX_ROUND_RETRIES,
            "发得出去的错，五次机会一次不该少"
        );
        assert!(
            flow.statuses.iter().all(|k| k == "agent.retrying"),
            "{:?}",
            flow.statuses
        );
        assert!(!flow.completed, "五次都没成，不许悄悄收尾");
        // 五次用尽之后收摊，说的是「这一轮的额度见底」外加最后一句报错——
        // 比一句干巴巴的「请求没成」强，用户知道该重发还是该改配置。
        assert_eq!(flow.error.as_deref(), Some("agent.request_halted"));
        let detail = flow.error_detail.unwrap_or_default();
        assert!(detail.contains("403"), "报错要带上原文：{detail}");
        assert!(detail.contains("used up"), "要说清是次数见顶：{detail}");
    }

    /// 同一个报错原样重来，第三遍就认栽。
    ///
    /// Provider 已经把那句话说尽了，第四遍、第五遍只是让用户多盯二十秒同一行字，
    /// 跟「卡死了」长得一模一样——这正是「触发了重试却像根本没动」的来路。
    #[tokio::test]
    async fn the_same_failure_three_times_in_a_row_cuts_the_retries_short() {
        let s = session();
        let stalled = || ProviderError::Http {
            status: 429,
            body: r#"{"error":{"message":"upstream stalled, try again later"}}"#.into(),
        };
        // 剧本备够：闸门第三遍就该收，后面几份根本用不到。
        rewire(
            &s,
            (0..=MAX_ROUND_RETRIES)
                .map(|_| vec![Err(stalled())])
                .collect(),
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert_eq!(
            flow.statuses.len(),
            IDENTICAL_FAILURE_LIMIT,
            "同一句报错第三次就收：{:?}",
            flow.statuses
        );
        assert!(
            flow.statuses.iter().all(|k| k == "agent.retrying"),
            "{:?}",
            flow.statuses
        );
        assert!(!flow.completed, "没成不许悄悄收尾");
        assert_eq!(flow.error.as_deref(), Some("agent.request_halted"));
        let detail = flow.error_detail.unwrap_or_default();
        assert!(
            detail.contains("same failure"),
            "要说清是复读闸收的：{detail}"
        );
    }

    /// 无限思考的刹车：连续几轮只吐推理、正文和工具调用一个都没有，就收摊。
    /// 不然模型会把二十次续写全烧在思考上，用户看到的是一个永远不动笔的思考节点。
    #[tokio::test]
    async fn a_model_that_never_stops_thinking_is_cut_off_early() {
        let s = session();
        // 钉住「开着思考」：不然第一次死思考就被自动翻盘接走了，
        // 测的就不是护栏本身。
        s.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .config
            .disable_thinking = Some(false);
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
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
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
        assert_eq!(
            flow.tools,
            vec!["pixel_plan".to_string()],
            "思考到尾模型一个工具都没调。pixel_plan 是开 turn 时我们自己摆的分流节点，\
             不算它动过手：{:?}",
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
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        assert_eq!(flow.text, "换了条路");
        assert!(flow.completed);
        assert_eq!(flow.statuses, vec!["agent.retrying".to_string()]);
    }

    /// 每跑一次就往右挪一格的 script：真的跑几遍， marker 就在第几格。
    /// 用它当探针，一眼看得出哪些发是被跳过的。
    fn marker_walker() -> &'static str {
        "local m = hex('#ff004d') local pos = -1 \
         for x = 0, width - 1 do if pget(x, 0) == m then pos = x end end \
         if pos >= 0 then pset(pos, 0, nil) end pset(pos + 1, 0, m)"
    }

    /// 实测里最烧钱的一种转圈：模型跑完 shader 不满意，就读画布、拿同一份
    /// script 原样重跑，再看、再跑。十几个回合烧在零收益的重复上，用户什么都
    /// 拿不到。护栏：同一份 script、同一个 cel、画布没动过，第二次直接跳过。
    #[tokio::test]
    async fn the_same_shader_rerun_verbatim_is_skipped_and_the_turn_stops() {
        let s = session();
        let script = marker_walker();
        // 要图的一轮得先写提示词清单，不然第一发 shader 会被挡下去补写，
        // 「同一个调用连错三次」的额度就被挡下那一次用掉一次。
        rewire(
            &s,
            vec![
                craft_call("c0"),
                tool_call("c1", "pixel_run_shader", json!({"script": script})),
                tool_call("c2", "pixel_run_shader", json!({"script": script})),
                tool_call("c3", "pixel_run_shader", json!({"script": script})),
                tool_call("c4", "pixel_run_shader", json!({"script": script})),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一个红色方块".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        // 头一发真的跑了；后三发原样重发，发发跳过。照抄三遍还不收手，
        // 按「同一个调用连错三次」收摊，别再往后烧。
        assert_eq!(
            flow.tools,
            vec![
                "pixel_prompt".to_string(),
                "pixel_run_shader".to_string(),
                "pixel_run_shader".to_string(),
                "pixel_run_shader".to_string(),
                "pixel_run_shader".to_string(),
            ]
        );
        assert_eq!(
            flow.error.as_deref(),
            Some("agent.same_call_failed"),
            "转圈得有个头：{flow:?}"
        );
        let doc = s
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let layer = doc.layers[0].id.clone();
        let frame = doc.frames[0].id.clone();
        let row = &doc.cel(&layer, &frame).unwrap().indices[..8];
        assert_ne!(row[0], 0, "第一发真的画上了");
        assert!(
            row[1..].iter().all(|&i| i == 0),
            "跳过的三发不许在画布上留下脚步：{row:?}"
        );
    }

    /// 脚本本身坏（负坐标、除零这类）时模型最爱原样重发。剧本一字不改再来
    /// 一遍只可能再坏一次，而完整问一轮要几十秒到两分钟：实测里一个负坐标
    /// bug 就这样白烧四轮，用户干等两分钟，画布还是空的。所以刚跑挂的同一
    /// 发现样重发，第二次直接收摊，不再往模型那儿送请求。改过一个字符的
    /// 修复不算重放，照样放行。
    #[tokio::test]
    async fn a_failed_shader_resent_unchanged_stops_the_turn() {
        let s = session();
        // 必然报错的脚本：pset 不做裁剪，负坐标直接炸。
        let broken = "pset(0, 0, hex('#ff004d')) pset(-1, 0, hex('#ff004d'))";
        rewire(
            &s,
            vec![
                craft_call("c0"),
                tool_call("c1", "pixel_run_shader", json!({"script": broken})),
                tool_call("c2", "pixel_run_shader", json!({"script": broken})),
                done("这可不该被拿到"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一个红色方块".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        assert_eq!(
            flow.tools,
            vec![
                "pixel_prompt".to_string(),
                "pixel_run_shader".to_string(),
                "pixel_run_shader".to_string(),
            ],
            "第二发现样重发就收摊，不能再往模型那儿烧一轮完整请求：{flow:?}"
        );
        assert_eq!(
            flow.error.as_deref(),
            Some("agent.shader_repeat_failure"),
            "收摊的原因要说清是「刚出错的脚本又发了一遍」：{flow:?}"
        );
        assert!(
            !flow.completed && flow.text.is_empty(),
            "这轮已经收了摊，模型后面的正文一个字符都不该出现：{flow:?}"
        );
        assert_eq!(
            flow.doc_updates, 1,
            "只有跑过头那一发的错误才算一次画布事件：{flow:?}"
        );
        let doc = s
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let layer = doc.layers[0].id.clone();
        let frame = doc.frames[0].id.clone();
        let painted = doc
            .cel(&layer, &frame)
            .unwrap()
            .indices
            .iter()
            .filter(|i| **i != 0)
            .count();
        assert_eq!(painted, 0, "跑挂的脚本不许在画布上留半截：{painted}");
    }

    /// 护栏只拦「一字不差的坏脚本」。模型把坐标改对了，那就是一次正常的修复，
    /// 必须放行——拦在这儿会把唯一能救场的路堵死，用户只能自己上编辑器。
    #[tokio::test]
    async fn a_corrected_script_after_a_failure_still_runs() {
        let s = session();
        rewire(
            &s,
            vec![
                craft_call("c0"),
                tool_call(
                    "c1",
                    "pixel_run_shader",
                    json!({"script": "pset(-1, 0, hex('#ff004d'))"}),
                ),
                tool_call(
                    "c2",
                    "pixel_run_shader",
                    json!({"script": "pset(0, 0, hex('#ff004d'))"}),
                ),
                done("画好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一个红色像素".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        assert!(flow.error.is_none(), "改对了不许被收摊挡掉：{flow:?}");
        assert!(flow.completed, "{flow:?}");
        assert_eq!(flow.text, "画好了");
        let doc = s
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let layer = doc.layers[0].id.clone();
        let frame = doc.frames[0].id.clone();
        let row = &doc.cel(&layer, &frame).unwrap().indices[..2];
        assert_ne!(row[0], 0, "修好的那一发得真的画上");
        assert_eq!(row[1], 0, "画错那一发不许留底：{row:?}");
    }

    /// 要图的一轮不写提示词清单就动笔：头一发被挡下并要求补写，画布一个像素
    /// 都不许动；第二次仍然跳过清单时，按用户原话兜底一支基线放行，并如实
    /// 告诉用户这单没走成完整的提示词流程。
    #[tokio::test]
    async fn an_art_turn_requires_the_prompt_lists_before_drawing() {
        let s = session();
        let script = marker_walker();
        rewire(
            &s,
            vec![
                tool_call("c1", "pixel_run_shader", json!({"script": script})),
                tool_call("c2", "pixel_run_shader", json!({"script": script})),
                done("画好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画个方块".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert_eq!(
            flow.statuses,
            vec!["agent.craft_fallback".to_string()],
            "兜底放行要说出来，不能悄悄降级：{:?}",
            flow.statuses
        );
        // 挡一次、画一次：各推一次画布，被挡那一次一个像素都没动。
        assert_eq!(
            flow.doc_updates, 1,
            "挡下那一次不许推画布，只有真画了才推：{flow:?}"
        );
        assert!(flow.completed, "{:?}", flow.error);
        let doc = s
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let layer = doc.layers[0].id.clone();
        let frame = doc.frames[0].id.clone();
        let row = &doc.cel(&layer, &frame).unwrap().indices[..8];
        assert_ne!(row[0], 0, "兜底之后这一发真的画上了");
    }

    /// 清单写定之后，这一轮的每一发请求都得带着它上路：不然写过的约束下
    // 一发就丢了，模型照着自己的老习惯画，等于没写。
    #[tokio::test]
    async fn a_crafted_list_binds_every_request_after_it() {
        let s = session();
        let script = marker_walker();
        let watched = rewire_watch(
            &s,
            vec![
                craft_call("c0"),
                tool_call("c1", "pixel_run_shader", json!({"script": script})),
                done("画好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画个方块".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert!(flow.completed, "{:?}", flow.error);
        let systems = watched
            .systems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(systems.len(), 3, "写清单一发、照着画一发、收尾一发");
        assert!(
            !systems[0].contains("CRAFTED PROMPTS FOR THIS TURN"),
            "清单还没写，这一发要的是「先去写」：{}",
            systems[0]
        );
        assert!(
            systems[2].contains("CRAFTED PROMPTS FOR THIS TURN"),
            "画完之后那一发也得带着清单，别让它回头推翻自己：{}",
            systems[2]
        );
        assert!(
            systems[0].contains("PROMPT CRAFT"),
            "没写清单的那一发仍然带着动笔前的要求：{}",
            systems[0]
        );
        assert!(
            systems[1].contains("CRAFTED PROMPTS FOR THIS TURN"),
            "写着清单的那一发得把它带上路：{}",
            systems[1]
        );
        assert!(
            systems[1].contains("a 64x64 red marker block"),
            "清单的原文要在约束段里：{}",
            systems[1]
        );
    }

    /// 纯聊天的一轮从不要求写提示词清单：没人在问答里往画布上落像素，
    /// 逼它先写清单只是把一轮闲聊拖成三轮。
    #[tokio::test]
    async fn a_chat_turn_never_asks_for_prompt_lists() {
        let s = session();
        let watched = rewire_watch(&s, vec![done("这一栏是导出用的")]);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        assert_eq!(flow.text, "这一栏是导出用的");
        let systems = watched
            .systems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(systems.len(), 1);
        assert!(
            !systems[0].contains("PROMPT CRAFT"),
            "闲聊不该被逼写提示词：{}",
            systems[0]
        );
    }

    /// 画布动过了，同一份 script 就不再算重放：模型针对新画面修修补补时
    /// 原样再来一次是正经操作，护栏不许误伤。
    #[tokio::test]
    async fn the_same_script_is_allowed_again_once_the_canvas_moved() {
        let s = session();
        let script = marker_walker();
        // 先写清单：这一轮在要图，绕不过提示词那一步。少了它第一发 shader
        // 会被挡下去补写，marker 的落点就全错了。
        rewire(
            &s,
            vec![
                craft_call("c0"),
                tool_call("c1", "pixel_run_shader", json!({"script": script})),
                tool_call(
                    "c2",
                    "pixel_apply_operations",
                    json!({"operations": [{"op": "add_palette_colors", "colors": ["#29ADFF"]}]}),
                ),
                tool_call("c3", "pixel_run_shader", json!({"script": script})),
                done("方块挪好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画个方块再整理下调色板".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        assert_eq!(
            // pixel_plan 是分流给界面看的节点，pixel_prompt 是提示词清单。
            // 两个都不动画布，剔掉再比剩下真正落在画布上的调用。
            flow.tools
                .iter()
                .filter(|n| **n != "pixel_plan" && n.as_str() != craft::PROMPT_TOOL)
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                "pixel_run_shader",
                "pixel_apply_operations",
                "pixel_run_shader"
            ]
        );
        assert!(flow.completed, "{:?}", flow.error);
        let doc = s
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let layer = doc.layers[0].id.clone();
        let frame = doc.frames[0].id.clone();
        let row = &doc.cel(&layer, &frame).unwrap().indices[..8];
        // 第一发把 marker 放在 0 格；apply_operations 动过画布之后第二发
        // 真的跑了，marker 走到 1 格——走下就说明没被当成重放。
        assert_eq!(row[0], 0, "marker 被第二发挪走了");
        assert_ne!(row[1], 0, "画布动过之后同一份 script 该跑就跑");
        assert!(
            row[2..].iter().all(|&i| i == 0),
            "只跑了两发，不许再多走：{row:?}"
        );
    }

    /// 同一份 script 瞄另一个 cel 就是另一堆像素，不能算重放。
    #[tokio::test]
    async fn the_same_script_aimed_at_another_layer_is_not_a_replay() {
        let s = session();
        let script = marker_walker();
        // 先写清单：建层和落笔都算生图，绕不过提示词那一步。少了它第一发就被
        // 挡下去补写，L9 根本建不出来，后两发 shader 全落在缺名的图层上，
        // 这条测试想守的「换个图层就不算重放」也就无从验证。
        rewire(
            &s,
            vec![
                craft_call("c0"),
                tool_call(
                    "c1",
                    "pixel_apply_operations",
                    json!({"operations": [{"op": "create_layer", "id": "L9"}]}),
                ),
                tool_call("c2", "pixel_run_shader", json!({"script": script})),
                tool_call(
                    "c3",
                    "pixel_run_shader",
                    json!({"script": script, "layer": "L9"}),
                ),
                done("两层都画好了"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画两层".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert_eq!(
            // pixel_plan 是分流给界面看的节点，pixel_prompt 是提示词清单，
            // 两个都不动画布，剔掉再比剩下真正落在画布上的调用。
            flow.tools
                .iter()
                .filter(|n| **n != "pixel_plan" && n.as_str() != craft::PROMPT_TOOL)
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                "pixel_apply_operations",
                "pixel_run_shader",
                "pixel_run_shader"
            ]
        );
        assert!(flow.completed, "{:?}", flow.error);
        let doc = s
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let frame = doc.frames[0].id.clone();
        assert!(
            doc.layers.iter().any(|l| l.id == "L9"),
            "create_layer 指了 id 就得叫这个名字：{:?}",
            doc.layers.iter().map(|l| &l.id).collect::<Vec<_>>()
        );
        // 先确认 cel 真的存在再读像素： unwrap 到 None 只会甩一句 panicked，
        // 看不出到底是层没建成还是 shader 没跑。
        let cel = doc
            .cel("L9", &frame)
            .expect("同一份 script 画到另一个图层得真跑，cel 必须在");
        assert_ne!(cel.indices[0], 0, "同一份 script 画到另一个图层得真跑");
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
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
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
        assert_eq!(
            s.messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            41
        );
    }

    #[tokio::test]
    async fn each_round_gets_its_own_continuation_budget() {
        let s = session();
        // 每轮都用掉三次续写。配额归「单次回复」时每轮都是干净的三次；
        // 换成 turn 级的话第二轮只剩两次，第三次就得提前撞墙收摊。
        let round = |tag: &str| {
            (0..3)
                .map(|i| {
                    cut(&format!(
                        "第 {tag} 轮第 {i} 段被掐断的正文，模型确实在往下写，长度足够算一次推进"
                    ))
                })
                .collect::<Vec<_>>()
        };
        // 每一「发」都是一个独立的响应：伪 provider 收到第一个 Done 就收流，
        // 同一个脚本里排在 Done 后面的事件根本没人读。flatten 会把三段并成
        // 一发，续写的次数就怎么也攒不起来。
        rewire(
            &s,
            round("一")
                .into_iter()
                .chain(std::iter::once(tool_call(
                    "c1",
                    "pixel_apply_operations",
                    json!({"operations": [{"op": "create_layer", "id": "L9"}]}),
                )))
                .chain(round("二"))
                .chain(std::iter::once(done("两轮都写完了")))
                .collect(),
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert!(flow.completed, "第二轮不许被上一轮用掉的续写次数拖死");
        assert_eq!(
            flow.statuses
                .iter()
                .filter(|k| *k == "agent.continuing")
                .count(),
            6,
            "两轮各续三次，每续一次都要有进度提示：{:?}",
            flow.statuses
        );
        assert!(flow.error.is_none(), "{:?}", flow.error);
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
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
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
        assert_eq!(
            s.messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            3
        );
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
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert!(flow.completed, "工具跑完接着问，这一轮要能收尾");
        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert_eq!(flow.text, "先写脚本画好了");
        // 半截入参不可信：进历史会被 provider 整包拒掉，续写那次就白发。
        // 「画一只猫」命中知识库，分流节点先摆出来；真正要看的是残缺那一次
        // apply_operations 没留下痕迹。
        assert_eq!(
            flow.tools,
            vec!["pixel_plan".to_string(), "pixel_read_canvas".to_string()]
        );
        let history = s
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
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

    /// 增量广播的骨：第一次给全量，之后只报动过的 cel。
    ///
    /// 这是整个「不再整份推」改动的关键性质——前端本地持有一份完整文档，靠
    /// 增量往里合。第二次广播要是漏报没动过的 cel，前端那份文档就永久缺一格，
    /// 而且不报错。所以宁可多 diff 一遍，也不靠「哪个工具写过的」来记。
    #[test]
    fn broadcasting_twice_only_reports_the_touched_cel() {
        let s = session();
        // 8x8 新文档一层一帧：第一次是全量，一个 cel 都不该少。
        let first = s.document_patch();
        assert_eq!(first.cels.len(), 1);
        assert!(first.dropped.is_empty());
        assert_eq!(first.name, "t");
        assert_eq!(first.width, 8);

        // 只动一个格子：模拟一次 apply_operations。
        let layer = s.active().layer;
        let frame = s.active().frame;
        s.with_document_mut(|doc| {
            doc.revision += 1;
            doc.cels
                .get_mut(&layer)
                .and_then(|f| f.get_mut(&frame))
                .expect("新文档该有这一格")
                .indices[0] = 3;
        });
        let second = s.document_patch();
        assert_eq!(second.cels.len(), 1, "只有一个 cel 动过");
        assert_eq!(
            second.cels[0].2.first().copied(),
            Some(3),
            "报上来的正是动过的那一格"
        );
        assert_eq!(second.revision, first.revision + 1);

        // 什么都没改：元数据照给，cel 一个都不报。空 patch 无害且便宜。
        let third = s.document_patch();
        assert!(third.cels.is_empty());
        assert!(third.dropped.is_empty());
        assert_eq!(third.revision, second.revision);
    }

    /// sync_document 之后基准必须换成前端交上来的那份。
    ///
    /// 不换的话下一次 emit 会拿旧基准 diff 出一份全量 patch——而前端刚刚才把
    /// 这份文档发过来，白推几十 MB，还把它本地尚未确认的选择态整个盖掉。
    #[test]
    fn sync_document_resets_the_broadcast_baseline() {
        let s = session();
        let fresh = Document::new("fresh", 4, 4).unwrap();
        s.sync_document(fresh);
        let patch = s.document_patch();
        assert_eq!(patch.name, "fresh");
        assert_eq!(patch.width, 4);
        // 一个 cel 都不报，正是基准换对了：前端刚交上来的就是这份，它本地已经有。
        // 基准没换的话，这里会拿旧 8x8 文档 diff 出一整个全量 patch 来。
        assert!(patch.cels.is_empty(), "同步过的文档不该再全量推一遍");

        // 基准已是这份：下一份增量不再带着旧文档的任何东西。
        let after = s.document_patch();
        assert!(after.cels.is_empty());
        assert!(after.dropped.is_empty());
        assert_eq!(after.name, "fresh");
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

    /// 主循环崩在半路，也必须给这一回合一个收口。
    ///
    /// 收不了口的话前端的 running 永远挂着：占位节点闪着光标，点停止只等到
    /// 「这个会话还在忙」，下一句发什么都撞忙——用户看到的就是「按了没反应」。
    /// 崩掉的 future 会就地蒸发，之后没有任何人会再发事件，所以这一层兜底
    /// 必须是「崩溃也照样收口」，而不是「正常跑完才收口」。
    #[tokio::test]
    async fn a_turn_that_panics_mid_way_still_closes() {
        struct ExplodingProvider;
        #[async_trait::async_trait]
        impl providers::LlmProvider for ExplodingProvider {
            async fn request(
                &self,
                _req: &ChatRequest,
            ) -> Result<providers::EventStream, ProviderError> {
                panic!("a poisoned lock right in the middle of the turn");
            }
        }

        let s = session();
        let config = s
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .config
            .clone();
        *s.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Engine {
            config,
            provider: Arc::new(ExplodingProvider),
        };

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // 默认钩子会往测试输出里刷一条栈，临时换成安静的那个，跑完立刻还原。
        let chatty = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        s.run_turn_guarded("画一只猫".into(), Vec::new(), tx, None)
            .await;
        std::panic::set_hook(chatty);

        let flow = drain(rx);
        assert!(!flow.completed, "崩掉的那一圈不算画完");
        assert_eq!(
            flow.error.as_deref(),
            Some("agent.turn_crashed"),
            "崩在主循环里也要回一条 error，前端才收得住 running"
        );
    }

    /// 换一把钉死关思考的会话：用户要的是「别想，直接画」。
    fn session_with_thinking_off() -> AgentSession {
        let s = session();
        let mut config = s
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .config
            .clone();
        config.disable_thinking = Some(true);
        let provider = s
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .provider
            .clone();
        *s.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Engine { config, provider };
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
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert_eq!(
            watched
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
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
        // 「画一只猫」命中美术知识库，分流节点先摆出来；要看的仍然是那句 shader 调用
        // 在关思考之后真的发了出来。
        assert_eq!(
            flow.tools,
            vec!["pixel_plan".to_string(), "pixel_run_shader".to_string()]
        );
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
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert!(
            flow.statuses
                .contains(&"agent.thinking_off_retry".to_string()),
            "不报截断的长思考也该关掉思考重问：{:?}",
            flow.statuses,
        );
        assert_eq!(
            watched
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[false, true, true],
            "翻盘后该带着关思考重发",
        );
        assert_eq!(
            flow.tools,
            vec!["pixel_plan".to_string(), "pixel_run_shader".to_string()]
        );
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
        s.run_turn("这个面板怎么用？".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        assert_eq!(
            watched
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[false],
            "照模型默认来，不许偷偷改开关：{:?}",
            watched
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
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
        s.run_turn("画5帧橘猫奔跑".into(), Vec::new(), tx, None)
            .await;
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        s.run_turn("照这个画风换色".into(), vec![reference_image()], tx, None)
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
        s.run_turn("画个图标".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert!(flow.completed, "{:?}", flow.error);
        let systems = watched
            .systems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        s.run_turn("画个图标".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert!(flow.completed, "{:?}", flow.error);
        let systems = watched
            .systems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    /// 纯快照的一轮不发分流节点：没有参照图可定性，也没有知识命中，空卡片只是噪声。
    #[tokio::test]
    async fn a_snapshot_only_turn_stays_quiet() {
        let s = session();
        // 一句话看完就答，不是在要图：不用备催促的重放。
        rewire(&s, vec![done("看完了")]);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("帮我看看这张图".into(), vec![snapshot_image()], tx, None)
            .await;
        let flow = drain(rx);

        assert!(
            !flow.tools.contains(&"pixel_plan".to_string()),
            "快照不定性，别摆节点：{:?}",
            flow.tools
        );
        assert_eq!(flow.text, "看完了");
    }

    /// 澄清提问是合法收尾：问完就该停下等用户回话，不能催。
    #[tokio::test]
    async fn a_clarifying_question_is_left_alone() {
        let s = session();
        rewire(&s, vec![done("想要侧视还是四分之三视角？")]);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画只猫".into(), Vec::new(), tx, None).await;
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
        s.run_turn("画5帧橘猫".into(), Vec::new(), tx, None).await;
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
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        assert_eq!(
            watched
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[true]
        );
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
        s.run_turn("导出按钮在哪".into(), Vec::new(), tx, None)
            .await;
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
        s.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .config
            .disable_thinking = Some(false);
        let watched = rewire_watch(
            &s,
            vec![
                cut_after_thinking("想"),
                cut_after_thinking("接着想"),
                cut_after_thinking("还在想"),
            ],
        );

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        s.run_turn("画一只猫".into(), Vec::new(), tx, None).await;
        let flow = drain(rx);

        assert_eq!(
            watched
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
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

        // 顺手钉住分工段的落点：光有绑定还不够，它得真出现在每一发请求的
        // 系统提示词里，不然模型压根不知道生图另有其人，会自己脑补位图。
        let cfg = s.runner_config();
        let req = s.chat_request(&cfg, None, false, false);
        assert!(
            req.system.contains("MODEL ROLES"),
            "系统提示词里没有模型分工段"
        );
        assert!(
            req.system.contains("Every stage above is you"),
            "还没绑专职模型，必须明说各阶段都由主模型完成"
        );
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
        let cfg = s.runner_config();
        let req = s.chat_request(&cfg, None, false, false);
        assert!(
            req.system.contains("image_gen (img label)"),
            "专职生图模型没在分工段里被点名"
        );
        assert!(
            req.system.contains("Do not hand-write an approximation"),
            "没警告别自己编专职阶段的输出"
        );

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
        *s.approval
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ApprovalSlot {
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
        *s.approval
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ApprovalSlot {
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
        // 中文那句话绝不会拿逗号、顿号、冒号收尾：这三个一出现就是断了。
        assert!(looks_cut_off("配色用的是橘色系，"));
        assert!(looks_cut_off("需要改的地方有尾巴、耳朵、"));
        assert!(looks_cut_off("接下来调整："));
    }

    #[test]
    fn a_reply_that_lands_cleanly_does_not_look_cut_off() {
        // 猜错一次要白跑一整轮续写，所以只认最硬的信号。
        assert!(!looks_cut_off("```lua\npset(1, 1, red)\n```\n画完了"));
        assert!(!looks_cut_off("五帧行走图已经画好，帧时长都是 100ms。"));
        // 句子中间是整段说的，只有最后一行算数，所以整段里出现逗号不算。
        assert!(!looks_cut_off("第一帧落地\n第二帧收腿\n五帧都画好了"));
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
            self.asked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(req.max_tokens);
            match self
                .outcomes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(0)
            {
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
        let mut config = s
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .config
            .clone();
        config.model = "deepseek-v4.1-flash".into();
        config.max_tokens = None;
        *s.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Engine {
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
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx, None)
            .await;
        let flow = drain(rx);

        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert!(flow.completed, "降级之后该正常跑完");
        assert_eq!(
            provider
                .asked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
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
        s.run_turn("解释一下这个面板怎么用".into(), Vec::new(), tx, None)
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
        let messages = s
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(messages.len(), 2, "追问指令该被撤掉");
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);
    }

    /// 长工具挂住时，停止键必须在轮询间隔内把人放出去，不能干等到工具自己的超时。
    /// 这正是生图和外部 MCP 两条路径的实际情形：用户点了停止，界面得立刻有反应。
    #[tokio::test]
    async fn a_long_tool_call_yields_the_moment_stop_is_pressed() {
        let s = Arc::new(session());
        let spawned = s.clone();
        let waiting = tokio::spawn(async move {
            spawned
                .until_cancelled(tokio::time::sleep(Duration::from_secs(30)))
                .await
        });

        // 先给它几拍 tick，确保取消不是在 future 头一次 poll 就被贴脸撞上的。
        tokio::time::sleep(CANCEL_POLL * 2).await;
        s.interrupt();

        let out = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("停止键按下后 until_cancelled 要立刻放行")
            .unwrap();
        assert!(out.is_none(), "取消成功了才回 None，调用方才能 bail 收摊");
    }

    /// 没人点停止，等到的东西要原样交出去：正常回来的工具结果不该被误伤。
    #[tokio::test]
    async fn a_tool_call_that_finishes_alone_is_passed_through() {
        let s = session();
        let out = s
            .until_cancelled(async {
                tokio::time::sleep(CANCEL_POLL * 2 + Duration::from_millis(20)).await;
                7
            })
            .await;
        assert_eq!(out, Some(7));
    }

    /// 生图提示词要挂着这一轮的渲染规矩出门。
    ///
    /// 用户没点名风格的那一轮恰恰是最常见的一轮，也是过去最吃亏的一轮：
    /// 一句话里连成品触发词都未必中，生图模型于是照自己的训练分布交差。
    /// 两套规矩不能同时上身——风格命中时风格顶替默认档，不然生图模型只在
    /// 两段打架的要求里猜权重，交回来的是一张四不像。
    #[test]
    fn the_image_prompt_carries_this_turns_style() {
        let s = session();
        let plain = s.image_prompt("a small frog");
        assert!(
            plain.contains("finished drawing means"),
            "没点名风格也该有默认质量档：{plain}"
        );

        let plan = TurnPlan::from_text_with_style("画只gameboy风格的猫", &[], false, None);
        assert_eq!(plan.style.map(|s| s.id()), Some("gameboy"), "风格没读出来");
        *s.plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = plan;
        let styled = s.image_prompt("a small frog");
        assert!(
            !styled.contains("finished drawing means"),
            "风格规矩该顶替默认档，不是叠加：{styled}"
        );
        assert_eq!(
            styled,
            artstyle::decorate_image_prompt("a small frog", Some(artstyle::ArtStyle::GameBoy)),
            "轮次里定的风格要真的落到生图提示词上"
        );
    }

    /// 轮次预设也走同一个出口：界面在输入区叠的收尾规矩跟着这一句进生图提示词。
    ///
    /// 生图模型不知道用户在输入区点了什么，也不共用我们的 Lua 沙箱规矩。
    /// 少了这一段，「写实渲染 + 微细结构」这三下点击在生图那条路上等于没点。
    #[test]
    fn this_turns_presets_also_reach_the_image_prompt() {
        let s = session();
        // 一条预设的轮次：默认档要让位，预设正文要上车。
        let plan = TurnPlan::from_text_pinned(
            "画只写实的猫",
            &[],
            false,
            None,
            vec![presets::parse("microdetail").unwrap()],
        );
        assert_eq!(plan.presets.len(), 1, "预设钉进了轮次计划");
        *s.plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = plan;
        let out = s.image_prompt("a small frog");
        assert!(
            !out.contains("default quality tier"),
            "预设在路上时默认档整段让位：{out}"
        );
        assert!(
            out.contains("fur direction in 1px strokes"),
            "预设规矩正文要跟着来：{out}"
        );

        // 换一轮就把旧的放下了：预设是「这一句的偏好」，不跟着下一句赖着。
        *s.plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            TurnPlan::from_text("画只猫", &[], false);
        let plain = s.image_prompt("a small frog");
        assert!(
            plain.contains("default quality tier"),
            "没叠预设的下一句照旧只挂默认档：{plain}"
        );
    }

    /// 行为准则三条必须真出现在发出去的系统提示词里，而且要跟着动笔走。
    /// 前半截查「要图那轮一条不少」，后半截查「纯问答那轮一条不带」——
    /// 少了后半截，一句「你好」也会收到三轮施工规矩，模型会把闲聊也当施工队。
    /// 中间那条查护栏没把技法挤掉：两者各走一份预算，这才是这段设计的全部意义。
    #[test]
    fn the_discipline_trio_reaches_the_request_and_only_on_drawing_turns() {
        let s = session();
        let cfg = s.runner_config();
        let trio = [
            "Lock the spec before drawing",
            "Self-check before calling anything done",
            "Patterns that make procedural work look fake",
        ];

        *s.plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            TurnPlan::from_text("画5帧橘猫奔跑", &[], false);
        let drawing = s.chat_request(&cfg, None, false, false);
        for title in trio {
            assert!(
                drawing.system.contains(title),
                "要图那轮少了准则「{title}」"
            );
        }
        // 步态相位表是这一轮真正该带的技法，护栏不许把它顶掉。
        assert!(
            drawing
                .system
                .contains("Gaits: walk, trot, canter, gallop, flight"),
            "护栏挤掉了技法条目：两条线各走各的预算失效了"
        );
        // 落点：护栏夹在分流与对照表之间，不能漂到画布上下文后面去。
        let routing_at = drawing.system.find("TURN ROUTING").expect("分流段没进请求");
        let discipline_at = drawing
            .system
            .find("Working discipline for every drawing")
            .expect("准则段没进请求");
        let tables_at = drawing
            .system
            .find("COLOR NAMES")
            .expect("颜色名表没进请求");
        assert!(
            routing_at < discipline_at && discipline_at < tables_at,
            "准则段该夹在分流与对照表之间"
        );

        *s.plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            TurnPlan::from_text("这段配色什么意思", &[], true);
        let chat = s.chat_request(&cfg, None, false, false);
        for title in trio {
            assert!(
                !chat.system.contains(title),
                "纯问答轮次不该带准则「{title}」"
            );
        }
    }
}
