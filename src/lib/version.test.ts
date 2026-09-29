import agentToml from "../../crates/agent-core/Cargo.toml?raw";
import pixelToml from "../../crates/pixel-core/Cargo.toml?raw";
import shellToml from "../../src-tauri/Cargo.toml?raw";
import workspaceToml from "../../Cargo.toml?raw";
import pkg from "../../package.json";
import tauriConf from "../../src-tauri/tauri.conf.json";
import { describe, expect, it } from "vitest";

/**
 * 版本号有三处硬编码：package.json（构建期烘成 __APP_VERSION__，「关于」页念它）、
 * src-tauri/tauri.conf.json（安装包元数据、DMG 文件名）、Cargo.toml（Rust 侧）。
 * 只改一处的事情太容易发生：改了 package.json 忘了 tauri.conf.json，
 * 「关于」页会说 1.0.20261101，而装出来的 dmg 还叫 1.0.20261031。
 * 这种漂移没有任何运行时会报错，只有用户对着两个版本号发懵，所以在这儿拦住。
 *
 * 仓库里没装 @types/node（tsconfig 的注释写明了原因），所以文件内容全部走构建期的
 * ?raw / JSON 静态引入，不碰 fs——这样 tsc -b 和 pnpm build 都不报错。
 */
describe("版本号只有一个出处", () => {
  it("package.json 与 tauri.conf.json 必须一致", () => {
    expect(pkg.version).toBe(tauriConf.version);
  });

  it("Rust workspace 与外壳跟着一起走", () => {
    // workspace 那一份是唯一的版本源：外壳和两个 crate 都是 version.workspace = true。
    const match = workspaceToml.match(/^version\s*=\s*"([^"]+)"/m);
    expect(match, "workspace Cargo.toml 里找不到顶层 version").not.toBeNull();
    expect(match?.[1]).toBe(pkg.version);
    const shells: Array<[string, string]> = [
      ["src-tauri/Cargo.toml", shellToml],
      ["crates/agent-core/Cargo.toml", agentToml],
      ["crates/pixel-core/Cargo.toml", pixelToml],
    ];
    for (const [name, text] of shells) {
      expect(text, `${name} 该继承 workspace 的版本号`).toMatch(
        /^version\.workspace\s*=\s*true$/m,
      );
    }
  });

  it("版本号长得像 1.0.YYYYMMDD，日期还得是真的", () => {
    expect(pkg.version).toMatch(/^\d+\.\d+\.\d{8}$/);
    const day = pkg.version.split(".")[2];
    // 不存在的日期（比如 20260231）能糊弄过正则，但糊弄不过 Date。
    const parsed = new Date(`${day.slice(0, 4)}-${day.slice(4, 6)}-${day.slice(6, 8)}T00:00:00Z`);
    expect(Number.isNaN(parsed.getTime()), `${day} 不是真实日期`).toBe(false);
  });
});
