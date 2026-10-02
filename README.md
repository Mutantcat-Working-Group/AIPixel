<div align="center">
<img src="./icon.png" style="width:100px;" width="100"/>
<h2>AIPixel</h2>
</div>

> GPL-3.0 开源的像素画 Agent 桌面工具：Tauri 2 + React 18 + antd 5，Rust 主循环，
> 只用你自己的模型，中间文件是 `.aip`。

### 一、产品概述

- AIPixel 是一个跑在本地的像素画 agent 工作台：左侧会话、中间对话、右侧画布与文档，一次对话产出一张 `.aip`
- **BYOM（自带模型）**：没有内置服务器、没有登录、没有计费。模型只来自你在本机 UI 里填的 Provider，密钥存在 `app_config_dir/models.json`，永不出本机
- **Rust 主循环**：prompt 组装、provider 流式、`tool_use` 抽取、工具执行、结果回填、续轮全部在 Rust 侧跑完，前端只负责渲染与输入
- **文本网格即权威状态**：canvas 的权威形态是文本网格（RLE 编码）而不是位图，图片只是上下文，模型永远不许手写像素矩阵
- 面向做 RPG / 独立游戏的美术与程序，也面向想研究「agent 怎么安全地驱动一个文档模型」的人

核心价值：让模型碰像素画，最怕它一口气「手写」一屏 4096 个色号，改一个像素要重画整张图，一跑偏就整张作废。
AIPixel 把生图路径收窄成六条类型化工具（结构 ops、脚本、读回、补帧、图片转像素、直连生图），模型的自由度放到该放的地方（结构、构图、脚本、垫图），每一笔像素都落在受预算约束的沙箱里。

### 二、界面

Agent 会话与工作台都已落地：会话负责产出，工作台负责盯着它、以及在画布上直接动手。

左侧 `src/ui/SessionSidebar.tsx` 是会话列表，新建时先选画布宽高（支持改名与拖动排序）；中间 `src/ui/ChatPanel.tsx` 是对话流：
用户气泡带附件条、助手消息带流式光标、工具调用可展开看 JSON 入参、推理片段折叠在 `<details>` 里；右侧 `src/ui/DocumentPanel.tsx` 把
document 渲染成画布，图层 / 帧 / 配色范围各一行，还能切到 `.aip` 原文。

画布上方是画笔 / 填充 / 橡皮三个工具加撤销，旁边是播放与洋葱皮，另有一档瓦片底图：把画面平铺成
1x1 / 2x2 / 3x3，瓦片接缝一眼能看完；只有中间那格接管指针，其余格是同一幅画的回声。
帧条每一格是一张缩略图（画法与 Rust 权威渲染同源的 `compositeFrame`，看到的就是会导出的），缩略图下面是帧号与停留时长，点哪格跳哪帧；
帧那一行右边是新建、复制、删除、前后挪帧，
行下面一格直接填这一帧停留多少毫秒。
逐帧工具生成了帧却看不到动画是说不通的，所以播放用一个轻量 `setTimeout` 循环，帧号在前端本地走，
不每帧打一次 IPC；洋葱皮把上一帧按 24% 透明度垫在当前帧底下，逐帧对位才有依据。
图层列表按栈顶在上的惯例倒着排（和 Aseprite / Photoshop 一致），每行有显隐开关，行尾两个箭头把选中图层沿绘制顺序挪一格，
列表下方是当前图层的不透明度。这些改动都走 `pixel_apply_operations`，所以和画笔一样进撤销栈。
右侧面板的「配色范围」一栏把调色板当成边界而不是摆设：六套预设（Gray 8 / Sweetie 16 / DawnBringer 16 /
PICO-8 / Game Boy / 1-bit，专有名词两种语言下都不改写）加一个「任意颜色」槽，拖动取色实时预览、松手才算落，
一个预制只带得动一个自定义槽位。换预设不是换着玩：Rust 侧 `set_palette` 会按 CIELAB 就近色把已有像素
重映射进新范围，画面留住、颜色归队，所以预设收到的都是清一色不透明色。索引 0 恒为透明、不属于配色范围，
调色板行首那颗透明格就是擦除落点，画笔选它也当橡皮用。

