<div align=center>
<img src="icon.png" style="width:100px;" width="100"/>
<h2>AI像素画</h2>
<p>AIPixel</p>
</div>

[English](README.en.md) | 简体中文

### 一、产品概述

- AI像素画（AIPixel）是一款跑在本机的像素画 Agent 桌面工具：左侧会话、中间对话、右侧画布，一次对话产出一份 `.aip` 工程。
- **BYOM（自带模型）**：没有内置服务器、没有登录、没有计费。模型只来自你在界面里自己填的 Provider（Anthropic / OpenAI 兼容），地址与密钥只存在本机。
- **每一笔像素都落在受预算约束的沙箱里**：结构改动走类型化操作，绘制与动画跑在 Lua 沙箱，读回用压缩编码，模型不用手写一屏像素矩阵。
- 画布是唯一权威状态：图层、帧、配色范围与文档同源，改一处、撤销一处、导出一处都对得上。
- `.aip` 中间文件是明文文本，宽高、调色板、图层、帧都写在里面，天然适合版本管理与人工微调。
- **发行方** 由异猫工作群（mutantcat.org）发行，GitHub: https://github.com/Mutantcat-Working-Group/AIPixel

核心价值：

- **开箱即用**：桌面版双击就能画，模型配置、MCP 服务器、批量配方全部只在本机，不上传任何服务器。
- **为像素画而生**：把生图收窄成一组受预算约束的工具，模型只管结构、构图、脚本与垫图，改一个像素不用重画整张图。
- **能进游戏工作流**：支持 RPG Maker 行走图网格、Aseprite 导出、批量量化与导出，产物可以直接进引擎。
- **对 AI 友好**：内置美术知识库与行为准则，MCP 双向打通，外部 Agent 可以「建画布、画、导到指定路径」。

### 二、功能说明

#### 工作台

- 左侧会话列表：新建时先填写会话名与画布宽高，支持改名与拖动排序；会话可分别绑定模型与角色。
- 中间对话流：用户消息带附件条，助手消息流式输出，工具调用可展开看入参，推理片段折叠显示。
- 右侧画布与文档面板：图层 / 帧 / 配色范围各一行，可随时切到 `.aip` 原文查看与手改。

#### 画布与动画

- 画笔、填充、橡皮三个工具加撤销，绘制实时落笔，抬笔提交整笔。
- 帧条每格一张缩略图，画法与导出同源，看到的就是会导出的；缩略图下是帧号与停留时长，点哪格跳哪帧。
- 帧的新建、复制、删除、前后挪帧；本地循环播放；洋葱皮把上一帧按 24% 透明度垫在当前帧底下。
- 瓦片底图：把画面平铺成 1x1 / 2x2 / 3x3，瓦片接缝一眼看完，只有中间那格接管指针。
- RPG Maker 角色行走图适配：新建画布可选 144x192（MV/MZ）、96x128（VX/Ace）、128x128（XP）、72x128 四档固定网格，可一键铺纸娃娃白膜，按头发 / 脸 / 衣服 / 裤子 / 鞋等部件分区上色。

#### 图层与配色范围

- 图层列表按栈顶在上倒序排列（与 Aseprite / Photoshop 一致），每行带显隐开关，支持重命名与顺序调整，下方是当前图层不透明度。
- 配色范围分预设（Gray 8 / Sweetie 16 / DawnBringer 16 / PICO-8 / Game Boy / 1-bit）与自定义颜色。
- 预设可选「配色锁」：锁住后 AI 只能在这个范围内取色，解锁后 AI 可以扩展配色表。
- 预设与自定义颜色都支持新增与删除，删除颜色时提示在当前已选色里挑一个替换色。
- 每个图层的配色范围独立；换预设时已有像素按 CIELAB 就近色重映射，画面留住、颜色归队。

#### 导入与导出

- 编码全在 Rust 侧完成，合成原料是调色板索引而不是位图像素，导出的和画布上看到的是同一份东西。
- 支持 GIF（无限循环，帧延时取文档自己的时长）、当前帧 PNG、横向整条 PNG、精灵表 PNG、Aseprite `.aseprite`（图层与帧语义原样保留）。
- 导入支持位图量化进画布、`.aip` 工程导入、参考图 / 参考视频与生图结果进画布，可缩放与栅格化。
- 导入与导出统一收在顶部工具栏右上角，不占右侧菜单。

#### 工作流与批量

- 工作流坞按当前模型能力自动判断可用性：智能体绘制、图像生成、参考图简报、视频抽帧、视频运动简报、补间帧、提示词微调、位图量化。
- 批量工作台：一个文件夹进、一个文件夹出的纯本机批处理，支持「参考图 → `.aip`」量化与「`.aip` → PNG / GIF」导出，跑完带成败计数与输出目录。
- 配方簿：把调顺的参数存进本机，可删可覆盖，可按 `.aipr` 单条或整本导出，方便分享与复跑。

#### 模型、MCP 与护栏

