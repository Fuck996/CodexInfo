# CodexInfo

中文 | [English](README.en.md)

CodexInfo 是一个 Windows 桌面用量组件，用来显示本机 Codex 的 5 小时用量、7 天用量、重置时间、重置机会、套餐名称、Token 用量和 API 等价成本。

界面使用黑色半透明玻璃质感、橙色高亮和像素字体。应用默认只显示任务栏悬浮条，鼠标移入悬浮条时会在任务栏上方弹出完整组件，鼠标移出悬浮条和组件范围后自动收起。

## 功能

- 5 小时用量与 7 天用量：显示剩余百分比、已用比例和重置时间。
- 任务栏悬浮条：显示两条用量进度和重置信息，可在右键菜单中开关。
- Hover 弹出组件：悬浮条移入显示主组件，移出后自动收起，并记忆收起前的展开状态。
- 重置机会：默认显示可用次数、即将过期、总剩余、最近到期；展开后显示每一次重置机会的到期时间。
- 订阅信息：显示套餐名称。
- Token 与 API 等价成本：Token 数值支持 `万` / `亿` 单位，点击 Token 区域展开近 7 天、前 7 天、近 30 天、前 30 天的 API 等价成本。
- 托盘菜单：显示组件、保持置顶、托盘显示用量、显示任务栏入口、开机启动、退出。
- 本地缓存：启动时先显示上次缓存，读取到最新数据后自动刷新。
- 无示例数据兜底：读取失败时显示真实错误，不伪造用量、订阅或重置次数。

## 下载

请到 [GitHub Releases](https://github.com/Fuck996/CodexInfo/releases) 下载：

- `CodexInfo.exe`：免安装运行版。
- `CodexInfo_<version>_x64-setup.exe`：Windows 安装包。

## 使用

1. 启动应用后，默认在任务栏右侧显示悬浮条。
2. 鼠标移入悬浮条，完整组件会在任务栏上方显示。
3. 鼠标离开悬浮条和组件区域后，组件自动收起。
4. 右键悬浮条或托盘图标可打开菜单。

## 数据来源

CodexInfo 优先读取本机 Codex 的真实数据：

- `account/read`：读取当前账号和套餐信息。
- `account/rateLimits/read`：读取 5 小时、7 天用量窗口和重置时间。
- `~/.codex/auth.json`：读取本机 ChatGPT 登录令牌，用于查询重置机会。
- `https://chatgpt.com/backend-api/wham/rate-limit-reset-credits`：读取可用重置机会和每次到期时间。
- 本机 Codex session 日志中的 `token_count` 事件：读取 Token 输入、缓存输入、输出和周期统计。

所有数据都来自本机 Codex / ChatGPT 登录状态和本地会话日志。没有可用数据时，应用会暴露错误，不会显示假数据。

## 开发

```powershell
npm install --registry=https://registry.npmmirror.com
npm run tauri:dev
```

## 构建 Windows 版本

```powershell
npm run typecheck
npm run dist:win
```

构建产物会复制到项目根目录的 `release/`：

- `release/CodexInfo.exe`
- `release/CodexInfo_<version>_x64-setup.exe`

## 自动发布

`npm run dist:win` 会先自动递增 patch 版本号，再同步 `package.json`、`package-lock.json`、`src-tauri/Cargo.toml` 和 `src-tauri/tauri.conf.json`，最后构建 Windows 产物。

推送版本 tag 会自动触发 GitHub Actions 构建 Windows 版本并上传到 Release：

```powershell
git tag v1.0.1
git push origin v1.0.1
```

Action 构建同样会执行版本递增，并使用递增后的 `package.json` 版本作为 Release tag。也可以在 GitHub Actions 页面手动运行 `Release Windows Build`。

## 技术栈

- Tauri 2
- React
- TypeScript
- Rust
- Vite

## 环境要求

- Windows 10 / Windows 11
- WebView2 Runtime
- Codex 已在本机登录

开发和打包还需要：

- Node.js
- Rust
- Microsoft C++ Build Tools

## 隐私

CodexInfo 不上传你的 Codex session 日志。应用只在本机读取 Codex / ChatGPT 登录信息，并向 ChatGPT 官方接口请求当前账号的用量和重置机会。