导出收在右上角工具链的下载菜单里，不占右侧面板：GIF（无限循环，帧延时取文档自己的 `duration_ms`，
短于 20ms 会被抬上去——GIF 的延时单位是厘秒，不少查看器把 0 当成立刻切帧）、当前帧 PNG、
横向整条 PNG（所有帧并排一张）、精灵表 PNG、Aseprite（`.ase`）。编码全在 Rust 侧，
合成的原料是调色板索引而不是位图像素，所以导出来的和画布上看到的是同一份东西。

落地逻辑在 `src/lib/store.ts`：画笔的笔迹先在 `DocumentPanel` 的透明画布上增量预览，抬笔才整笔发给 Rust，
改动统一经 `document_updated` 回到前端，画布只有一条刷新路径。单帧合成在 `src/lib/render.ts`，
与 Rust 侧 `pixel_core::png::composite_pixel` 逐位对齐，两侧画出来的东西不会岔开。

顶栏依次是当前会话绑定的模型、权限档位（Auto / Chat / Ask，决定每次工具调用要不要先问）、打开与另存 `.aip`、挂参考图、模型设置，右侧是导出菜单。
一条模型都没配、或者会话还挂在已删掉的模型上时，顶栏显示「未设置模型」并给一条指向设置的横幅——界面不收起：会话能建、画布能画，只是发消息没人接。

顶栏左侧的「工作流」一栏是一组可切换的工作流条目，`src/ui/WorkflowDock.tsx`。每条按当前会话绑定的模型算 readiness：
模型能力不够的条目照样列出来，只是禁用并写清缺什么，不让你只看到一个灰按钮。目录七条：

智能体绘制（对话即绘制，每次编辑跑一个沙箱 Lua 脚本）、图像生成（模型渲染一张位图再量化到画布网格）、参考图简报
（视觉模型读成结构化简报再照着画）、视频抽帧（ffprobe 抽帧后逐帧纯本机量化，全程不调模型）、视频运动简报（读视频模型把一段
视频读成可编辑的运动简报，产物是文本不是帧）、补间帧（本机插值）、提示词微调。外加一条不在能力目录里的量化：把一张位图丢到
网格上，纯本机运行。七条按对模型的要求分三档：三条要专项能力（生图、读图、读视频），两条只要会话模型（智能体绘制、提示词微调），
两条全程本机、一个 token 都不花（视频抽帧、补间帧）——所以没有读视频模型的用户照样能把一段视频抽成帧。运动简报跑完不落文档，
结果塞进生图面板或对话输入框，由你决定下一步往哪走。

右栏三条轨道：画布、工作流、批量。「批量」一栏是 `src/ui/BatchPanel.tsx`，一个文件夹进、一个文件夹出的纯本机批处理，
和 agent 会话互补：会话是「一次一两张、一个模型盯着」，这里是「一个 token 都不花」的确定性重复劳动。两个方向：
量化把参考图批量反查成 `.aip`（量化参数与工作流同源，共用 `src/ui/QuantizeFields.tsx`），导出把 `.aip` 批量渲染成 PNG / GIF。
开跑前先扫一遍，把候选文件数报给你；过程走独立的 `batch-event` 通道，逐文件折进行表，单个文件失败只记原因不中断，
跑完的回执带成败计数与输出目录。同样的输入与同样的配方得到同样的输出，方便复跑与版本管理。调顺的参数组起个名字就能存进本机配方簿（落盘 app config 目录的 `recipes.json`，与 `models.json` 同目录同套路），下次直接取，可删可覆盖。

### 三、架构

两个 Rust crate 加一个 Tauri 壳，前端是一层薄壳。

| 层 | 位置 | 职责 |
| --- | --- | --- |
| Rust 壳 | `src-tauri/` | 应用状态托管、命令注册、`agent-event` 事件广播。只做桌面装配，没有业务逻辑；`src-tauri/src/editor.rs` 是工作台编辑器命令层（画笔 / 油漆桶 / 结构操作），与主循环共用同一把文档锁 |
| Agent 主循环 | `crates/agent-core/` | prompt 组装、provider 流式、`tool_use` 抽取、工具执行、结果回填、续轮 |
| 像素文档模型 | `crates/pixel-core/` | document 模型、类型化操作、RLE 上下文编码、`.aip` v2、Lua 沙箱着色器、PNG / GIF / 精灵表导出 |
| 前端 | `src/` | React + antd + zustand，只做渲染和输入；`src/lib/bridge.ts` 是唯一的 invoke / event 出口 |

