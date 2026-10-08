<div align=center>
<img src="icon.png" style="width:100px;" width="100"/>
<h2>AIPixel</h2>
<p>Pixel art agent workspace</p>
</div>

English | [简体中文](README.md)

### 1. Overview

- AIPixel is a pixel-art agent workspace that runs entirely on your machine: sessions on the left, conversation in the middle, canvas on the right. One conversation produces one `.aip` project.
- **BYOM (bring your own model)**: no built-in server, no login, no billing. Models come only from the Provider you fill in yourself (Anthropic / OpenAI-compatible), and addresses plus keys never leave your machine.
- **Every pixel lands in a budgeted sandbox**: structural edits go through typed operations, drawing and animation run in a Lua sandbox, and read-back uses run-length encoding. The model never hand-writes a screen of pixel values.
- The canvas is the single authoritative state: layers, frames and named palettes share one document, so an edit, an undo and an export all touch the same thing.
- The `.aip` intermediate file is plain text. Width, height, palette, layers and frames all live inside it, which makes it natural for version control and manual tweaking.
- **Publisher** Released by Mutantcat Working Group (mutantcat.org), GitHub: https://github.com/Mutantcat-Working-Group/AIPixel

Core value:

- **Works out of the box**: the desktop app draws as soon as you open it. Model settings, MCP servers and batch recipes stay on your machine and are never uploaded anywhere.
- **Built for pixel art**: image generation is narrowed to a set of budgeted tools, so the model stays on structure, composition, scripting and reference images instead of redrawing the whole image for one pixel.
- **Fits a game workflow**: RPG Maker character-sheet grids, Aseprite export, and batch quantization/export drop straight into an engine's asset folder.
- **AI friendly**: a built-in art knowledge base plus behavioural rules, and MCP in both directions, so an external agent can "create a canvas, draw, export to a path".

### 2. Features

#### Workspace

- Session list on the left: fill in a session name and canvas size when creating, then rename or drag to reorder. Each session can bind its own model and role.
- Conversation in the middle: user messages carry attachments, assistant messages stream in, tool calls expand to show their inputs, and reasoning fragments collapse.
- Canvas and document panel on the right: one row each for layers, frames and palettes, plus a view that switches to the raw `.aip` text.

#### Canvas and animation

- Brush, fill and eraser plus undo. Edits land while you draw, and the whole stroke is committed when you lift the pointer.
- Every frame in the frame strip is a thumbnail rendered the same way the export will be, so what you see is what you get. Under each thumbnail sit the frame index and its hold duration; click a frame to jump to it.
- Create, duplicate, delete and reorder frames; playback runs a lightweight local loop; onion skin lays the previous frame under the current one at 24% opacity.
- Tile backdrop: tile the canvas 1x1 / 2x2 / 3x3 so seams are obvious at a glance, with only the center cell taking the pointer.
- RPG Maker character-sheet support: pick one of the fixed grids 144x192 (MV/MZ), 96x128 (VX/Ace), 128x128 (XP) or 72x128 when creating a canvas, then lay a grey paper-doll base with one click and recolour it by part region (hair, face, clothes, trousers, shoes and so on).

#### Layers and palettes

- The layer list is ordered top-of-stack first (like Aseprite / Photoshop). Each row has a visibility toggle, layers can be renamed and reordered, and the current layer's opacity sits below the list.
- Palettes come as presets (Gray 8 / Sweetie 16 / DawnBringer 16 / PICO-8 / Game Boy / 1-bit) plus custom colours.
- A preset can be **locked**: while locked, the AI may only pick colours from that range; unlock it and the AI can extend the palette.
- Presets and custom colours can both be added and removed, and removing a colour asks you to pick a replacement from the current selection.
- Each layer keeps its own palette range. Switching presets remaps existing pixels to the nearest colour along CIELAB, so the picture stays while the colours regroup.

#### Import and export

- Encoding happens entirely in Rust, and compositing works from palette indices rather than bitmap pixels, so the export is the same thing you see on the canvas.
- Export supports GIF (infinite loop, per-frame duration taken from the document), the current frame as PNG, a horizontal strip PNG, a sprite sheet PNG, and Aseprite `.aseprite` (layer and frame semantics preserved).
- Import supports bitmap quantization into the canvas, `.aip` project import, and reference images / video frames / generated images dropped onto the canvas, with scaling and rasterization.
- Import and export both live at the top-right of the toolbar instead of taking up the right-hand menu.

#### Workflows and batch

