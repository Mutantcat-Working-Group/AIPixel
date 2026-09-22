<div align="center">
<img src="https://raw.githubusercontent.com/Mutantcat-Working-Group/AIPixel/main/output/storm_sword.png" style="width:100px;" width="100"/>
<h2>AIPixel</h2>
</div>

> AIPixel —— 围绕 `.aip` 像素画格式的一整套工具箱。
> AIP 转 PNG、PNG 转 AIP、批量缩放、精灵图切帧，外加一个浏览器里的像素画编辑器。

### 一、产品概述

- AIPixel 是一个 AIP 像素画工具集：`.aip` 文本格式与 PNG 位图互转、图片缩放、精灵图切帧，以及一个网页版像素编辑器
- AIP 格式把像素画存成「宽高 + 颜色矩阵」的明文文本，天然适合版本管理和人工微调
- 工具全部基于 Python + Pillow 编写，无重型依赖，脚本可以直接搬进自己的流水线
- 仓库自带 `refer_aip/`、`refer_img/`、`refer_pixel/`、`output/` 四组参考与产出样例，拿来就能验证效果
- 面向做 RPG / 独立游戏的美术与程序：武器、角色、怪物素材常在这里过一遍

核心价值：像素画素材在「源文件」和「引擎可用」之间总要来回倒。AIPixel 把这条路铺平：文本可读可改，转 PNG 一键完成，反向也能从成品图还原出 AIP。

### 二、界面与产物

- `html/index.html` 是一个网页版 AIP 像素画编辑器，基于 layui，浏览器打开即用
- 命令行工具无界面，输入输出都是文件路径，适合脚本化批处理
- `output/` 目录存放转换产物样例（风暴大剑、血族之刃、兔子、行走女孩、骑士等），一眼看出转换质量

### 三、功能说明

#### AIP -> PNG

- `aip_converter.py`：解析 `.aip` 文件，提取 `width` / `height`，再按 `image:` 与 `color:` 两段还原颜色矩阵并绘制成 PNG
- 缺 `width` / `height` 或 `image` / `color` 段会直接抛错并指出文件名，不会静默产出错图

#### PNG -> AIP

- `img2aip_converter.py`：反向转换器，分析图像中出现过的所有颜色并按首次出现顺序分配索引
- 索引 0 固定保留给透明像素，其余颜色从 1 开始编号
- 处理前统一转 RGBA，透明与半透明像素都被正确归类

#### 图片缩放

- `img2x_converter.py`：缩放到指定尺寸，可选是否保持宽高比
- 保持宽高比时用 letterbox 填充，重采样走 Lanczos 高质量算法
- RGBA 与 RGB 均正确处理，填充背景色可自定义

#### 精灵图切帧

- `process_attack.py`：把 64x64 单帧攻击图缩到 32x32，`NEAREST` 重采样保住硬边像素风
- `process_knight.py`：处理 1536x1024 的骑士精灵图，按 2 行 4 列切出 8 个 384x512 帧，每帧再缩到 32x32
- 处理后直接落盘，控制台打印原始尺寸与输出路径

#### 工程特性

- 仅依赖 Pillow，Python 3.9+ 即可运行
- `pyproject.toml` 按 setuptools 打包，五个模块全部声明为 py-module，可 `pip install`
- CI 在发布前执行检查

### 四、安装与下载

从 [Releases](https://github.com/Mutantcat-Working-Group/AIPixel/releases) 下载最新产物：

| 资产 | 说明 |
| --- | --- |
| `aipixel-1.0.20260920-py3-none-any.whl` | Python wheel 包，`pip install` 后用 |
| `aipixel-1.0.20260920-source.tar.gz` | 源码包 |
| `aipixel-1.0.20260920.tar.gz` | 含参考与产出样例的完整归档 |
| `checksums.txt` / `checksums-md5.txt` / `checksums-sha1.txt` | 三套校验值 |

```bash
pip install aipixel-1.0.20260920-py3-none-any.whl
```

也可以直接用源码，无需安装：

```bash
git clone https://github.com/Mutantcat-Working-Group/AIPixel.git
cd AIPixel
python3 aip_converter.py input/blood_drinker.aip            # AIP -> PNG
python3 img2aip_converter.py refer_img/banana_shadow.png    # PNG -> AIP
```

版本号格式为 `主版本.次版本.发布日期`。推送 `v` 前缀标签（如 `v1.0.20260920`）后，GitHub Actions 会自动构建 wheel 与源码包并发布 Release，无需手动上传。

### 五、快速上手

1. 用 `aip_converter.py` 把 `input/` 里的 `.aip` 转成 PNG，确认格式解析正常。
2. 用 `img2aip_converter.py` 把一张成品 PNG 反转回 `.aip`，与 `refer_aip/` 下的样例对比。
3. 精灵图批量切帧走 `process_knight.py` / `process_attack.py`，按需改目标尺寸。
4. 想在浏览器里手动画，直接打开 `html/index.html`。

### 六、工程结构

```text
AIPixel/
├── aip_converter.py        # AIP -> PNG
├── img2aip_converter.py    # PNG -> AIP（颜色索引 + 透明处理）
├── img2x_converter.py      # 高质量缩放（letterbox / Lanczos）
├── process_attack.py       # 64x64 -> 32x32 单帧缩放
├── process_knight.py       # 2x4 精灵图切帧并逐帧缩放
├── pipeline.py             # 流水线串联
├── longchain.py            # 长流程处理
├── html/index.html         # 网页版像素画编辑器
├── input/                  # 待转换的 .aip 输入样例
├── refer_aip/              # AIP 参考样例
├── refer_img/              # PNG 参考样例
├── refer_pixel/                # 参考像素样例
├── output/                 # 转换产物样例
├── pyproject.toml          # 包定义（aipixel 1.0.20260920）
└── .github/workflows/      # ci.yml / release.yml
```

### 备注

- 切帧脚本的目标尺寸写死在函数内，改尺寸直接改 `target_size` 即可。
- AIP 格式是明文文本，改像素本质是改文本，适合脚本化批量调色。
- 输出目录里带 `.DS_Store` 等系统文件，不影响工具运行，可自行清理。
