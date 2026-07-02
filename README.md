# Codex 用量组件

Windows 桌面托盘组件，用于显示 Codex 当前用量。界面采用黑色半透明窗口、橙色高亮、点阵字体和中文文案。

## 使用

运行后只显示托盘图标，不在任务栏显示窗口图标。托盘右键菜单包含：

- `显示组件`：勾选显示窗口，取消勾选隐藏窗口。
- `保持置顶`：勾选后窗口保持置顶，取消后按普通窗口处理。
- `开机启动`：跟随系统登录启动。
- `退出`：关闭应用。

窗口会记录上次所在显示器和位置；再次启动时优先恢复上次位置。启动时会先显示上一份真实缓存，读取到最新数据后自动刷新。

## 数据来源

应用通过本机 Codex CLI 的官方 app-server 读取基础用量数据：

- `account/read`：读取当前账号和套餐类型。
- `account/rateLimits/read`：读取 Codex 5 小时、7 天用量窗口和重置时间。

重置机会读取本机 `~/.codex/auth.json` 中的 ChatGPT 登录令牌，查询 `https://chatgpt.com/backend-api/wham/rate-limit-reset-credits`：

- `available_count`：显示可用重置机会数量。
- `credits[].expires_at`：展开后逐条显示每一次重置机会的到期时间。

Token 输入、输出会从本机真实 Codex session 日志中的 `token_count` 事件读取。如果官方接口和本机 session 日志都没有可用数据，应用会显示读取错误；不会使用示例数据兜底。

## 开发

```powershell
npm install --registry=https://registry.npmmirror.com
npm run tauri:dev
```

## 打包 Windows 安装包

```powershell
npm run typecheck
npm run dist:win
```

Tauri 2 在 Windows 上需要 Rust、Microsoft C++ Build Tools 和 WebView2。构建脚本会把常用产物复制到项目根目录的 `release/`：

- `release/CodexInfo.exe`：直接运行版。
- `release/CodexInfo_0.1.0_x64-setup.exe`：安装包。
