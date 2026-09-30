# example：最初那套 Pillow 工具链与样例数据

仓库根目录是桌面端（Tauri 2 + Rust 主循环 + React 前端），这一层是它之前的东西：
纯 Python + Pillow 的 `.aip` 转换脚本、早期的网页版像素编辑器，以及跑通它们所需的样例数据。
桌面端不依赖这里任何一个文件，整套搬过来是为了让最早的格式约定和转换结果还能被查证、复跑。

## 脚本

| 路径 | 说明 |
| --- | --- |
| `aip_converter.py` | `.aip` 转 PNG，扫描 `input/`，产出落到 `output/` |
| `img2aip_converter.py` | PNG 反查成 `.aip`（颜色索引 + 透明处理），扫 `refer_img/`，产出落 `refer_aip/` |
| `img2x_converter.py` | 高质量缩放：letterbox / crop / 缩小，可顺带量化 |
| `process_attack.py`、`process_knight.py` | 精灵图切帧到 32x32 的一次性小脚本 |
| `longchain.py`、`pipeline.py` | 早期空壳，没有内容，仅存档 |
| `html/index.html` | 早期的网页版像素编辑器 |
| `pyproject.toml` | 上面这些脚本的打包元数据，CI 靠它装依赖 |

## 样例数据

| 路径 | 说明 |
| --- | --- |
| `input/` | 待转换的 `.aip` |
| `output/` | `aip_converter.py` 的产出 PNG |
| `refer_img/` | 喂给 `img2aip_converter.py` 的参考位图 |
| `refer_aip/` | 反查产出的 `.aip` |
| `refer_pixel/` | 一组像素画样例 `.aip` |

这些样例跟着仓库走，是交付内容，被根目录 README 引用。

## 运行

只依赖 Pillow，Python 3.9+：

```bash
pip install -e .
python3 aip_converter.py                       # input/ -> output/
python3 img2aip_converter.py                   # refer_img/ -> refer_aip/
python3 img2x_converter.py 输入.png -s 32x32 -m letterbox -o resized
```

各脚本按自己所在目录找数据，所以整个 `example/` 目录要一起搬，单独抽走脚本会找不到样例。

## 和桌面端的关系

- `.aip` 的早期约定（明文文本、索引 0 恒为透明）从这里来，桌面端沿用并长成了 v2：图层与帧是一等结构
- 桌面端工作台的「量化」就是 `img2aip_converter.py` 那条思路的 Rust 实现，落在 `crates/pixel-core`

本目录随 AIPixel 一同以 GPL-3.0 分发，`pyproject.toml` 里的元数据同样标成 GPL-3.0。
