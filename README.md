# CodexInfo

<p>
  <a href="#中文">中文</a> |
  <a href="#english">English</a>
</p>

<details open id="中文">
<summary><strong>中文</strong></summary>

## 简介

CodexInfo 是一个 Windows 桌面用量组件，用来显示本机 Codex 的 5 小时用量、7 天用量、重置时间、重置机会、订阅到期时间和 Token / API 等价成本。

界面使用黑色半透明玻璃质感、橙色高亮和像素字体。应用默认只显示任务栏悬浮条，鼠标移入悬浮条时会在任务栏上方弹出完整组件，鼠标移出悬浮条和组件范围后自动收起。

## 功能

- 5 小时用量与 7 天用量：显示剩余百分比、已用比例和重置时间。
- 任务栏悬浮条：显示两条用量进度和重置信息，可在右键菜单中开关。
- Hover 弹出组件：悬浮条移入显示主组件，移出后自动收起，并记忆收起前的展开状态。
- 重置机会：默认显示可用次数、即将过期、总剩余、最近到期；展开后显示每一次重置机会的到期时间。
- 订阅信息：显示套餐名称和订阅到期时间。
- Token 与 API 等价成本：Token 数值支持 `万` / `亿` 单位，点击 Token 区域展开本周、上周、本月、上月的 API 等价成本。
- 托盘菜单：显示组件、保持置顶、托盘显示用量、显示任务栏入口、开机启动、退出。
- 本地缓存：启动时先显示上次缓存，读取到最新数据后自动刷新。
- 无示例数据兜底：读取失败时显示真实错误，不伪造用量、订阅或重置次数。

## 下载

请到 GitHub Releases 下载：

- `CodexInfo.exe`：免安装运行版。
- `CodexInfo_1.0.0_x64-setup.exe`：Windows 安装包。

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
- `release/CodexInfo_1.0.0_x64-setup.exe`

## 自动发布

推送版本 tag 会自动触发 GitHub Actions 构建 Windows 版本并上传到对应 Release：

```powershell
git tag v1.0.0
git push origin v1.0.0
```

tag 必须与 `package.json` 里的版本一致，例如 `1.0.0` 对应 `v1.0.0`。也可以在 GitHub Actions 页面手动运行 `Release Windows Build`。

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

</details>

<details id="english">
<summary><strong>English</strong></summary>

## Overview

CodexInfo is a Windows desktop usage widget for Codex. It displays 5-hour usage, 7-day usage, reset times, reset credits, subscription expiration, Token usage, and estimated API-equivalent cost.

The UI uses a black translucent glass style, orange highlights, and a pixel-style font. By default, the app shows only a compact taskbar dock. Hovering over the dock opens the full widget above the taskbar; moving the cursor away from both the dock and the widget collapses it automatically.

## Features

- 5-hour and 7-day usage: remaining percentage, used ratio, and reset time.
- Taskbar dock: compact usage bars and reset information, configurable from the context menu.
- Hover widget: opens from the dock, collapses automatically, and remembers the previous expanded state.
- Reset credits: shows available count, expiring soon, total remaining time, nearest expiration, and per-credit expiration details.
- Subscription info: displays plan name and subscription expiration time.
- Token and API-equivalent cost: Token values support Chinese `万` / `亿` units; clicking the Token area expands weekly and monthly API-equivalent cost.
- Tray menu: show widget, keep always on top, show usage in tray, show taskbar entry, launch at startup, and quit.
- Local cache: shows the last cached data on startup, then refreshes automatically after fresh data is loaded.
- No fake fallback data: when data cannot be read, the app surfaces the real error instead of showing demo usage, subscription, or reset data.

## Download

Download from GitHub Releases:

- `CodexInfo.exe`: portable executable.
- `CodexInfo_1.0.0_x64-setup.exe`: Windows installer.

## Usage

1. Start the app. A compact dock appears near the right side of the Windows taskbar by default.
2. Move the cursor over the dock to open the full widget above the taskbar.
3. Move the cursor away from both the dock and the widget to collapse it.
4. Right-click the dock or the tray icon to open the menu.

## Data Sources

CodexInfo reads real local Codex data first:

- `account/read`: current account and plan information.
- `account/rateLimits/read`: 5-hour and 7-day usage windows and reset times.
- `~/.codex/auth.json`: local ChatGPT login token used to query reset credits.
- `https://chatgpt.com/backend-api/wham/rate-limit-reset-credits`: available reset credits and per-credit expiration times.
- Local Codex session `token_count` events: Token input, cached input, output, and period statistics.

All data comes from the local Codex / ChatGPT login state and local session logs. If no data is available, the app reports the real error instead of displaying mock data.

## Development

```powershell
npm install --registry=https://registry.npmmirror.com
npm run tauri:dev
```

## Build for Windows

```powershell
npm run typecheck
npm run dist:win
```

Build artifacts are copied to the root `release/` directory:

- `release/CodexInfo.exe`
- `release/CodexInfo_1.0.0_x64-setup.exe`

## Release Automation

Pushing a version tag triggers GitHub Actions to build the Windows artifacts and upload them to the matching GitHub Release:

```powershell
git tag v1.0.0
git push origin v1.0.0
```

The tag must match the version in `package.json`, for example `1.0.0` maps to `v1.0.0`. You can also run `Release Windows Build` manually from the GitHub Actions page.

## Tech Stack

- Tauri 2
- React
- TypeScript
- Rust
- Vite

## Requirements

- Windows 10 / Windows 11
- WebView2 Runtime
- Codex logged in locally

For development and packaging:

- Node.js
- Rust
- Microsoft C++ Build Tools

## Privacy

CodexInfo does not upload your Codex session logs. It reads Codex / ChatGPT login information locally and requests usage and reset-credit data from official ChatGPT endpoints.

</details>