- 模型设置里填 Provider、接口地址、密钥与模型名，可配多个模型并切换激活；每个会话可单独绑定模型。
- 权限档位三档：Auto 每个调用直接执行；Ask 每个调用先停下来等你批准；Chat 只拦写操作，只读调用直接过。
- 可作为 MCP 客户端挂载外部 MCP 服务器（stdio 与 HTTP 两种传输），外部工具以命名空间进入每轮调用。
- 也可作为 MCP 服务端，被外部 AI 驱动「创建 → 绘制 → 导出」，详见第五节。
- 护栏：单轮工具步数、续轮次数、回灌字节数可设，失败重试带退避与防死锁检测，流式期间随时可以中断。

#### 内置美术知识库

- 三块内容随提示词动态下发：像素画知识（动画与瓦片、配色与描边、光影与形体、特效与材质、动物与物件描述、成套素材与作画流程）、美术术语中英双语表、常见颜色中英文名表。
- 检索是明文匹配：按用户这句话命中的触发词加权召回，无关内容不塞进提示词。
- 三条行为准则跟着动笔走：画图与改画时注入「先锁规格再开画」等规则，纯问答不注入。
- 检索结果会显示在对话的计划节点上，用户看得出这一轮参考了哪几条。逐条来源见 [docs/KNOWLEDGE_SOURCES.md](docs/KNOWLEDGE_SOURCES.md)。

### 三、安装与下载

