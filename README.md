<div align="center">
<img src="./icon.png" style="width:100px;" width="100"/>
<h2>AIPixel</h2>
</div>

> 开源的像素画 Agent 桌面工具：Tauri 2 + React 18 + antd 5，Rust 主循环，
> 只用你自己的模型，中间文件是 `.aip`。

### 一、产品概述

- AIPixel 是一个跑在本地的像素画 agent 工作台：左侧会话、中间对话、右侧画布与文档，一次对话产出一张 `.aip`
- **BYOM（自带模型）**：没有内置服务器、没有登录、没有计费。模型只来自你在本机 UI 里填的 Provider，密钥存在 `app_config_dir/models.json`，永不出本机
- **Rust 主循环**：prompt 组装、provider 流式、`tool_use` 抽取、工具执行、结果回填、续轮全部在 Rust 侧跑完，前端只负责渲染与输入
- **文本网格即权威状态**：canvas 的权威形态是文本网格（RLE 编码）而不是位图，图片只是上下文，模型永远不许手写像素矩阵
- 面向做 RPG / 独立游戏的美术与程序，也面向想研究「agent 怎么安全地驱动一个文档模型」的人

核心价值：让模型碰像素画，最怕它一口气「手写」一屏 4096 个色号，改一个像素要重画整张图，一跑偏就整张作废。
AIPixel 把生图路径收窄成三条类型化工具，模型的自由度放到该放的地方（结构、构图、脚本生成），每一笔像素都落在受预算约束的沙箱里。

### 二、界面

Agent 会话与工作台都已落地：会话负责产出，工作台负责盯着它、以及在画布上直接动手。

左侧 `src/ui/SessionSidebar.tsx` 是会话列表，支持空白新建和带 WxH 的新建；中间 `src/ui/ChatPanel.tsx` 是对话流：
用户气泡带附件条、助手消息带流式光标、工具调用可展开看 JSON 入参、推理片段折叠在 `<details>` 里；右侧 `src/ui/DocumentPanel.tsx` 把
document 渲染成画布，图层 / 帧 / 调色板各一行，还能切到 `.aip` 原文。

画布上方是 Brush / Fill 切换与撤销；帧那一行右边是新建、复制、删除、前后挪帧。落地逻辑在
`src/lib/store.ts`：画笔的笔迹先在 `DocumentPanel` 的透明画布上增量预览，抬笔才整笔发给 Rust，
改动统一经 `document_updated` 回到前端，画布只有一条刷新路径。

顶栏依次是当前会话绑定的模型、权限档位（Auto / Chat / Ask，决定每次工具调用要不要先问）、打开与另存 `.aip`、挂参考图、模型设置。
没有配过模型时，`src/ui/StarterGate.tsx` 会把应用收成一张引导页。

### 三、架构

两个 Rust crate 加一个 Tauri 壳，前端是一层薄壳。

| 层 | 位置 | 职责 |
| --- | --- | --- |
| Rust 壳 | `src-tauri/` | 应用状态托管、命令注册、`agent-event` 事件广播。只做桌面装配，没有业务逻辑；`src-tauri/src/editor.rs` 是工作台编辑器命令层（画笔 / 油漆桶 / 结构操作），与主循环共用同一把文档锁 |
| Agent 主循环 | `crates/agent-core/` | prompt 组装、provider 流式、`tool_use` 抽取、工具执行、结果回填、续轮 |
| 像素文档模型 | `crates/pixel-core/` | document 模型、类型化操作、RLE 上下文编码、`.aip` v2、Lua 沙箱着色器、PNG 导出 |
| 前端 | `src/` | React + antd + zustand，只做渲染和输入；`src/lib/bridge.ts` 是唯一的 invoke / event 出口 |

agent-core 不依赖 Tauri，是纯 Rust。它通过一个 `tokio::sync::mpsc` 通道向外吐 `AgentEvent`，由 Tauri 壳转成事件广播，
前端在 `src/lib/transcript.ts` 把事件流折叠成可渲染条目（纯函数，可单测）。

### 四、Agent 怎么工作

一次发送的流程：`prompt admission -> provider 流式输出 -> tool_use -> 工具执行 -> 结果回填 -> 续轮`。
每轮都重新组装系统提示词，因为上一轮的工具可能已经改过 canvas。模型能用的工具只有三个：

- `pixel_apply_operations`：一次事务里做一坨类型化操作。图层 / 帧 / 调色板的结构改动走这里（建、复制、挪、删、改名、设时长），也可以用 `set_pixels`、`stamp_grid`、`draw_shape`、`bucket_fill`、`clear_region` 打小补丁。任一操作非法则整事务回滚，错误信息会指出失败的操作下标
- `pixel_run_shader`：一段 Lua 脚本，配一次事务的绘制与动画。带 Loops 与 palette helpers，`animate=true` 时按 `phase`（0..1）驱动每一帧
- `pixel_read_canvas`：读回当前网格，`overview=true` 时给降采样地图，最多读 128x128 的精确窗口

「模型不许手写矩阵」的契约在 `crates/agent-core/src/tools.rs` 收口：绘制和动画统一走 Lua 沙箱，结构改动统一走 ops，读回统一走 RLE。