agent-core 不依赖 Tauri，是纯 Rust。它通过一个 `tokio::sync::mpsc` 通道向外吐 `AgentEvent`，由 Tauri 壳转成事件广播，
前端在 `src/lib/transcript.ts` 把事件流折叠成可渲染条目（纯函数，可单测）。

### 四、Agent 怎么工作

一次发送的流程：`prompt admission -> provider 流式输出 -> tool_use -> 工具执行 -> 结果回填 -> 续轮`。
每轮都重新组装系统提示词，因为上一轮的工具可能已经改过 canvas。模型能用的工具有七个：
- `pixel_apply_operations`：一次事务里做一坨类型化操作。图层 / 帧 / 调色板的结构改动走这里（建、复制、挪、删、改名、设时长），也可以用 `set_pixels`、`stamp_grid`、`draw_shape`、`bucket_fill`、`clear_region` 打小补丁。任一操作非法则整事务回滚，错误信息会指出失败的操作下标
- `pixel_run_shader`：一段 Lua 脚本，配一次事务的绘制与动画。带 Loops 与 palette helpers，`animate=true` 时按 `phase`（0..1）驱动每一帧
- `pixel_read_canvas`：读回当前网格，`overview=true` 时给降采样地图，最多读 128x128 的精确窗口
- `pixel_tween_frames`：在两个已存在的帧之间插中间帧。要补间、过渡、或者「从 A 姿态长到 B 姿态」时用。`migrate` 按序翻差异像素（像素画该有的变形）、`blend` 插值颜色、`copy` 是占位，`ease` 会给迁移进度上 smoothstep
- `pixel_pixelize_image`：把一张位图（通常是生图模型的产出，base64 PNG/JPEG）量化成索引像素落到目标 cel，往画布调色板上吸附、尽量复用接近色而不撑爆调色板。用来把生成结果落到网格，而不是一个像素一个像素地描述
- `pixel_generate_image`：让生图模型直接画一张位图、再量化上画布。画刷、细密过渡、偏写实这类 Lua 脚本和类型化 ops 表达不来的走这条。默认覆盖激活 cel；要「改这一帧」就把当前帧 id 透传进 `reference_frame` 当垫图，`spot="new_frame"` 则落到新建帧而不是覆盖
- `pixel_plan`：纠正「这一轮被理解成了什么」。一个 turn 只允许调一次，且必须赶在任何绘制工具之前，其余时候调它只会得到报错

直连生图的传输链见 `crates/agent-core/src/imagegen.rs`：有垫图时优先走 `images/edits` 多段上传，没有垫图时走 `images/generations`，
两者都被端点拒绝（404/405/501）才退回 chat 的 `modalities=["image"]`。这里有一条硬规则——垫图绝不静默丢失：只要调用带了 reference，
就永远不会掉回纯提示词的 `generations`，只能退回把 base64 垫图塞进 content parts 的 chat 通道，宁可报错也不悄悄画一张没参考过的图。
所以「改这一帧」「照这一帧再长一帧」「照示例图画」在任何 OpenAI 风格端点上行为一致。

「模型不许手写矩阵」的契约在 `crates/agent-core/src/tools.rs` 收口：绘制和动画统一走 Lua 沙箱，结构改动统一走 ops，读回统一走 RLE。

#### 本轮分流与知识库

开一个 turn 之前，Rust 先按用户原话把这一轮的方向定一次，再交给模型——这件事在 `crates/agent-core/src/plan.rs`。五件事：参照图是「只借画风」还是「照着实临摹」、成品是瓦片 / 角色 / 场景 / 图标 / 图案 / 道具、风格预设锁不锁（1-bit、Game Boy、NES、PICO-8、CGA、抖动递色、粉彩、高比特）、收尾规矩取哪一档（写实渲染、电影打光、材质区分、细节精修、景深层次、柔边平滑、肌理质感）、这一轮该带哪几条像素画工艺知识。定性的位置放在 Rust 而不放在模型：把预算全花在思考、一个工具都不调的情况真实存在，那时候约束必须已经在路上。