- The workflow dock enables entries according to the current model's capabilities: agent drawing, image generation, reference-image briefing, video frame extraction, video motion briefing, tweening, prompt tuning and bitmap quantization.
- Batch workspace: a fully local folder-in, folder-out job that can quantize "reference images → `.aip`" or export "`.aip` → PNG / GIF", and reports success/failure counts plus the output directory when it finishes.
- Recipe book: save a tuned parameter set on your machine, delete or overwrite it, and export it as a single `.aipr` or the whole book for sharing and re-running.

#### Models, MCP and guards

- Model settings hold the Provider, endpoint, key and model name. Configure several and switch between them; each session can bind its own model.
- Three permission levels: Auto runs every call straight away; Ask stops at every call for approval; Chat blocks only write operations and lets reads through.
- Acts as an MCP client too: attach external MCP servers over stdio or HTTP, and their tools join each turn under a namespace.
- Acts as an MCP server as well, letting external AI drive "create → draw → export". See section 5.
- Guards: tool-step count, continuation count and back-fill bytes per turn are configurable, retries back off and deadlocks are detected, and a streaming turn can be interrupted at any time.

#### Built-in art knowledge base

- Three blocks are injected into the prompt on demand: pixel-art knowledge (animation and tiles, colour and outlines, light and form, effects and materials, animal and object description, asset sets and workflow), a bilingual art glossary, and a Chinese/English colour-name table.
- Retrieval is plain-text matching: knowledge entries are weighted by the trigger words present in your message, and unrelated entries are left out rather than padding the prompt.
- Three behavioural rules follow the brush: they are injected when drawing or editing and skipped for pure questions.
- Retrieval hits are shown on the plan node in the conversation, so you can see which entries a turn used. Full provenance lives in [docs/KNOWLEDGE_SOURCES.en.md](docs/KNOWLEDGE_SOURCES.en.md).

### 3. Install and Download