预算与退避保护同样在主循环里：`max_tool_steps`（单 turn 工具步数，默认 24）、`max_turns`（续轮次数，默认 12）、
`max_tool_result_bytes`（回灌截断，默认 6000 字符），外加同一个失败调用连续 3 次的退避。流式期间按 120ms 轮询取消标志，`interrupt()` 立刻收尾。

审批闸门是权限档位的落点，见 `crates/agent-core/src/runner.rs` 的 `await_approval`：Auto 每个调用直接执行；
Ask 每个调用都停在 `approval_request` 上等用户；Chat 只拦写操作，`pixel_read_canvas` 这种只读回放直接过。
一个 turn 顺序执行工具，同时最多挂一条等票；等待期间照样本轮询取消标志，所以中断不会把后半场卡死。
Approve all 只把当前 turn 降级成 Auto，不写回会话配置——「这次别烦我」不是「以后都别问」。
Reject 不当失败调用（不进退避计数）：喂一条 tool_result 让模型解释它想干什么、换方向，对话继续。

系统提示词是「静态 craft 规则 + 动态 canvas 上下文」两段，见 `crates/agent-core/src/prompt.rs`。静态部分写工作流、像素与动画 craft、
RLE 编码约定；动态部分由 `pixel_core::context` 按当前激活图层 / 帧实时生成。提示词与工具契约按本项目自己的约束重做。

#### MCP 工具服务器

主循环的能力不止三个内置工具。顶栏的插头图标打开「MCP tool servers」面板，可以挂用户自己的 MCP 服务器：stdio（拉起子进程、
换行分隔 JSON-RPC）和 HTTP（JSON-RPC POST，兼容 SSE 响应）两种传输都支持，协议版本按 2025-06-18 / 2025-03-26 / 2024-11-05
依次协商。配置落盘在 app config 目录的 `mcp.json`；勾了 Auto 的服务器在启动时自动连接，失败的只记错误、不阻塞启动。

服务器上的工具以 `mcp__<server>__<tool>` 命名空间进入每轮的 tool specs，和内置像素工具并列交给模型。名字过长会被截断并加哈希
后缀，调用时由 `crates/agent-core/src/mcp.rs` 的 registry 按最长前缀还原到具体服务器。MCP 调用同样过审批闸门：Ask 档位每个
调用都停在 `approval_request` 上，Chat 档位把写操作挑出来拦；回灌结果和内置工具共用同一条截断预算。

`env` / `headers` 只写在本机 `mcp.json`，界面视图只回键名不回值，凭据不回传 webview。编辑已有服务器时，值留空表示沿用本机旧值，
整行删掉才清掉这个键。

### 五、`.aip` 格式

`.aip` 是明文文本：宽高、调色板、图层、帧都在文本里，天然适合版本管理和人工微调。v2 把图层和帧提成一等公民，
可以存动画。索引 0 恒为透明。它既是中间文件，也是 agent 的持久化产物：顶栏可以把当前文档另存为 `.aip` v2，
也能打开 v2 或旧版。

### 六、开发

需要 Node 20+ 与 Rust stable，包管理器用 pnpm。

```bash
# 前端
pnpm install
pnpm dev            # 仅起 vite（1420 端口），用于调 UI
pnpm build          # tsc -b && vite build
pnpm test           # vitest

# 桌面端
pnpm tauri dev      # 完整桌面调试

# Rust 侧
cargo test --workspace
```

模型配置在应用内「设置」里填，落盘在 app config 目录的 `models.json`，`api_key` 只留在本机、不回传 webview。

### 七、遗留 Python 工具链

仓库根目录还留着最初那套 Pillow 脚本，仍然可用，与桌面端无关：

- `aip_converter.py`：`.aip` 转 PNG
- `img2aip_converter.py`：PNG 反查成 `.aip`（颜色索引 + 透明处理）
- `img2x_converter.py`：高质量缩放（letterbox / Lanczos）
- `process_attack.py`、`process_knight.py`：精灵图切帧

```bash
python3 aip_converter.py input/heart.aip
python3 img2aip_converter.py refer_img/banana_shadow.png
```

`html/index.html` 是早期的网页版像素编辑器，一并保留作参考。

### 八、路线图

- Agent 会话（已落地）：会话、对话流、三个工具、`.aip` 读写、BYOM 配置
- 工作台（已落地）：Auto / Chat / Ask 三档审批、画笔与油漆桶（调色板选透明格即擦）、帧的新建 / 复制 / 删除 / 挪位、仅限直接编辑的撤销栈
- 下一步：图层行的直接操作（显隐 / 透明度 / 排序）、批量与脚本化流程

### 参照与致谢

- `424431185/pixel-asset-master-skills`：像素资产生成的工作流切分与提示词工程思路
- `Fantety/PixTXT`：`.aip` 相邻文本像素格式的图层 / 帧文档模型设计

两份都是参照而非照抄：格式、主循环与工具名都按我们自己的约束重做，`.aip` 与 `pixel_apply_operations` / `pixel_run_shader` /
`pixel_read_canvas` 的边界来自这份仓库自己的取舍。

License: MIT