聊天里每轮最多出现一张「理解这一轮」卡片，五件事摆成几行，行尾带上判据——用户原话里定下这件事的那个说法。模型读了觉得不对，可以调一次 `pixel_plan` 纠正；`none` 是显式摘掉上一轮的预设，「不限」也是一条要说出来的结论。

画风与收尾规矩是两根平行的轴，各管一半：画风钉的是硬指标——四级绿就只有四级绿，谁来说情都不改；收尾规矩讲的是另一半，同样四级绿，可以画成一张干瘪的示意图，也可以画成有形体、有材质、有收尾的一张图。两者同时选上时不打架，规则段里写明「色数、描边、抖动听画风的，其余照常上路」，id 撞车的两个（写实、精细）由画风段直接顶替，不发两遍。收尾规矩只认界面上那一下点击：一句话里出现「电影感」不等于要把整轮锁成电影打光——它可能只是在描述题材，所以没有、也不该有 `classify_text`。

知识库是 59 条像素画工艺条目（`crates/agent-core/src/knowledge.rs`），按「技法 -> 素材 -> 生物 -> 器物 -> 场景 -> 工具词」排好——「画只猫」只给上色规则是不够的，模型知道怎么铺色阶，照样能把猫画成四条腿的毯子，所以生物和器物两组讲的是「怎么把一样东西画对」。按用户原话做关键词加权明文检索，命中前几条随系统提示词进本轮。没有向量库、没有嵌入模型，也多不出一个要用户自己去配的服务：搜「瓦片」就该命中瓦片，明文命中就够。美术名词的中英别称表与常用色名表同理（`glossary.rs` / `colornames.rs`），按需检索，不整本塞进提示词。

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

主循环的能力不止七个内置工具。顶栏的插头图标打开「MCP tool servers」面板，可以挂用户自己的 MCP 服务器：stdio（拉起子进程、
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

需要 Node 20+、Rust stable 与 pnpm 12，包管理器用 pnpm。esbuild 的 postinstall 需要放行，
白名单写在根目录 [pnpm-workspace.yaml](pnpm-workspace.yaml) 里，漏了它全新克隆 `pnpm install` 会失败。

```bash
# 前端
pnpm install
pnpm dev            # 仅起 vite（1420 端口），用于调 UI
pnpm build          # tsc -b && vite build
pnpm test           # vitest

# 桌面端
pnpm tauri dev      # 完整桌面调试

`pnpm tauri dev` 自己会再拉一个 vite。要是 1420 已经被 `pnpm dev` 占着，先关掉那个再跑，
否则前端会热更新到新代码、Rust 侧却还是上一个二进制，界面就会出现「Command xxx not found」
这种前后端版本错位的报错。

# Rust 侧
cargo test --workspace
```

模型配置在应用内「设置」里填，落盘在 app config 目录的 `models.json`，`api_key` 只留在本机、不回传 webview。
app config 目录下共有三份 JSON：`models.json`（Provider 与密钥）、`mcp.json`（MCP 服务端）、`recipes.json`（批量配方簿），都只在本机，不进仓库。

#### 打包（`pnpm tauri build`）

`targets` 里三个目标都配好了：macOS 出 `.app` + `.dmg`，Windows 出 NSIS 安装包。

macOS：没有开发者证书也能打，[`tauri.conf.json`](src-tauri/tauri.conf.json) 里 `bundle.macOS.signingIdentity`
取 `"-"`，签名走 ad-hoc（`codesign -s -`）。自签的 `.app` 在 Finder 里能直接打开，不会被当成无名无姓的
未签名产物。`.dmg` 里除了 `.app` 还带一个指向 `/Applications` 的拖拽安装快捷方式，卷图标用根目录的
`icon.png` 转出来的 `.icns`。打 DMG 的机器要同意 Xcode 命令行工具许可，缺了会死在 `SetFile` 那句上。

Windows：NSIS 安装包走 [`src-tauri/nsis/installer.nsi`](src-tauri/nsis/installer.nsi) 这套模板。三处配置：

- `installMode: "perMachine"`：安装时弹 UAC 申请管理员权限，默认装到 `Program Files\AIPixel`，注册表落 HKLM
- `languages: ["SimpChinese"]`：安装界面整站简体中文（连 WebView2 缺失提示也是中文，用 Tauri 自带的 `SimpChinese.nsh`）
- `template: "nsis/installer.nsi"`：左下角不再是 `NullSoft Install System v3.xx`，显示 `AIPixel v0.1.0` 这样的产品名加版本号

