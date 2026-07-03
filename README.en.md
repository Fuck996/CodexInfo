# CodexInfo

[中文](README.md) | English

CodexInfo is a Windows desktop usage widget for Codex. It displays local Codex 5-hour usage, 7-day usage, reset times, reset credits, plan name, Token usage, and estimated API-equivalent cost.

The UI uses a black translucent glass style, orange highlights, and a pixel-style font. By default, the app only shows a compact taskbar dock. Hovering over the dock opens the full widget above the taskbar; moving the cursor away from both the dock and the widget collapses it automatically.

## Features

- 5-hour and 7-day usage: remaining percentage, used ratio, and reset time.
- Taskbar dock: compact usage bars and reset information, configurable from the context menu.
- Hover widget: opens from the dock, collapses automatically, and remembers the previous expanded state.
- Reset credits: shows available count, expiring soon, total remaining time, nearest expiration, and per-credit expiration details.
- Subscription info: displays plan name.
- Token and API-equivalent cost: Token values support Chinese `万` / `亿` units; clicking the Token area expands weekly and monthly API-equivalent cost.
- Tray menu: show widget, keep always on top, show usage in tray, show taskbar entry, launch at startup, and quit.
- Local cache: shows the last cached data on startup, then refreshes automatically after fresh data is loaded.
- No fake fallback data: when data cannot be read, the app surfaces the real error instead of showing demo usage, subscription, or reset data.

## Download

Download from [GitHub Releases](https://github.com/Fuck996/CodexInfo/releases):

- `CodexInfo.exe`: portable executable.
- `CodexInfo_<version>_x64-setup.exe`: Windows installer.

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
- `release/CodexInfo_<version>_x64-setup.exe`

## Release Automation

`npm run dist:win` bumps the patch version first, syncs `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`, then builds the Windows artifacts.

Pushing a version tag triggers GitHub Actions to build the Windows artifacts and upload them to a GitHub Release:

```powershell
git tag v1.0.1
git push origin v1.0.1
```

The Action build runs the same version bump and uses the bumped `package.json` version as the Release tag. You can also run `Release Windows Build` manually from the GitHub Actions page.

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