Grab the latest installer from [Releases](https://github.com/Mutantcat-Working-Group/AIPixel/releases/latest). Version numbers look like `1.0.20261019` (the patch component is the build date):

| Platform | Installer |
| --- | --- |
| Windows x64 | `AIPixel_<version>_x64-setup.exe` |
| Windows arm64 | `AIPixel_<version>_arm64-setup.exe` |
| macOS (Intel) | `AIPixel_<version>_x64.dmg` |
| macOS (Apple Silicon) | `AIPixel_<version>_aarch64.dmg` |
| macOS (universal) | `AIPixel_<version>_universal.dmg` |
| Linux x86_64 | `AIPixel_<version>_amd64.AppImage` |
| Linux aarch64 | `AIPixel_<version>_aarch64.AppImage` |

- Windows ships an NSIS installer that requests administrator rights and uses a Simplified Chinese install UI.
- macOS DMGs are ad-hoc signed and include an Applications drag-and-drop shortcut.
- Linux builds are AppImages: mark them executable and double-click.
- Every Release carries `checksums.txt`, `checksums-md5.txt` and `checksums-sha1.txt` so downloads can be verified.

### 4. Quick Start

1. Install and launch the desktop app. When creating a session, fill in a name and canvas size (leaving the name blank gives you s1, s2, ...).
2. Open **Settings → Model**, enter your Provider endpoint, API key and model name, then hit "Fetch" to pull the model list from the endpoint.
3. Describe the pixel art you want in the chat box, for example "draw a 32x32 knight idle pose, four greens". The model lands it on the canvas under budgeted constraints.
4. Inspect the result on the right: flip frames to check animation, turn on onion skin to register movement, and switch presets or pick a single colour in the palette range.
5. If you need animation, create a frame and let the model tween it, or tune it yourself with the frame strip and playback.
6. When you are happy, use the top-right export menu for GIF / PNG / sprite sheet / Aseprite, or save the `.aip` to disk.
7. If you have a pile of reference images to turn into `.aip`, or a pile of `.aip` to render as PNG / GIF, switch to the "Batch" tab on the right, pick a folder, scan, and start.

### 5. MCP and External Integration

AIPixel is both an MCP client (attaching other people's MCP servers) and an MCP server that external AI can drive. The server exposes the full canvas loop - **create → draw → export to a path** - so one external agent plus a game engine is enough for the AI to draw assets, write them to disk and have the engine load them directly.

The switch lives in **Settings → MCP** and is off by default. It binds to the loopback address only (`127.0.0.1`) and never opens to the network. The default port is `7815`; set it to `0` to let the kernel pick a free one.

It exposes 18 tools in four groups:

- Canvas sessions: `list_sessions`, `create_canvas`, `drop_canvas`, `rename_canvas`, `get_canvas`, `canvas_preview`
- Drawing: `paint_stroke`, `fill_region`, `apply_ops`, `resize_canvas`, `lay_paperdoll_base`
- Files: `list_export_formats`, `export_canvas`, `save_project`, `import_project`, `import_image`
- Agent: `prompt_agent`, `interrupt_agent`

Transport is a hand-written HTTP/1.1 + JSON-RPC 2.0 endpoint, one request per connection. Probe it with curl:

```bash
curl http://127.0.0.1:7815/
```

Create a 64x64 canvas, paint a stroke, and export Aseprite:

```bash
curl -X POST http://127.0.0.1:7815/ -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"create_canvas","arguments":{"width":64,"height":64,"name":"hero"}}}'

curl -X POST http://127.0.0.1:7815/ -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"paint_stroke","arguments":{"id":"hero","from_x":8,"from_y":32,"to_x":56,"to_y":32,"color":"#e8823a","size":2}}}'

curl -X POST http://127.0.0.1:7815/ -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"export_canvas","arguments":{"id":"hero","format":"aseprite","path":"/tmp/AIPixel/hero.aseprite"}}}'
```

A full loop from Node - create, paint, export into an engine's asset folder:

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
await call("export_canvas", { id: "hero", format: "png", path: "game/assets/hero.png" });
```

`prompt_agent` is the tool that hands over the whole main agent: the external AI describes the need, this program's model takes it apart, draws and self-checks. Implemented in [src-tauri/src/mcp_server.rs](src-tauri/src/mcp_server.rs), a mechanism entirely separate from the user-configured external MCP servers (`src-tauri/src/mcp.rs`).

### 6. Data Storage and Privacy

- Model settings (Provider, endpoint, API key) are written to `models.json` in the system application-config directory. Keys stay on your machine and are never handed back to the WebView.
- The same directory holds `mcp.json` (MCP servers) and `recipes.json` (the batch recipe book).
- `.aip` / `.aipr` are plain text, so they can be diffed, hand-edited or checked into Git.
- The app ships no built-in server and makes no network request other than to the model endpoints and MCP servers you configure; video frame extraction, tweening and batch quantization all run locally.

### 7. Local Development and Build

Requirements: Node.js 20+, Rust stable and pnpm 12. esbuild's postinstall must be allowed; the allowlist lives in [pnpm-workspace.yaml](pnpm-workspace.yaml).

```bash
pnpm install
pnpm dev            # vite only (port 1420), for UI work
pnpm test           # vitest
pnpm tauri dev      # full desktop debugging
cargo test --workspace
```

`pnpm tauri dev` starts its own vite. If port 1420 is already held by `pnpm dev`, stop that first, otherwise the frontend hot-reloads onto new code while Rust still runs the previous binary and you get "Command xxx not found".

Pushing a `v*` tag (for example `v1.0.20261019`) triggers `.github/workflows/release.yml`, which builds desktop installers on six runners in parallel (macOS dmg, Windows NSIS, Linux AppImage / deb), computes checksums and publishes a Release. `.github/workflows/ci.yml` runs Rust tests, formatting and Clippy plus frontend lint, unit tests and build on every push.

Project layout:

```text
crates/pixel-core    # pixel document model, typed ops, RLE codec, Lua sandbox, PNG / GIF / sheet export
crates/agent-core    # Rust agent main loop: prompt, provider streaming, tool_use, execution, continuations
src-tauri            # Tauri shell: command layer, state, event broadcast, MCP client and server
src                  # thin React shell: rendering and input only
example              # legacy Python toolchain and sample data, not used by the desktop app
```

### 8. Development Progress

- [X] Desktop app (Tauri 2 + React + Ant Design)
- [X] Rust agent main loop with streaming output
- [X] Canvas, layers, frames and animation playback
- [X] Palette presets, colour lock and per-layer palette ranges
- [X] Undo / redo and per-step operation cache
- [X] Multi-format export (PNG / GIF / sprite sheet / Aseprite)
- [X] Bitmap quantization import and reference image / video workflows
- [X] Built-in art knowledge base and intent routing
- [X] MCP both as client and as server
- [X] Batch workspace and recipe book
- [X] Six-platform CI packaging and release

[GPL-3.0](LICENSE)

---

## Acknowledgements

Developed and published by Mutantcat Working Group ([mutantcat.org](https://www.mutantcat.org/)). The built-in art knowledge base draws on public tutorials and assets from MakeBead, pixel-asset-master-skills, saint11, Pedro Medeiros, Derek Yu, Cure / Pixel Joint, Slynyrd, Lospec and Concept Art Empire, and owes ideas to [IPaperDoll](https://github.com/Mutantcat-Working-Group/IPaperDoll), Aseprite and PixTXT. Entry-by-entry provenance is in [docs/KNOWLEDGE_SOURCES.en.md](docs/KNOWLEDGE_SOURCES.en.md).