模板基线是 Tauri 官方默认 `installer.nsi`（tag `tauri-v2.12.0`），只改了 `BrandingText` 一行。
升级 Tauri 后重新对齐一次官方模板，免得新配置项在这里缺占位符。
模板拿本地 `makensis` 干编译过：占位符按真实构建的数据填充（`installMode=perMachine`、
`languages=SimpChinese`），Windows 专有的 `nsis_tauri_utils` 插件调用打桩，能一路编到输出
安装包退出码 0。没在真机 Windows 上跑过安装流程。

### 七、`example/`：遗留工具链与样例数据

仓库根目录只放桌面端。最初那套 Pillow 脚本、早期网页版像素编辑器和全部样例数据都在
[`example/`](example/README.md) 里，与桌面端没有代码共用：`.aip` 的早期约定从这里来，
桌面端沿用并长成了 v2；工作台的「量化」就是 `img2aip_converter.py` 那条思路的 Rust 实现。

```bash
python3 example/aip_converter.py                 # input/ -> output/
python3 example/img2aip_converter.py             # refer_img/ -> refer_aip/
```

脚本按自己所在目录找数据，整个 `example/` 目录要一起搬；里面有什么、怎么跑，见
[`example/README.md`](example/README.md)。

### 八、路线图

- Agent 会话（已落地）：会话、对话流、七个工具、`.aip` 读写、BYOM 配置
- 工作台（已落地）：Auto / Chat / Ask 三档审批、画笔与橡皮、瓦片底图、帧条缩略图与播放预览、洋葱皮、帧的新建 / 复制 / 删除 / 挪位 / 停留时长、图层倒序列表与显隐 / 不透明度 / 排序、仅限直接编辑的撤销栈、配色范围（六种预设 + 一个任意颜色槽，透明格即擦）、GIF / 整条帧带 / 精灵表 / 当前帧 / Aseprite 导出
- 批量工作台（已落地）：一个文件夹进、一个文件夹出的纯本机批处理，量化（位图到 `.aip`）与导出（`.aip` 到 PNG / GIF）两个方向，扫描先行、单文件失败不中断、回执带成败计数，独立 `batch-event` 通道
- 批量配方簿（已落地）：参数组命名落盘 `recipes.json`，本机存取、覆盖、删除，批量面板开跑前一键回填
- 配方随项目走（已落地）：`.aipr` 纯文本 JSON，单条或整本导出，手上这份没存过也能直接分享；导入时同名不覆盖而是加序号，坏条目单独跳过并在回执里逐条交代
- 下一步：批量脚本化入口（CLI），让 recipe 能在 CI 里复跑

### 发行商

- AIPixel 由 **异猫工作群（Mutantcat Working Group）** 开发并发行，官方网站 [mutantcat.org](https://www.mutantcat.org/)
- 项目源码仓库：[github.com/Mutantcat-Working-Group/AIPixel](https://github.com/Mutantcat-Working-Group/AIPixel)
- 发行版以 GPL-3.0 发布，二进制安装包与源码一一对应；使用中遇到的问题欢迎到仓库提 Issue

### 开源协议

- 著作权归 **异猫工作群（Mutantcat Working Group · mutantcat.org）** 所有，Copyright (C) 2026
- 本项目采用 **GNU General Public License v3.0（GPL-3.0）**，完整条款见仓库根目录 [LICENSE](./LICENSE)
- 每个源文件顶部都有两行许可头：`Copyright (C) 2026 Mutantcat Working Group` 加 `SPDX-License-Identifier: GPL-3.0-only`，看单文件就能确认授权归属
- 使用、修改、二次分发都自由，但衍生作品必须以 GPL-3.0 同协议继续开源，并保留原版权与许可声明
- 分发二进制或安装包时，须同时提供对应的完整源码；本仓库的发布产物与源码始终一一对应
- `example/` 里的示例、随附脚本与本 README 同样适用本协议
- `.aip` / `.aipr` 数据格式、七个内置工具的命名与主循环结构都是本仓库自己的取舍，不含受第三方协议约束的代码
