<div align=center>
<img src="icon.png" style="width:100px;" width="100"/>
<h2>AI像素画</h2>
<p>AIPixel</p>
</div>

[English](README.en.md) | 中文

### 一、产品概述

- AI像素画（AIPixel）是一款跑在本机的像素画 Agent 桌面工具：左侧会话、中间对话、右侧画布，一次对话产出一份 `.aip`。
- **BYOM（自带模型）**：没有内置服务器、没有登录、没有计费。模型只来自你在界面里自己填的 Provider（Anthropic / OpenAI 兼容），地址与密钥只存在本机。
- **每一笔像素都落在受预算约束的沙箱里**：模型不许凭感觉手写像素矩阵，结构改动走类型化操作，绘制与动画跑在 Lua 沙箱，读回用压缩编码。
- 中间文件 `.aip` 是明文文本：宽高、调色板、图层、帧都写在文本里，天然适合版本管理和人工微调。
- **发行方** 由异猫工作群（mutantcat.org）发行，GitHub: https://github.com/Mutantcat-Working-Group

核心价值：

- 让模型画像素画，最怕它一口气「手写」一屏色号，改一个像素要重画整张图。这里把生图路径收窄成七条受预算约束的工具，模型的自由度留给结构、构图、脚本和垫图。
- 画布不是给人看的截图，而是唯一权威状态：图层、帧、配色范围都是一等公民，改一处、撤销一处、导出一处，前后端是同一份东西。
- 桌面版双击即用，模型配置、MCP 服务器、批量配方全部只在本机，不上传任何服务器。
- <img width="64" height="64" alt="untitled" src="https://github.com/user-attachments/assets/dbb4620f-4560-4db6-b90c-9be20f3c629c" />

### 二、功能说明

#### 工作台三栏

- 左侧会话列表：新建时先选画布宽高，支持改名与拖动排序；会话可分别绑定模型与角色。
- 中间对话流：用户消息带附件条，助手消息流式输出，工具调用可展开看入参，推理片段折叠显示。
- 右侧画布与文档面板：图层 / 帧 / 配色范围各一行，可随时切到 `.aip` 原文查看与手改。

#### 画布与动画

- 画笔、填充、橡皮三个工具加撤销，抬笔才把整笔提交，笔迹先在画布上增量预览。
- 帧条每格一张缩略图，画法与导出同源，看到的就是会导出的；缩略图下是帧号与停留时长，点哪格跳哪帧。
- 帧的新建、复制、删除、前后挪帧；播放用轻量本地循环，不逐帧打后端；洋葱皮把上一帧按 24% 透明度垫在当前帧底下，逐帧对位有依据。
- 一档瓦片底图：把画面平铺成 1x1 / 2x2 / 3x3，瓦片接缝一眼看完，只有中间那格接管指针。
- RPG Maker 角色行走图适配：新建画布可选 144x192（MV/MZ）、96x128（VX/Ace）、128x128（XP）、72x128 四档固定网格，引擎按行列号切图，选错一格导出的图就错位一行；创建时勾一下「铺白膜」，就用几何算出四向 x 3 或 4 列的人形剪影，按头发 / 脸 / 衣服 / 裤子 / 远侧肢体 / 鞋与五官分区给灰阶——分区就是将来换色的边界。工具栏小人图标随时补铺，一步撤销回来。

#### 图层

- 图层列表按栈顶在上倒序排列（与 Aseprite / Photoshop 一致），每行带显隐开关。
- 行尾箭头把选中图层沿绘制顺序挪一格，列表下方是当前图层不透明度。
- 索引 0 恒为透明，调色板行首那颗透明格就是擦除落点。

#### 配色范围

- 六套预设：Gray 8 / Sweetie 16 / DawnBringer 16 / PICO-8 / Game Boy / 1-bit，另加一个「任意颜色」槽。
- 换预设不是换着玩：已有像素会按 CIELAB 就近色重映射进新范围，画面留住、颜色归队。
- 拖动取色实时预览、松手才算落，工具按钮轻点即复制色值。

#### 导出

- 编码全在 Rust 侧完成，合成原料是调色板索引而不是位图像素，所以导出的和画布上看到的是同一份东西。
- 支持 GIF（无限循环，帧延时取文档自己的时长）、当前帧 PNG、横向整条 PNG、所有帧并排一张、精灵表 PNG、Aseprite `.ase`（图层与帧语义原样保留）。

#### 工作流坞

- 七条工作流，按当前模型的能力自动判断可用性：模型能力不够的条目照样列出，只是禁用并写清缺什么。
- 智能体绘制：对话即绘制，每次编辑跑一个沙箱 Lua 脚本。
- 图像生成：模型渲染一张位图再量化到画布网格，支持垫图。
- 参考图简报：读图模型把参考图读成结构化简报，再照着画。
- 视频抽帧：抽帧后逐帧纯本机量化，全程不调模型、一个 token 都不花。
- 视频运动简报：读视频模型把一段视频读成可编辑的运动简报，产物是文本不是帧。
- 补间帧：在两个已有帧之间本机插值。
- 提示词微调：把一句大白话改写成结构化生图提示词。
- 外加一条挂在坞末尾的量化：把一张位图丢到网格上，纯本机运行。