桌面版从 [Releases](https://github.com/Mutantcat-Working-Group/AIPixel/releases/latest) 下载最新安装包，版本号形如 `1.0.20261019`（小版本加构建日期）：

| 平台 | 安装包 |
| --- | --- |
| Windows x64 | `AIPixel_<版本>_x64-setup.exe` |
| Windows arm64 | `AIPixel_<版本>_arm64-setup.exe` |
| macOS（Intel） | `AIPixel_<版本>_x64.dmg` |
| macOS（Apple Silicon） | `AIPixel_<版本>_aarch64.dmg` |
| macOS（universal） | `AIPixel_<版本>_universal.dmg` |
| Linux x86_64 | `AIPixel_<版本>_amd64.AppImage` |
| Linux aarch64 | `AIPixel_<版本>_aarch64.AppImage` |

- Windows 为 NSIS 安装包，申请管理员权限安装，安装界面为简体中文。
- macOS DMG 为 ad-hoc 签名，内含 Applications 拖放快捷方式。
- Linux 为 AppImage，赋予执行权限后双击运行。
- 每个 Release 附带 `checksums.txt`、`checksums-md5.txt`、`checksums-sha1.txt`，可核对下载文件完整性。

### 四、快速上手

1. 安装并启动桌面版，新建会话时填写会话名与画布宽高（会话名不填则默认 s1、s2…）。
2. 打开 **设置 → 模型**，填入自己的 Provider 接口地址、API Key 与模型名；点「获取」可从接口拉取模型列表。
3. 在对话框里描述你想要的像素画，例如「画一只 32x32 的骑士 idle 姿态，四级绿」，模型会按受预算约束的方式落到画布。
4. 在右侧画布检查效果：切帧看动画、开洋葱皮对位、在配色范围里换预设或吸取单色。
5. 需要动画就新建帧让模型补间，或自己用帧条与播放逐帧调。
6. 满意后用右上角导出菜单输出 GIF / PNG / 精灵表 / Aseprite，或把 `.aip` 另存到磁盘。
7. 手上有一批参考图要转成 `.aip`，或一批 `.aip` 要出 PNG / GIF，切到右侧「批量」一栏，选文件夹、扫一遍、开始。

### 五、MCP 与外部集成

本程序既可以是 MCP 客户端（挂别人的 MCP 服务器），也可以作为 MCP 服务端被外部 AI 驱动。服务端对外暴露完整的画布闭环——**创建 → 绘制 → 导出到指定路径**，用一个外部 Agent 加一个游戏引擎，就能让 AI 自己画素材、自己落盘、引擎直接加载。

开关在 **设置 → MCP**，默认关闭；监听地址固定为本机回环，只响应 `127.0.0.1`，不对外网开放；端口默认 `7815`，填 `0` 表示让内核挑一个空闲端口。

对外一共 18 个工具，分四类：

- 画布会话：`list_sessions`、`create_canvas`、`drop_canvas`、`rename_canvas`、`get_canvas`、`canvas_preview`
- 绘制：`paint_stroke`、`fill_region`、`apply_ops`、`resize_canvas`、`lay_paperdoll_base`
- 文件：`list_export_formats`、`export_canvas`、`save_project`、`import_project`、`import_image`
- 智能体：`prompt_agent`、`interrupt_agent`

传输是手写的 HTTP/1.1 + JSON-RPC 2.0，一条连接一次请求。先用 curl 探一下活：

```bash
curl http://127.0.0.1:7815/
```

建一个 64x64 的新画布、画一笔、导成 Aseprite：

```bash
curl -X POST http://127.0.0.1:7815/ -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"create_canvas","arguments":{"width":64,"height":64,"name":"hero"}}}'

curl -X POST http://127.0.0.1:7815/ -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"paint_stroke","arguments":{"id":"hero","from_x":8,"from_y":32,"to_x":56,"to_y":32,"color":"#e8823a","size":2}}}'

curl -X POST http://127.0.0.1:7815/ -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"export_canvas","arguments":{"id":"hero","format":"aseprite","path":"/tmp/AIPixel/hero.aseprite"}}}'
```

Node 侧一次完整闭环——建画布、画一笔、导进引擎资源目录：

```js
const ENDPOINT = "http://127.0.0.1:7815/";
let seq = 0;

async function call(name, args) {
  const res = await fetch(ENDPOINT, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: ++seq, method: "tools/call",
                           params: { name, arguments: args } }),
  });
  const payload = await res.json();
  if (payload.error) throw new Error(`${payload.error.code}: ${payload.error.message}`);
  // 服务器自己的失败走的是 HTTP 200 + isError，得看 content，不能只看 error。
  if (payload.result?.isError) throw new Error(payload.result.content[0].text);
  return payload.result;
}

await call("create_canvas", { width: 64, height: 64, name: "hero" });
await call("paint_stroke", { id: "hero", from_x: 8, from_y: 32, to_x: 56, to_y: 32,
                             color: "#e8823a", size: 2 });
await call("export_canvas", { id: "hero", format: "png", path: "game/assets/hero.png" });
```

`prompt_agent` 是把整个主智能体交出去的那一个工具：外部 AI 描述需求，本程序的模型负责拆解、绘制、自检。实现见 [src-tauri/src/mcp_server.rs](src-tauri/src/mcp_server.rs)，与用户自配的外部 MCP 服务器（`src-tauri/src/mcp.rs`）是两套独立机制，互不影响。

### 六、数据与隐私

- 模型配置（Provider、接口地址、API Key）落盘在系统应用配置目录的 `models.json`，密钥只留在本机、不回传 WebView。
- 同目录下还有 `mcp.json`（MCP 服务器）与 `recipes.json`（批量配方簿）。
- `.aip` / `.aipr` 是明文文本，想 diff、想手工微调、想进 Git 都行。
- 软件不内置任何服务器、不发起除你配置的模型接口与 MCP 服务器之外的网络请求；视频抽帧、补间帧与批量量化都在本机完成。

### 七、开发与构建

环境要求：Node.js 20+、Rust stable、pnpm 12，包管理器用 pnpm。esbuild 的 postinstall 需要放行，白名单写在根目录 [pnpm-workspace.yaml](pnpm-workspace.yaml) 里。

```bash
pnpm install
pnpm dev            # 仅起 vite（1420 端口），用于调 UI
pnpm test           # vitest
pnpm tauri dev      # 完整桌面调试
cargo test --workspace
```

`pnpm tauri dev` 自己会再拉一个 vite。要是 1420 已经被 `pnpm dev` 占着，先关掉那个再跑，否则会出现「Command xxx not found」这种前后端版本错位的报错。

推送 `v*` 格式的 tag（例如 `v1.0.20261019`）即触发 `.github/workflows/release.yml`，六路并行构建桌面安装包（macOS dmg、Windows NSIS、Linux AppImage / deb），统一计算校验值和发布 Release。`.github/workflows/ci.yml` 负责每次推送的 Rust 测试、格式化、Clippy，以及前端 lint、单测与构建。

项目结构：

```text
crates/pixel-core    # 像素文档模型、类型化操作、RLE 编码、Lua 沙箱、PNG / GIF / 精灵表导出
crates/agent-core    # Agent Rust 主循环：prompt、provider 流式、tool_use 抽取、工具执行、续轮
src-tauri            # Tauri 桌面壳：命令层、状态托管、事件广播、MCP 客户端与服务端
src                  # React 薄壳：只做渲染和输入
example              # 遗留 Python 工具链与样例数据，桌面端不依赖
```

### 八、开发进度

- [X] 桌面端（Tauri 2 + React + Ant Design）
- [X] Rust Agent 主循环与流式输出
- [X] 画布、图层、帧与动画播放
- [X] 配色预设、配色锁与图层独立配色范围
- [X] 撤销 / 重做与每一步操作缓存
- [X] 多格式导出（PNG / GIF / 精灵表 / Aseprite）
- [X] 位图量化导入与参考图 / 视频工作流
- [X] 内置美术知识库与意图分流
- [X] MCP 客户端与服务端双向打通
- [X] 批量工作台与配方簿
- [X] 六平台 CI 自动打包与发布

[GPL-3.0](LICENSE)

---

## 致谢

本项目由异猫工作群（[mutantcat.org](https://www.mutantcat.org/)）开发并发行。内置美术知识库参考了 MakeBead、pixel-asset-master-skills、saint11、Pedro Medeiros、Derek Yu、Cure / Pixel Joint、Slynyrd、Lospec、Concept Art Empire 等公开教程与素材，也感谢 [IPaperDoll](https://github.com/Mutantcat-Working-Group/IPaperDoll) 以及 Aseprite、PixTXT 带来的思路启发。逐条出处与说明见 [docs/KNOWLEDGE_SOURCES.md](docs/KNOWLEDGE_SOURCES.md)。
