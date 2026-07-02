import { copyFileSync, existsSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { basename, join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const releaseDir = join(root, "release");
const releaseExe = join(root, "src-tauri", "target", "release", "codex-info.exe");
const nsisDir = join(root, "src-tauri", "target", "release", "bundle", "nsis");

mkdirSync(releaseDir, { recursive: true });

if (!existsSync(releaseExe)) {
  throw new Error(`缺少 release exe: ${releaseExe}`);
}

copyFileSync(releaseExe, join(releaseDir, "CodexInfo.exe"));

if (!existsSync(nsisDir)) {
  throw new Error(`缺少 NSIS 目录: ${nsisDir}`);
}

const installers = readdirSync(nsisDir)
  .filter((name) => name.endsWith(".exe"))
  .map((name) => join(nsisDir, name))
  .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);

if (installers.length === 0) {
  throw new Error(`未找到 NSIS 安装包: ${nsisDir}`);
}

copyFileSync(installers[0], join(releaseDir, basename(installers[0])));
console.log(`已复制构建产物到 ${releaseDir}`);
