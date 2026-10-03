<div align="center">
<img src="icon.png" style="width:100px;" width="100"/>
<h2>AIPixel</h2>
<p>Pixel art agent workspace</p>
</div>

中文 | **English**

### 1. Overview

- AIPixel is a pixel-art agent workspace that runs entirely on your machine: sessions on the left, conversation in the middle, canvas on the right. One conversation produces one `.aip` file.
- **BYOM (bring your own model)**: no built-in server, no login, no billing. Models come only from the Provider you fill in yourself (Anthropic / OpenAI-compatible), and addresses plus keys never leave your machine.
- **Every pixel lands in a budgeted sandbox**: the model is never allowed to hand-write pixel matrices. Structural edits go through typed operations, drawing and animation run in a Lua sandbox, and read-back uses run-length encoding.
- The intermediate file `.aip` is plain text. Width, height, palette, layers and frames all live in the file itself, which makes it natural for version control and manual tweaking.
- **Publisher** Released by Mutantcat Working Group (mutantcat.org), GitHub: https://github.com/Mutantcat-Working-Group

Core value:

- The worst thing a model can do with pixel art is "hand-write" a screen of color indices and redraw the whole image for a single pixel change. AIPixel narrows image generation to seven budgeted tools, leaving the model freedom where it belongs: structure, composition, scripting and reference images.
- The canvas is not a screenshot for humans to look at. It is the single authoritative state: layers, frames and named palettes are first-class, so an edit, an undo and an export all touch the same thing.
- The desktop app works out of the box. Model configuration, MCP servers and batch recipes stay on your machine and are never uploaded anywhere.

### 2. Features

#### Three-pane workspace

- Session list on the left: pick a canvas size when creating a session, rename and drag to reorder. Each session can bind its own model and role.
- Conversation in the middle: user messages carry attachments, assistant messages stream in, tool calls expand to show their inputs, and reasoning fragments collapse.
- Canvas and document panel on the right: one row each for layers, frames and named palettes, plus a view that switches to the raw `.aip` text.

#### Canvas and animation

- Brush, fill and eraser plus undo. The stroke previews incrementally on the canvas and is only committed when you lift the pointer.
- Every frame in the frame strip is a thumbnail rendered the same way the export will be, so what you see is what you get. Under each thumbnail sit the frame index and its hold duration; click a frame to jump to it.
- Create, duplicate, delete and reorder frames. Playback runs a lightweight local loop instead of hitting the backend per frame. Onion skin lays the previous frame under the current one at 24% opacity so frame-by-frame registration has a reference.
- A tile-backdrop mode tiles the canvas 1x1 / 2x2 / 3x3 so seams are obvious at a glance; only the center cell takes the pointer, the rest are echoes of the same drawing.

#### Layers

- Layers are listed top-down like Aseprite / Photoshop, each row with a visibility toggle.
- Row-end arrows move the selected layer one step along the draw order; opacity for the active layer sits below the list.
- Index 0 is always transparent. The leading transparent swatch in the palette row is where erasing lands.

#### Named palettes

- Six built-in presets: Gray 8 / Sweetie 16 / DawnBringer 16 / PICO-8 / Game Boy / 1-bit, plus one "any color" slot.
- Switching presets is not cosmetic: existing pixels are remapped into the new range by nearest CIELAB color, so the picture stays and the colors fall in line.
- Dragging a color previews live and commits on release; the tool button copies the hex value.

#### Export

