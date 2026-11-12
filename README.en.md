# CodexInfo

[中文](README.md) | English

CodexInfo is a Windows desktop usage widget for Codex and Claude Code. It displays 5-hour and 7-day usage, reset times, Token usage, and estimated API-equivalent cost, with comparisons between actual quota windows.

The UI uses a black translucent glass style, orange highlights, and a pixel-style font. By default, the app only shows a compact taskbar dock. Hovering over the dock opens the full widget above the taskbar; moving the cursor away from both the dock and the widget collapses it automatically.

## Features

- 5-hour and 7-day usage: remaining percentage, used ratio, and reset time.
- Taskbar dock: compact usage bars and reset information, configurable from the context menu.
- Hover widget: opens from the dock, collapses automatically, and remembers the previous expanded state.
- Reset credits: shows available count, expiring soon, total remaining time, nearest expiration, and per-credit expiration details.
- Subscription info: displays plan name.
- Switch manually between Codex and Claude, or rotate every 5, 10, 30, or 60 seconds. The dock, widget, and tray share the selected source.
- Choose the display for the dock and widget, including secondary taskbars, negative coordinates, and different scale factors. A disconnected target temporarily uses the primary display and returns when reconnected.
- Token and API-equivalent cost: compare current and previous quota windows, including cache reads/writes, models, and observed Tokens/cost per percentage point of quota.
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

### Claude Code setup

Claude monitoring is off by default. Enable it in Display settings after connecting the bridge. Turning it off stops background collection, switches the display to Codex, and hides the Claude and rotation controls. Local history remains available for comparisons.

Open Display settings in CodexInfo and select Connect Claude Code. The app installs a local plugin using the official Claude CLI, including the CLI bundled with Microsoft Store Claude Desktop. After signing in through the official CLI, CodexInfo runs a background host to collect account usage at least 30 minutes apart, including consumption from Projects, web, and desktop chat. Existing Code sessions can run `/reload-plugins` to record their request tokens. Desktop's bundled Claude Code needs v2.1.286 or later. The terminal CLI needs v2.1.287 or later.

The plugin uses official Mods `session.authorize` to obtain an opaque handle, then asks the Claude host to call its internal `/api/oauth/usage` endpoint. Credentials remain inside Claude. Only counters, timestamps, session/request identifiers, and models are exported. The `turn.step` observer still records Code Tokens/model. A local file lock coordinates polling across sessions, and HTTP 429 respects `Retry-After`, with a one-hour wait when the header is missing or unusable. Opening the comparison chart reads local history only. Account readings replace one file atomically; Token requests are deduplicated by identifier. Reconnect after moving the portable executable so the plugin uses its new path.

Query failures are exposed and readings older than 35 minutes are marked stale. Expired windows are removed and missing readings are not replaced with zero. The account endpoint is Claude's internal usage path without a stable public API contract. Incompatible fields or permissions fail explicitly. Historical Code Tokens start when the plugin is connected. Banked reset balances remain unavailable for Claude. Existing Codex banked reset balances and expiration details remain available. The widget never redeems resets.

### Comparison and pricing

Cycles use the platform's reset timestamp and window duration instead of calendar weeks/months. Codex statistics include existing active and archived logs, with no 120-file cutoff and metadata caching. The previous window must have been observed; missing history remains unavailable. Statistics cover recorded requests on this machine, excluding unrecorded sessions, other devices, and web usage.

Per-percentage-point statistics match Tokens to quota changes over the same intervals. When rounded readings do not move, requests wait until the meter advances; trailing requests not yet reflected in quota remain excluded. Intervals where usage decreases are excluded, without assuming the cause was a banked reset. Compare model/cache usage and sampling coverage as well. These observations alone cannot prove a reduction in the provider's limits.

API-equivalent costs use each model's official standard short-context prices as of **2026-10-24** as a fixed comparison basis. Unknown model prices remain unavailable. Cache writes with an unknown TTL produce a price range. This is not the subscription quota formula or an invoice and excludes tool fees and context/speed/region premiums.

Sources: [Claude Mods](https://code.claude.com/docs/en/plugins/mods/overview), [Mods API](https://code.claude.com/docs/en/plugins/mods/reference), [banked resets](https://support.claude.com/en/articles/17007452-what-is-a-limit-reset), [OpenAI pricing](https://developers.openai.com/api/docs/pricing), [Claude pricing](https://platform.claude.com/docs/en/about-claude/pricing).

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

## Failure monitoring and logs

Each widget run starts an independent monitor. It records the process exit code and then exits, without restarting the widget. It checks memory, handles, GDI / USER objects, and window responsiveness every 30 seconds, but writes healthy summaries only every 10 minutes. Responsiveness changes, sampling error transitions, and process exits are recorded immediately. Exit records include the last in-memory sample and its age. Rust panics, menu exit requests, and window lifecycle events are written promptly. The first frontend error is recorded immediately; further errors are coalesced each minute with a count and the last error, and any pending errors are flushed before a normal exit.

Logs are stored in `%LOCALAPPDATA%\net.local.codex-info\logs`: `lifecycle.log` contains runtime events; `monitor-<PID>-<creation-time>.log` contains samples and the final exit code; its `.json` file records the run phase and exit intent. Each segment is capped at 512 KiB with one previous segment; oversized entries are explicitly marked as truncated. Diagnostic files have an 8 MiB total budget. At most 12 completed runs are retained for up to 7 days; the oldest runs are removed sooner if necessary to meet the budget. Every write checks retention under a cross-process directory lock, including files left by incomplete startups. PID and creation time identify active processes, so stale ready markers do not prevent cleanup. Writes fail explicitly if active records exhaust the budget. Logging does not force a disk sync per entry. A failed monitor launch is explicitly logged as `monitor_start_failed`.

Window checks also record dock bounds, hidden and offscreen states, taskbar occlusion, and fullscreen yielding, distinguishing a process exit from a running but obscured widget. The main process writes `dock_state` only on layer state changes and verifies the actual order after restoring the dock. During fullscreen use the dock is hidden while retaining its topmost state. When the fullscreen app on the same monitor closes, the existing show flow restores the original dock above the taskbar. These records use the same throttling and storage limits.

Logs stay local. Review error text and local paths before sharing them. An exit code identifies the termination result but does not necessarily identify which program terminated the process.

## Privacy

CodexInfo does not upload session logs. Codex uses local credentials to request official usage/reset endpoints; Claude exports counters through the local plugin, without CodexInfo reading Claude credentials. Quota observation history stays in the app data directory.