#### 批量工作台

- 一个文件夹进、一个文件夹出的纯本机批处理，与会话互补：会话是一两张图一个模型盯着，这里是确定性重复劳动，一个 token 都不花。
- 两个方向：量化把参考图批量反查成 `.aip`，导出把 `.aip` 批量渲染成 PNG / GIF。
- 开跑前先扫描一遍报出候选文件数；过程走独立事件通道逐文件折进行表，单个文件失败只记原因不中断，跑完回执带成败计数与输出目录。
- 同样的输入与同样的配方得到同样的输出，方便复跑与版本管理。

#### 配方簿

- 调顺的参数组起个名字就能存进本机配方簿，下次直接取，可删可覆盖。
- 配方可用 `.aipr` 单条或整本导出：伸手给对方的就是一个能在任何编辑器里读的 JSON，手上这份没存过也能直接分享。
- 导入时同名不覆盖而是加序号，坏条目单独跳过并在回执里逐条交代。

#### 模型与 MCP

- 模型设置里填 Provider、接口地址、密钥与模型名，可配多个模型并切换激活；每个会话可单独绑定模型。
- 权限档位三档：Auto 每个调用直接执行；Ask 每个调用先停下来等你批准；Chat 只拦写操作，只读调用直接过。
- 「这次别烦我」只把当前轮降级成 Auto，不写回会话配置。
- 主循环能力不止内置工具：顶栏插头图标打开 MCP 工具服务器面板，可挂自己的 MCP 服务器，stdio（拉起子进程）与 HTTP 两种传输都支持。
- MCP 服务器上的工具以命名空间进入每轮调用，和内置像素工具并列交给模型，同样过审批闸门。
- 反方向也算：本程序自己还是个 MCP 服务端，外部 AI 连进来就能「建画布、画、导到指定路径」，详见第十节。

#### 预算护栏

- 单轮工具步数、续轮次数、回灌字节数都可设；同一个失败调用连续多次自动退避。
- 流式期间随时可以中断，中断立即收尾，不会把后半场卡死。

#### 内置美术知识库

- 三块内容随提示词动态下发：**132 条像素画知识**（动画与瓦片、配色与描边、光影与形体、动物与物件描述、成套素材与作画流程）、**73 条美术术语的中英双语表**（带别称，「色阶」对 ramp、「勾线」对 outline）、**常见颜色中英文名表**（按色相分簇，需要时让模型自己点名字而不是猜十六进制）。
- 检索是明文匹配：用户这句话里出现了哪几个触发词，就按词长加权把那几条知识塞进提示词，没提到的宁可整段不给——提示词塞满概念只会稀释后面真正相关的部分。
- 三条行为准则跟着动笔走：要图、改画都发，纯问答一条不带——护栏跟着画笔走，不跟着触发词走，「别画成程序化假图案」这种话没人会主动说，等触发词命中就等于永远不发。「先锁规格再开画」要求把画布尺寸、palette 十六进制、画风、目标平台、帧数表写在第一颗像素之前，防止同一批资产里颜色走样、尺寸漂移；护栏另走一份字符预算，不占检索条目那 4 个名额，所以「画 5 帧奔跑」照样带得上步态相位表。
- 检索结果会显示在对话的计划节点上，用户看得出来「这一轮参考了哪几条」，不是个黑盒。

### 三、安装与下载