- Encoding happens entirely on the Rust side, and the composite source is palette indices rather than bitmap pixels, so exports match the canvas exactly.
- Supported outputs: GIF (infinite loop, frame delay taken from the document's own durations), current-frame PNG, horizontal full-strip PNG, all frames laid out in one image, spritesheet PNG, and Aseprite `.ase` (layer and frame semantics preserved as-is).

#### Workflow dock

- Seven workflows, each judged for readiness against the currently bound model. A workflow the model cannot run is still listed, just disabled with a note about what is missing.
- Agent drawing: conversation as drawing; every edit runs one sandboxed Lua script.
- Image generation: the model renders a bitmap which is then quantized onto the canvas grid. Reference images are supported.
- Vision brief: a vision model reads a reference image into a structured brief, then draws from it.
- Video frames: extract frames and quantize each one purely locally. Zero model calls, zero tokens.
- Video motion brief: a video-reading model turns a clip into an editable motion brief. The product is text, not frames.
- Frame tween: interpolate between two existing frames on your machine.
- Prompt refine: rewrite one plain sentence into a structured image-generation prompt.
- One extra entry parked at the end of the dock: quantization, which drops a bitmap onto the grid locally.

#### Batch workspace

- One folder in, one folder out, purely local, and complementary to sessions: sessions are one or two images with a model watching, batch is deterministic repeated work that spends zero tokens.
- Two directions: quantize batches of reference images into `.aip`, or render batches of `.aip` into PNG / GIF.
- A scan runs first and reports the candidate count. Progress flows over a dedicated event channel row by row; a single failing file only records its reason without interrupting the run, and the receipt carries success / skip / fail counts plus the output directory.
- The same input with the same recipe yields the same output, which makes reruns and version control straightforward.

#### Recipe book

- Save a tuned parameter set under a name and pull it back next time; recipes can be overwritten or deleted.
- Export a single recipe or the whole book as `.aipr`: it is just JSON you can read in any editor, so you can share a recipe you have never stored.
- On import, same-named recipes get a sequence suffix instead of being overwritten, and broken entries are skipped with a line-by-line account in the receipt.

#### Models and MCP

- Fill in Provider, endpoint, key and model name in model settings. Multiple models can be configured and switched, and each session can bind its own.
- Three permission levels: Auto executes every call directly; Ask stops every call for your approval; Chat only intercepts writes and lets read-only calls through.
- "Stop asking me for this turn" only downgrades the current turn to Auto. It is not written back into the session configuration.
- The main loop is not limited to built-in tools: the plug icon in the top bar opens the MCP tool servers panel, where you can attach your own MCP servers over stdio (spawning a child process) or HTTP.
- Tools from an MCP server enter each round under a namespace alongside the built-in pixel tools, and they pass through the same approval gate.
- The reverse direction counts too: AIPixel is itself an MCP server, so an external AI can connect in and "create a canvas, paint, export to a path". See section 10 for details.

#### Budget guards

- Tool steps per turn, continuation rounds and result bytes are all configurable; repeated identical failures back off automatically.
- Interrupt at any time during streaming. Interruption settles immediately and never wedges the rest of the turn.

#### Built-in art knowledge base

- Three blocks go into the prompt on demand: **92 entries of pixel-art knowledge** (animation and tiles, colour and outlines, light and form, animal and object descriptions, set consistency and process), **73 art terms in both Chinese and English with aliases** ("value ramp" for ramp, "outline" for outline), and a **common colour names table in Chinese and English** grouped by hue, so a model can name a colour instead of guessing a hex value.
- Retrieval is plain-text matching: whichever trigger words appear in the user's sentence, those entries are weighted by word length and sent with the prompt. Entries nobody mentioned are simply left out - a prompt stuffed with concepts dilutes the part that actually matters.
- Three discipline notes ride along with every pass that touches the canvas - new artwork and edits alike, never a pure chat turn. The guardrails follow the brush, not the trigger words: nobody ever types "never let procedural work read as a fake pattern", so waiting for a keyword hit would mean never sending it. They also carry their own character budget instead of taking a slot from the four retrieved entries, so "draw a 5-frame run" keeps its gait phase table.
- The entries a turn pulled are shown on the plan node in the conversation, so users can see which ones were referenced rather than wonder.

### 3. Install and Download

Desktop builds are on [Releases](https://github.com/Mutantcat-Working-Group/AIPixel/releases). Version numbers look like `1.0.20261009` (minor version plus build date):

| Platform | Installer |
| --- | --- |
| Windows x86_64 | `AIPixel_<version>_x64-setup.exe` |
| Windows arm64 | `AIPixel_<version>_arm64-setup.exe` |
| macOS (Intel) | `AIPixel_<version>_x64.dmg` |
| macOS (Apple Silicon) | `AIPixel_<version>_aarch64.dmg` |
| macOS (universal) | `AIPixel_<version>_universal.dmg` |
| Linux x86_64 | `AIPixel_<version>_amd64.AppImage` |
| Linux aarch64 | `AIPixel_<version>_aarch64.AppImage` |

Windows ships as an NSIS installer (perMachine, Simplified Chinese installer UI); the macOS DMG is ad-hoc signed with an Applications drag-and-drop shortcut; Linux ships as an AppImage that runs after granting execute permission. Each Release also includes `checksums.txt`, `checksums-md5.txt` and `checksums-sha1.txt` so you can verify what you downloaded.

### 4. Quick Start

1. Install and launch the desktop app, then pick a canvas size when you create a session.
2. Open "model settings" in the top bar, fill in your own Provider endpoint, API key and model name, and test the connection.
3. Describe the pixel art you want in the conversation box, for example "draw a 32x32 knight in idle pose with a four-step green ramp". The model draws on the canvas through budgeted tools.
4. Inspect the result on the right: flip through frames, turn on onion skin for registration, and swap presets or pick single colors in the named palette row.
5. When you need animation, create frames and let the model tween them, or adjust them yourself with the frame strip and playback.
6. When you are happy, export from the top-right menu: GIF / PNG / spritesheet / Aseprite, or save the `.aip` to disk.
7. When you have a pile of reference images to convert into `.aip`, or a pile of `.aip` to render as PNG / GIF, switch to the Batch column on the right: choose folders, scan, start.

### 5. Data Storage and Privacy

- Model configuration (Provider, endpoint, API key) is written to `models.json` in the system application config directory. Keys stay on your machine and are never passed back to the WebView, and can be edited or cleared at any time.
- The same directory holds `mcp.json` (MCP servers; `env` / `headers` stay local, the UI returns key names but never values, and an empty value on edit means "keep the existing one") and `recipes.json` (the batch recipe book).
- `.aip` / `.aipr` are plain text, so diffing, manual tweaking and committing them to Git all work.
- The app ships no server and makes no network requests other than the model endpoint and MCP servers you configure. Video frame extraction and frame tweening happen locally.

### 6. Local Development and Build

Requirements: Node.js 20+, Rust stable, and pnpm 12. Use pnpm as the package manager. esbuild's postinstall needs to be allowed; the allowlist lives in [pnpm-workspace.yaml](pnpm-workspace.yaml) at the repository root. Without it, a fresh clone fails `pnpm install` with `ERR_PNPM_IGNORED_BUILDS`.

```bash
# Frontend
pnpm install
pnpm dev            # vite only (port 1420), for UI work
pnpm build          # tsc -b && vite build
pnpm test           # vitest

# Desktop
pnpm tauri dev      # full desktop debugging

# Rust
cargo test --workspace
```

`pnpm tauri dev` starts its own vite. If port 1420 is already taken by `pnpm dev`, shut that one down first. Otherwise the frontend hot-reloads to new code while Rust still runs the previous binary, and the UI starts reporting version-skew errors such as "Command xxx not found".

#### Packaging (`pnpm tauri build`)

`bundle.targets` in `tauri.conf.json` is already set to dmg / app / nsis / appimage / deb: macOS produces `.app` + `.dmg`, Windows produces an NSIS installer, and Linux produces AppImage / deb.

- macOS: it packages without a developer certificate. `bundle.macOS.signingIdentity` is set to `"-"`, so signing goes through ad-hoc (`codesign -s -`). A self-signed `.app` opens straight from Finder and is not treated as an anonymous unsigned artifact. The machine building the DMG must accept the Xcode command line tools license; without it the build dies on the `SetFile` line.
- Windows: the NSIS template is [src-tauri/nsis/installer.nsi](src-tauri/nsis/installer.nsi), configured with `installMode: "perMachine"` (UAC prompt, installs to `Program Files\AIPixel`), `languages: ["SimpChinese"]` (the whole installer in Simplified Chinese, including the WebView2 missing prompt), and the product name plus version in the lower-left corner. The template baseline is Tauri's official default `installer.nsi` (tag `tauri-v2.12.0`) with only the `BrandingText` line changed; re-align it with the official template after each Tauri upgrade.

### 7. CI and Automated Releases

Pushing a `v*` tag (for example `v1.0.20261009`) triggers `.github/workflows/release.yml`:

1. Six parallel desktop builds: macOS x86_64 / aarch64 (dmg), Windows x86_64 / arm64 (NSIS), Linux x86_64 / aarch64 (AppImage / deb).
2. The Python packages and source tarball for the legacy toolchain under `example/` are built in parallel.
3. Once all assets are present, `checksums.txt`, `checksums-md5.txt` and `checksums-sha1.txt` are computed uniformly and published together with every artifact to the GitHub Release.

`.github/workflows/ci.yml` runs the Rust workspace tests, formatting and Clippy on every push, plus frontend lint, unit tests and build.

### 8. Project Structure

```text
.
├── icon.png                     # App and README icon
├── package.json                 # Single source of the version, baked in at build time
├── pnpm-workspace.yaml          # pnpm 12 build-script allowlist (esbuild postinstall)
├── Cargo.toml                   # Rust workspace manifest
├── crates
│   ├── pixel-core               # Pixel document model, typed operations, RLE encoding,
│   │                            # .aip v2, Lua sandbox, PNG / GIF / spritesheet export
│   └── agent-core               # The agent's Rust main loop: prompt assembly, provider
│                                # streaming, tool_use extraction, tool execution,
│                                # result backfill and continuation rounds
├── src-tauri                    # Tauri shell: state hosting, command registration, event
│   ├── src                      # broadcast
│   │   ├── commands.rs          # Session / model / document / export command layer
│   │   ├── editor.rs            # Workspace editor commands (brush / fill / structural ops)
│   │   ├── workflow.rs          # Command layer for the seven workflows
│   │   ├── mcp.rs               # MCP tool server configuration and commands
│   │   ├── batch.rs             # Batch workspace and recipe book persistence
│   │   └── state.rs             # Reads and writes models.json / mcp.json / recipes.json
│   ├── nsis/installer.nsi       # Custom Windows NSIS template
│   ├── capabilities/            # Tauri permission manifest
│   └── tauri.conf.json          # App and bundle configuration
├── src                          # Thin React shell: rendering and input only
│   ├── ui                       # Session / conversation / canvas / workflow dock /
│   │                            # batch / MCP panels
│   ├── lib                      # The single exit for invoke and events, plus per-domain
│   │                            # pure functions (unit tested)
│   ├── assets
│   ├── App.tsx  main.tsx  styles.css
├── example                      # Legacy Pillow toolchain and sample data; the desktop app
│                                # does not depend on it
├── .github/workflows
│   ├── ci.yml                  # Rust tests / fmt / Clippy + frontend lint / test / build
│   └── release.yml             # Six-platform packaging + checksums + Release
└── vite.config.ts               # vite / vitest config; version baked in from package.json
```

### 9. License and Acknowledgements

- Copyright belongs to **Mutantcat Working Group (Mutantcat Working Group · mutantcat.org)**, Copyright (C) 2026.
- This project is released under the **GNU General Public License v3.0 (GPL-3.0)**. Full terms are in [LICENSE](LICENSE) at the repository root.
- Every source file starts with two license lines: `Copyright (C) 2026 Mutantcat Working Group` and `SPDX-License-Identifier: GPL-3.0-only`, so a single file is enough to confirm its authorship.
- Use, modification and redistribution are all free, but derivative works must remain open source under GPL-3.0 and retain the original copyright and license notice.
- When distributing binaries or installers you must also provide the corresponding complete source; published artifacts in this repository always correspond one-to-one with the source.
- The examples, bundled scripts and this README under `example/` are covered by the same license.
- The `.aip` / `.aipr` data formats, the naming of built-in tools and the structure of the main loop are this repository's own choices and contain no code bound by third-party licenses.

This project is developed and published by Mutantcat Working Group (mutantcat.org), official site [mutantcat.org](https://www.mutantcat.org/). Issues are welcome in the repository.

#### Thanks and Knowledge Sources

The built-in art knowledge base was not written from impression. These public tutorials and reference sets gave it its spine:

- [MakeBead Pixel Art Tutorial](https://makebead.com/zh-Hans/how-to-make-pixel-art/): a Chinese-language pixel art tutorial. Hue shifting, physical media such as beads and cross-stitch, the numbered-grid spreadsheet method, and a graded practice list for beginners all come from it. It is a rewrite based on Saultoons' "The Ultimate Pixel Art Tutorial".
- [MakeBead Pixel Art Ideas and Style List](https://makebead.com/zh-Hans/pixel-art-ideas/): a style and subject list with ready-made hex values, which the built-in style presets mirror.
- [MakeBead Spreadsheet Pixel Art](https://makebead.com/zh-Hans/spreadsheet-pixel-art/): the full method of filling a pixel drawing as a numbered table, matching the spreadsheet entry in the knowledge base.
- [pixel-asset-master-skills](https://github.com/424431185/pixel-asset-master-skills): a pixel-art execution skill. The "get it right before you get it good" discipline - lock the spec, self-check before delivery, re-read the spec before every asset - comes from it.
- Lospec: the reference for how named palettes are catalogued, how many colours they carry, and what they suit (DB32, Endesga, Resurrect and the rest).
- Aseprite and PixTXT: reference points for `.aseprite` layer and frame semantics, and for the "index grid plus a palette" intermediate-file idea.

### 10. MCP Server Mode: Letting External AI Drive AIPixel

Besides "AIPixel connecting to someone else's MCP server", AIPixel also ships its own MCP server, exposing the full canvas loop to the outside: **create -> paint -> export to a path you choose**. With an external agent (Claude Code, Cursor, your own agent) plus a game engine, AI can draw assets, write them to disk and let the engine load them, with nobody watching.

The switch lives in **Settings -> Models & Provider -> MCP tool server**, off by default. The reason: it writes files at whatever path the caller gives, which means handing off disk write capability, and that deserves an explicit human nod. It listens on the local loopback only and answers `127.0.0.1` exclusively, never a public interface. The port defaults to `7815`; `0` asks the kernel to pick a free one.

There are 17 tools in four groups:

- Canvas sessions: `list_sessions`, `create_canvas`, `drop_canvas`, `rename_canvas`, `get_canvas`, `canvas_preview`
- Drawing: `paint_stroke`, `fill_region`, `apply_ops` (batched point / line / rect / ellipse / fill operators, all or nothing with a rollback), `resize_canvas`
- Files: `list_export_formats`, `export_canvas` (PNG / GIF / sprite sheet / frame strip / `.aseprite` / `.aip`, written to the caller's path, nested directories created automatically), `save_project`, `import_project`, `import_image` (external bitmap downsampled and quantized onto the canvas)
- Agent: `prompt_agent` (let the built-in main agent do the work), `interrupt_agent`

Transport is a hand-written HTTP/1.1 + JSON-RPC 2.0 subset: one request per connection, `Connection: close`, CORS enabled. `GET /` returns service info (for a human health check), `OPTIONS` returns 204, malformed JSON returns 400 + `-32700`, an unknown method returns `-32601`, an unknown tool returns `isError`, a notification (no `id`) returns 202. Request bodies are capped at 8MB.

Poke it with curl:

```bash
curl http://127.0.0.1:7815/
```

Then list canvases, open a 64x64 one, paint a stroke and export it to an absolute path as PNG:

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

`list_export_formats` lays out every export id: `gif` / `sheet` / `strip` / `frame` / `png` (an alias of `frame`) / `aseprite` / `ase`, plus `aip` for project files.

An Aseprite file is the same tool with a different `format`:

```bash
curl -X POST http://127.0.0.1:7815/ \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"export_canvas","arguments":{"id":"hero","format":"aseprite","path":"/tmp/AIPixel/hero.aseprite"}}}'
```

An external bitmap can come back in too: the AI produces a PNG with its own image model, points `path` at it and the file lands on the canvas.

```bash
curl -X POST http://127.0.0.1:7815/ \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"import_image","arguments":{"id":"hero","path":"/tmp/AIPixel/hero-source.png"}}}'
```

A full loop from Node: create a canvas, paint a stroke, save the project, export into a game engine resource directory:

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
  // Server-side failures come back as HTTP 200 + isError, so read content, not just error.
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

`prompt_agent` is the tool that hands over the whole main agent: the external AI describes the need, this program's model takes it apart, draws and self-checks, then returns a readable process log. Division of labour: the external AI directs, AIPixel is the hand that can actually draw.

Implemented in [src-tauri/src/mcp_server.rs](src-tauri/src/mcp_server.rs), a mechanism entirely separate from the user-configured external MCP servers (`src-tauri/src/mcp.rs`); the two do not interfere with each other.