桌面版从 [Releases](https://github.com/Mutantcat-Working-Group/AIPixel/releases) 下载，版本号形如 `1.0.20261009`（小版本加构建日期）：

| 平台 | 安装包 |
| --- | --- |
| Windows x86_64 | `AIPixel_<版本>_x64-setup.exe` |
| Windows arm64 | `AIPixel_<版本>_arm64-setup.exe` |
| macOS（Intel） | `AIPixel_<版本>_x64.dmg` |
| macOS（Apple Silicon） | `AIPixel_<版本>_aarch64.dmg` |
| macOS（universal） | `AIPixel_<版本>_universal.dmg` |
| Linux x86_64 | `AIPixel_<版本>_amd64.AppImage` |
| Linux aarch64 | `AIPixel_<版本>_aarch64.AppImage` |

Windows 为 NSIS 安装包（perMachine，简体中文安装界面）；macOS DMG 为 ad-hoc 签名，含 Applications 拖放快捷方式；Linux 为 AppImage，赋予执行权限后双击运行。每个 Release 同时附带 `checksums.txt`、`checksums-md5.txt`、`checksums-sha1.txt` 三个校验文件，可核对下载文件完整性。

### 四、快速上手

1. 安装并启动桌面版，新建会话时先选画布宽高。
2. 点顶栏「模型设置」，填入你自己的 Provider 接口地址、API Key 与模型名称，可先测试连接。
3. 在对话框里描述你想要的像素画，例如「画一只 32x32 的骑士 idle 姿态，四级绿」，模型会按受预算约束的方式落到画布。
4. 在右侧画布检查效果：切帧看动画、开洋葱皮对位、在配色范围里换预设或吸取单色。
5. 需要动画就新建帧让模型补间，或自己用帧条与播放逐帧调。
6. 满意后用右上角导出菜单输出 GIF / PNG / 精灵表 / Aseprite，或把 `.aip` 另存到磁盘。
7. 手上有一批参考图要转成 `.aip`，或一批 `.aip` 要出 PNG / GIF，切到右侧「批量」一栏，选文件夹、扫一遍、开始。

### 五、数据存储与隐私

- 模型配置（Provider、接口地址、API Key）落盘在系统应用配置目录的 `models.json`，密钥只留在本机、不回传 WebView，可随时修改或清空。
- 同目录下还有 `mcp.json`（MCP 服务器，`env` / `headers` 只写本机，界面只回键名不回值，编辑时值留空表示沿用旧值）与 `recipes.json`（批量配方簿）。
- `.aip` / `.aipr` 是明文文本，想 diff、想手工微调、想进 Git 都行。
- 软件不内置任何服务器、不发起除你配置的模型接口与 MCP 服务器之外的网络请求；视频抽帧与补间帧在本机完成。

### 六、本地开发与构建

环境要求：Node.js 20+、Rust stable、pnpm 12，包管理器用 pnpm。esbuild 的 postinstall 需要放行，白名单写在根目录 [pnpm-workspace.yaml](pnpm-workspace.yaml) 里，漏了它全新克隆 `pnpm install` 会以 `ERR_PNPM_IGNORED_BUILDS` 失败。

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

`pnpm tauri dev` 自己会再拉一个 vite。要是 1420 已经被 `pnpm dev` 占着，先关掉那个再跑，否则前端会热更新到新代码、Rust 侧却还是上一个二进制，界面就会出现「Command xxx not found」这种前后端版本错位的报错。

#### 打包（`pnpm tauri build`）

`tauri.conf.json` 的 `bundle.targets` 已配好 dmg / app / nsis / appimage / deb：macOS 出 `.app` + `.dmg`，Windows 出 NSIS 安装包，Linux 出 AppImage / deb。

- macOS：没有开发者证书也能打，`bundle.macOS.signingIdentity` 取 `"-"`，签名走 ad-hoc（`codesign -s -`）。自签的 `.app` 在 Finder 里能直接打开，不会被当成无名无姓的未签名产物。打 DMG 的机器要同意 Xcode 命令行工具许可，缺了会死在 `SetFile` 那句上。
- Windows：NSIS 模板是 [src-tauri/nsis/installer.nsi](src-tauri/nsis/installer.nsi)，配了 `installMode: "perMachine"`（弹 UAC、装到 `Program Files\AIPixel`）、`languages: ["SimpChinese"]`（安装界面整站简体中文，连 WebView2 缺失提示也是中文），左下角显示产品名加版本号。模板基线是 Tauri 官方默认 `installer.nsi`（tag `tauri-v2.12.0`），只改了 `BrandingText` 一行；升级 Tauri 后需重新对齐一次官方模板。

### 七、CI 与自动发布

推送 `v*` 格式的 tag（例如 `v1.0.20261009`）即触发 `.github/workflows/release.yml`：

1. 六路并行构建桌面安装包：macOS x86_64 / aarch64（dmg）、Windows x86_64 / arm64（NSIS）、Linux x86_64 / aarch64（AppImage / deb）。
2. 同步构建 `example/` 下遗留工具链的 Python 包与源码 tar。
3. 资产齐了之后统一计算 `checksums.txt`、`checksums-md5.txt`、`checksums-sha1.txt`，与全部产物一起发布到 GitHub Release。

`.github/workflows/ci.yml` 负责每次推送的 Rust 工作区测试、格式化与 Clippy，以及前端的 lint、单测与构建。

### 八、项目结构

```text
.
├── icon.png                     # 应用与 README 图标
├── package.json                 # 版本号唯一出处，构建期烘成常量
├── pnpm-workspace.yaml          # pnpm 12 构建脚本白名单（esbuild postinstall）
├── Cargo.toml                   # Rust workspace 清单
├── crates
│   ├── pixel-core               # 像素文档模型、类型化操作、RLE 编码
│   │                            # .aip v2、Lua 沙箱、PNG / GIF / 精灵表导出
│   └── agent-core               # Agent Rust 主循环：prompt、provider 流式、
│                                # tool_use 抽取、工具执行、结果回填、续轮
├── src-tauri                    # Tauri 桌面壳：状态托管、命令注册、事件广播
│   ├── src
│   │   ├── commands.rs          # 会话 / 模型 / 文档 / 导出命令层
│   │   ├── editor.rs            # 工作台编辑器命令层（画笔 / 填充 / 结构操作）
│   │   ├── workflow.rs          # 七条工作流的命令层
│   │   ├── mcp.rs               # MCP 工具服务器配置与命令
│   │   ├── batch.rs             # 批量工作台与配方簿落盘
│   │   └── state.rs             # models.json / mcp.json / recipes.json 的读写
│   ├── nsis/installer.nsi       # Windows NSIS 自定义模板
│   ├── capabilities/            # Tauri 权限清单
│   └── tauri.conf.json          # 应用与打包配置
├── src                          # React 薄壳：只做渲染和输入
│   ├── ui                       # 会话 / 对话 / 画布 / 工作流坞 / 批量 / MCP 面板
│   ├── lib                      # invoke 与事件的唯一出口，加各域纯函数（可单测）
│   ├── assets
│   ├── App.tsx  main.tsx  styles.css
├── example                      # 遗留 Pillow 工具链与样例数据，桌面端不依赖
├── .github/workflows
│   ├── ci.yml                  # Rust 测试 / 格式化 / Clippy + 前端 lint / 测试 / 构建
│   └── release.yml             # 六平台打包 + checksums + Release
└── vite.config.ts               # vite / vitest 配置，版本号从 package.json 烘入
```

### 九、开源协议与致谢

- 著作权归 **异猫工作群（Mutantcat Working Group · mutantcat.org）** 所有，Copyright (C) 2026。
- 本项目采用 **GNU General Public License v3.0（GPL-3.0）**，完整条款见仓库根目录 [LICENSE](LICENSE)。
- 每个源文件顶部都有两行许可头：`Copyright (C) 2026 Mutantcat Working Group` 加 `SPDX-License-Identifier: GPL-3.0-only`，看单文件就能确认授权归属。
- 使用、修改、二次分发都自由，但衍生作品必须以 GPL-3.0 同协议继续开源，并保留原版权与许可声明。
- 分发二进制或安装包时，须同时提供对应的完整源码；本仓库的发布产物与源码始终一一对应。
- `example/` 里的示例、随附脚本与本 README 同样适用本协议。
- `.aip` / `.aipr` 数据格式、内置工具的命名与主循环结构都是本仓库自己的取舍，不含受第三方协议约束的代码。

本项目由异猫工作群（mutantcat.org）开发并发行，官方网站 [mutantcat.org](https://www.mutantcat.org/)，使用中遇到的问题欢迎到仓库提 Issue。

#### 感谢与知识来源

内置美术知识库不是凭印象写的，下面几份公开教程与素材给了它骨架：

- [MakeBead 像素画教程](https://makebead.com/zh-Hans/how-to-make-pixel-art/)：中文像素画教程，滤镜与色相偏移、拼豆与刺绣这类实体媒介、电子表格编号填格、以及面向新手的分级练习清单都出自它。该教程参考 Saultoons《The Ultimate Pixel Art Tutorial》重新撰写。
- [MakeBead 像素画灵感与风格清单](https://makebead.com/zh-Hans/pixel-art-ideas/)：带现成 hex 值分的风格与题材清单，风格预设里的色号参照了它。
- [MakeBead 电子表格像素画](https://makebead.com/zh-Hans/spreadsheet-pixel-art/)：把像素画当编号表格填的整套方法与注意点，对应知识库里的电子表格条目。
- [pixel-asset-master-skills](https://github.com/424431185/pixel-asset-master-skills)：像素美术执行技能，规格锁、交付自检、每件重读规格这套「先别画错，再画得好」的行为准则来自它。
- Lospec：调色板库的命名、色数与推荐场景（DB32 / Endesga / Resurrect 等）的对照依据。
- [Cure《The Pixel Art Tutorial》（Pixel Joint）](https://pixeljoint.com/forum/forum_posts.asp?TID=11299)：抗锯齿的过量、不足与 AA 色带，抖动的图案分类（50%、交织、风格化、随机），banding 的四种形态（贴边、肥像素、隔空、45 度带），sel-out 只在背景已知时才成立，调色板的过饱和与明度跨度，以及「像素画是单像素级控制、油漆桶与直线工具可用、自动模糊涂抹不算、绝不导出 JPG 而是 PNG/GIF」这组定义与导出红线，都从这篇整理进来。它本身是 Pixelation「Ramblethread」的通俗版。
- [Derek Yu：像素画基础](http://derekyu.com/makegames/pixelart.html)与其「常见错误」一章：粗轮廓 → 清理 → 铺色 → 明暗 → AA → sel-out → 收尾的起手顺序，「厚像素」规则（只有一像素粗的部件画不了明暗），以及镜像翻转、去色看明暗这两道终检出在这里；局部色写实、枕形阴影、等距色带三条忌讳也对照进了相应条目。
- [saint11 像素画教程系列](https://saint11.art/blog/pixel-art-tutorials/)：知识库的题材覆盖度按它那套 512x512 单页教程的目录逐项核对，动作、特效、等轴、瓦片、植被、布料等都对得上；打击、爆炸、烟尘与拖尾这一组特效规则，以及俯视四方向角色的朝向约定，就是从它的特效篇和俯视篇整理进来的。亚像素位移、缓动、无缝循环、动态模糊、动画规划、模块化动画、破对齐、可爱比例、暗部、布光方案、1-bit、故障效果与作画管线这组单页技巧也已逐条吸收。
- [saint11《Pixel Art Articles》入门合集](https://saint11.art/pixel_articles/)：站内可读的六篇逐条核对并吸收。[《3 - A Basic Aseprite Animation》](https://saint11.art/pixel_articles/article3/)的逐帧与关键帧两条路线、先画静帧定细节上限、4/3/2/1 的重力节奏加落地保持帧、挤压拉伸保体积、起跳预备与循环接帧，整理成了动画原理条目，它给出的「保留可编辑源文件，按用途导出 GIF 或 PNG 序列 / 图集」也整理成了动画交付条目；[《4 - Basic Shading》](https://saint11.art/pixel_articles/article4/)的七类光术语、软硬投影之分、浅角度受光变暗、平面保持纯色与照片参考六步流程，整理成了明暗流程条目；[《5 - Anti-Alias and Banding》](https://saint11.art/pixel_articles/article5/)里「只缓冲超过一像素的阶梯、AA 可以吃掉形体自身像素、AA 方向随坡度」这几条补进了抗锯齿条目；[《6 - Basic Color Theory》](https://saint11.art/pixel_articles/article6/)里「同明度下黄橙最亮、蓝紫最暗，要按感知而非数值补差」与「控制色相数量」两条补进了色相关系条目。第 7《Working with lines》的「线条是人工产物、低分辨率下尽量少画、多余线条并进阴影、三到四像素的方角要删一个像素修圆、方线风格反过来把对角连接补成方角、彩色描边取邻色加深、外轮廓保持黑色」与六步线稿流程，整理成了线条取舍与线稿起稿两条；第 8《Saving and Exporting》的位图与矢量之别、无损与有损压缩、保存与导出分离、只能整数倍缩放、精确尺寸用「较小整倍 + 扩画布」凑、导出可挑图层与帧与动画方向、Twitter 最后一帧时长减半、引擎只收 PNG 序列或图集且不预先放大、序列名从零开始补两位、多动画用 tag 命名、图集要带 padding/pivot/裁剪/挤出与 JSON、切片批量导出，也都并进动画交付条目。至此这套入门合集从第 1 篇到第 8 篇都已逐条吸收。
- [Pedro Medeiros《How to start making pixel art》#1 与 #2（Pixel Grimoire）](https://saint11.art/pixel_articles/)：16x16 配 4 色的起步练习、像素完美线与「铅笔恒为 1 像素」，受限调色板下靠借用相邻色相上光投影（冷色作暗、暖色作亮），孤立像素的三种正当用途，只按整数倍缩放，以及色簇起稿（大色块起稿 → 由远及近逐步细化 → 推像素修锯齿）这套流程，都是从这两篇整理进来的；第 1 篇开头「像素画只是另一种绘画媒介，解剖、透视、光影、色彩理论与美术史一样要练」也补进了像素画定义条目。
- [Slynyrd / Raymond Schlitter《Pixelblog 60 - Side View Run 'N Gun》](https://www.slynyrd.com/blog/2026/1/26/side-view-run-n-gun)：8x16 白膜尺寸、contact/down/pass/up 四姿势的八帧行走、由行走派生的奔跑（前倾、加大步幅与弹跳、加快播放），上下半身分层加三帧射击覆盖层，跳跃与落地的经济做法（含按坠落距离动态化的落地延迟）、长枪枪口朝下的持握与男女造型差异落在神态而非解剖、宽松衣物与头发各自的次级动作、忙背景配强对比调色板时改回纯黑描边的例外，120ms / 60ms 的节奏锚点，以及 8x8 横版瓦片 3x3 加 4x4 的 12 块最小套（含横版先按明度分层再上细节的辨识度约束），都从这篇吸收进对应条目。
- [Slynyrd《Pixelblog 58 - Top Down Character Animation Part 3》](https://www.slynyrd.com/blog/2025/10/2/pixelblog-58-top-down-character-animation-part-3)：八方向动作的 36 帧规划、先动画白膜再上服装、装备独立成层、持重物时收小手臂摆幅这类俯视角色规则出自它。
- [Slynyrd《Pixelblog 59 - Tiny Sci-Fi Pixels》](https://www.slynyrd.com/blog/2025/11/28/pixelblog-59-tiny-sci-fi-pixels)：8x8 瓦片与 NES 配色、受限调色板下反而要靠描边保可读性、建筑按规则复用而非逐块手绘，对应知识库里的微型瓦片条目。
- [Slynyrd《Pixelblog 61 - Isometric Mecha Tactics》](https://www.slynyrd.com/blog/2026/4/1/pixelblog-61-isometric-mecha-tactics)：32x32 等轴机甲用几何体起形、主动省略读不出的部件、脚部做得夸张以求稳定落点、等轴瓦片顶行重叠保证无缝，都补进了等轴条目。
- [Slynyrd《Pixelblog 62 - Landscape Backgrounds》](https://www.slynyrd.com/blog/2026/5/27/pixelblog-62-landscape-backgrounds)：先铺地平线色带再用空气透视、预算内跨层复用颜色、纹理尺度随距离收敛，以及山谷 / 沙漠 / 森林三种常见变体的配色走向，构成风景背景条目。
- [Slynyrd《Pixelblog 63 - Horizontal Shmup》](https://www.slynyrd.com/blog/2026/7/26/pixelblog-63-horizontal-shmup)：玩家小判定框、机体滚动用平飞加两档倾斜、禁止玩家惯性、敌弹小判定与僚机增强画面存在感，整理成了弹幕射击条目。
- [Slynyrd《Pixelblog 20 - Top Down Tiles》](https://www.slynyrd.com/blog/2019/8/27/pixelblog-20-top-down-tiles)与[《Pixelblog 43 - Top Down Tiles Part 2》](https://www.slynyrd.com/blog/2023/3/29/pixelblog-43-top-down-tiles-part-2)：视觉重量均衡、关键簇只在角上相接、镜面复用出朝向变体、分层拼装纹理、先从正面墙铺起再转 45 度派生侧面与墙顶、投影不超一格，以及沙地 S 形笔触、水面双帧循环与 50% 混合中间帧，都来自这两篇。
- [Slynyrd《Pixelblog 22 - Top Down Character Sprites》](https://www.slynyrd.com/blog/2019/10/21/pixelblog-22-top-down-character-sprites)：精灵尺寸按瓦片单位取整、俯视只允许 Y 轴重叠、小尺寸角色头占身高三分之一到一半、四方向可承载八方向移动、行走帧从奔跑帧里抽掉大步帧放慢播，以及 320x180 / 480x270 / 640x360 这套能整除 1920x1080 的原生分辨率，都整理进了精灵尺寸条目。
- [Slynyrd《Pixelblog 25 - Motion Cycles》](https://www.slynyrd.com/blog/2020/1/28/pixelblog-25-motion-cycles)：行走先上色块人体、逐部件逐帧动画，四足后腿跟对侧前腿错开四分之一拍且前后腿位移必须相等、肩胯反向等量起伏，以及翅膀先画正面线框、标出肩肘折叠点、先只动翅膀再加整体起伏，都补进了步态与鸟类条目。
- [Slynyrd《Pixelblog 41 - Isometric Pixel art》](https://www.slynyrd.com/blog/2022/11/28/pixelblog-41-isometric-pixel-art)：2:1 两步线、斜圆由斜置方框加十字得出、立方体转角风格的可拼性取舍、长方体只要顶面统一就能堆高、36x36 常用等轴瓦片尺寸，以及用一个单独的 2:1 标尺图层核对对齐，都补进了等轴条目。
- [Slynyrd《Pixelblog 44 - Top Down Trees》](https://www.slynyrd.com/blog/2023/5/30/pixelblog-44-top-down-trees)：叶簇的几何单元（菱形、2x2 方块、圆形）、亮中暗三变体、四到五色一簇，以及针叶树先立骨架再自顶向下画枝、反射侧枝、让上层枝给下层枝投影，都补进了树木条目。
- [Slynyrd《Pixelblog 45 - Bricks, Walls, Doors, and More》](https://www.slynyrd.com/blog/2023/7/26/pixelblog-45-bricks-walls-doors-and-more)：16px 砖块加 1px 灰缝只能用 15 / 7 / 3 / 1px 尺寸、通用柱角替代四种定制转角、地面墙面的砖向差异、投影统一不超过一格，以及火焰保持一个 S 形主体再甩出小 S 粒子、6 帧 100ms 配 50ms 高光闪烁，分别进了瓦片与特效条目。
- [Slynyrd《Pixelblog 49 - Realistic Human Anatomy》](https://www.slynyrd.com/blog/2024/3/25/pixelblog-49-realistic-human-anatomy)：八头身约 29x96、六头身约 29x78、肘线对齐肚脐、腕线对齐腹股沟、膝在髋踝中点、肩宽约两头，以及先铺色块 dummy 再上服装，构成写实人体条目。
- [Slynyrd《Pixelblog 50 - Human Walk Cycle》](https://www.slynyrd.com/blog/2024/5/24/pixelblog-50-human-walk-cycle)：真人录像切八帧、contact / down / pass / swing 四阶段镜像复用、头部走三角波而不是正弦、手臂与异侧腿同相位，都补进了步态条目。
- [Slynyrd《Pixelblog 52 - Idle Fighting Stance》](https://www.slynyrd.com/blog/2024/9/26/pixelblog-52-idle-fighting-stance)与[《Pixelblog 53 - Punch and Kick》](https://www.slynyrd.com/blog/2024/11/25/pixelblog-53-punches-and-kicks)：八帧战斗待机以躯干为锚的呼吸起伏、起身慢落身快、拳击与空手道两种站姿、衣发次动作，以及近战六阶段（预备 / 拖影 / 回弹 / 顺势 / 收招 / 过冲）与剑 100/50/50/50/100/50、矛 200/50/50/50/150/50、锤 250/50/50/50/300/100 的毫秒表，整理成战斗待机与近战攻击两条。
- [Slynyrd《Pixelblog 54 - Isometric Pixel Art》](https://www.slynyrd.com/blog/2025/1/23/pixelblog-54-isometric-pixel-art)、[《Pixelblog 55 - Top Down Character Animation》](https://www.slynyrd.com/blog/2025/3/24/pixelblog-55-top-down-character-animation)、[《Pixelblog 56 - Top Down Character Attack Animation》](https://www.slynyrd.com/blog/2025/5/23/pixelblog-56-top-down-character-attack-animation)与[《Pixelblog 57 - Knights, Monsters & Castles》](https://www.slynyrd.com/blog/2025/7/28/pixelblog-57-knights-monsters-amp-castles)：俯视八方向动作的姿势分层与武器换手、城堡从带顶塔楼起手并复用同一组配色，都并入了等轴、俯视与建筑条目。
- [Slynyrd《Pixelblog 17 - Human Anatomy》](https://www.slynyrd.com/blog/2019/5/21/pixelblog-17-human-anatomy)：眼睛位于颅顶到下巴的中线、发际到下巴三等分、鼻底在眼与下巴中点、嘴在鼻底到下巴的三分之一到二分之一处、两眼间距与鼻底宽约等于一眼宽、嘴宽约等于瞳孔距、耳上缘对齐眉线耳下缘对齐鼻底、颈宽约二分之一到三分之二头，以及「线框 → 粗形 → 成品」三遍走、每张姿势先落一笔脊柱手势线，都补进了写实人体条目。
- [Slynyrd《Pixelblog 23 - Parallax Scrolling》](https://www.slynyrd.com/blog/2019/11/12/pixelblog-23-parallax-scrolling)：像素完美移动要求每层每帧位移同一个整数像素；画布宽度取 96 这类能被 1/2/3/4/6/8/12/16/24/32/48 整除的值，循环总帧数等于画布像素宽、图像重复次数等于每帧位移，最远层从 1ppf 起逐层换下一档整除数，慢于 1ppf 需同屏重复两次且易抖，叠在视差上的循环动画帧数必须整除总帧数，都补进了视差条目。
- [Slynyrd《Pixelblog 47 - Tiny Pixels》](https://www.slynyrd.com/blog/2023/11/26/pixelblog-47-tiny-pixels)：8x8 瓦片把环境光遮蔽与投影单独成层就能重组出更多变体（推荐自下而上的层序：土、草、阴影、树石、崖、二次阴影），且 8px 角色移动一个像素就是一整步、几帧即可走完一个循环，补进了微型瓦片条目。
- [Slynyrd《Pixelblog 51 - City Builder》](https://www.slynyrd.com/blog/2024/7/25/pixelblog-51-city-builder)：俯视城镇建筑全部落在与地形一致的 16x16 地基上，建筑高度可在顶端超出地基几像素制造遮挡纵深，但绝不能破掉地基的底边与侧边；从屋顶自上而下起形、檐下与凹处先投后描、用比相邻像素深一两档的微暗描边替代纯黑、周边地产按同一网格铺并投下落地影，构成新的俯视城镇条目。
- [Lospec 像素画教程库](https://lospec.com/pixel-art-tutorials)：教程索引入口，调和与风格类条目的术语对照过它收录的篇目，教程按 walkthrough / animation / shading / tiles 等标签的分类方式也参照了它。
- [Concept Art Empire《How To Make Pixel Art: 40+ Free Video Tutorials》](https://conceptartempire.com/pixel-art-tutorials/)：题材覆盖度按它 40 余个教程的目录核对过一遍，角色、瓦片、等轴、岩石、树木、水面、云、抖动与走路循环都在两边的条目里能对上。
- [IPaperDoll](https://github.com/Mutantcat-Working-Group/IPaperDoll)：异猫工作群的纸娃娃行走图合成器，「先铺白膜、按部件分区、绕锚点换色」这套做法与角色网格规格的参照对象。
- Aseprite 与 PixTXT：`.aseprite` 图层与帧语义、以及索引网格加调色板的中间文件思路的参照对象。

### 十、MCP 服务端模式：让外部 AI 驱动本程序

除了「本程序去连别人的 MCP 服务器」，本程序自己也带一个 MCP 服务端，对外暴露完整的画布闭环——**创建 → 绘制 → 导出到指定路径**。用一个外部智能体（Claude Code、Cursor、自研 agent）加一个游戏引擎，就能让 AI 自己画素材、自己落盘、引擎直接加载，全程不用人盯着。

开关在 **设置 → 模型与 Provider → MCP 工具服务器** 里，默认关闭。理由是它按调用方给的路径写文件，等于把落盘的能力递出去，得由人点头。监听地址固定为本机回环，只响应 `127.0.0.1`，不对外网开放；端口默认 `7815`，填 `0` 表示让内核挑一个空闲端口。

对外一共 18 个工具，分四类：

- 画布会话：`list_sessions`、`create_canvas`、`drop_canvas`、`rename_canvas`、`get_canvas`、`canvas_preview`
- 绘制：`paint_stroke`、`fill_region`、`apply_ops`（点 / 线 / 矩形 / 椭圆 / 填充的批量算子，要么全成要么整体回滚）、`resize_canvas`、`lay_paperdoll_base`（按 4 行角色行走图网格铺纸娃娃白膜，之后按区上色）
- 文件：`list_export_formats`、`export_canvas`（PNG / GIF / 精灵表 / 序列帧 / `.aseprite` / `.aip`，按调用方指定的路径写盘，深层目录自动创建）、`save_project`、`import_project`、`import_image`（外部位图降采样量化进画布）
- 智能体：`prompt_agent`（让本程序内置的主智能体干活）、`interrupt_agent`

`import_image` 是闭环的另一半入口：外部 AI 用自己的生图模型出一张图，`path` 给本地文件（或 `image_base64` 内联），程序做面积平均降采样 + 调色板量化后写进指定层/帧——和内部生图工作流同一套量化。少了它，外部智能体只能把像素一个个手传给 `apply_ops`，一帧 64x64 要背 4096 个坐标。

传输是手写的 HTTP/1.1 + JSON-RPC 2.0，一条连接一次请求、`Connection: close`，带 CORS。`GET /` 回服务信息（人肉健康检查用），`OPTIONS` 回 204，坏 JSON 回 400 + `-32700`，未知方法回 `-32601`，未知工具回 `isError`，通知（没有 `id`）回 202。请求体上限 8MB。

先用 curl 探一下活：

```bash
curl http://127.0.0.1:7815/
```

再列一遍现成画布、开一个 64x64 的新画布、画一笔、按绝对路径导成 PNG：

```bash
curl -X POST http://127.0.0.1:7815/ \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}'

curl -X POST http://127.0.0.1:7815/ \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_canvas","arguments":{"width":64,"height":64,"name":"hero"}}}'

curl -X POST http://127.0.0.1:7815/ \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"paint_stroke","arguments":{"id":"hero","from_x":8,"from_y":32,"to_x":56,"to_y":32,"color":"#e8823a","size":2}}}'

curl -X POST http://127.0.0.1:7815/ \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"export_canvas","arguments":{"id":"hero","format":"png","path":"/tmp/AIPixel/hero.png"}}}'
```

`list_export_formats` 会把能导的格式 id 全摊出来：`gif` / `sheet` / `strip` / `frame` / `png`（`frame` 的别名）/ `aseprite` / `ase`，工程文件另有 `aip`。

导成 Aseprite 文件也是同一个工具换个 `format`：

```bash
curl -X POST http://127.0.0.1:7815/ \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"export_canvas","arguments":{"id":"hero","format":"aseprite","path":"/tmp/AIPixel/hero.aseprite"}}}'
```

回头上枚位图也是同一个工具：外部 AI 用自己的生图模型出一张 PNG，指定路径落进画布。

```bash
curl -X POST http://127.0.0.1:7815/ \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"import_image","arguments":{"id":"hero","path":"/tmp/AIPixel/hero-source.png"}}}'
```

Node 侧一次完整闭环——建画布、画一笔、存工程、导到游戏引擎的资源目录：

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
await call("save_project", { id: "hero", path: "game/assets/hero.aip" });
const out = await call("export_canvas", { id: "hero", format: "aseprite",
                                          path: "game/assets/hero.aseprite" });
console.log(out.content[0].text);
```

`prompt_agent` 是把整个主智能体交出去的那一个工具：外部 AI 描述需求，本程序的模型负责拆解、绘制、自检，回一串可读的过程记录。二者分工上，外部 AI 当导演，本程序当会画图的那只手。
实现在 [src-tauri/src/mcp_server.rs](src-tauri/src/mcp_server.rs)，与用户自配的外部 MCP 服务器（`src-tauri/src/mcp.rs`）是两套独立机制，互不影响。
