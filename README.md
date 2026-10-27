# CodexInfo

中文 | [English](README.en.md)

CodexInfo 是一个 Windows 桌面用量组件，可切换显示 Codex 与 Claude Code 的 5 小时、7 天用量、重置时间、Token 用量和 API 等价成本，并按真实额度窗口对比本周期与上周期。

界面使用黑色半透明玻璃质感、橙色高亮和像素字体。应用默认只显示任务栏悬浮条，鼠标移入悬浮条时会在任务栏上方弹出完整组件，鼠标移出悬浮条和组件范围后自动收起。

## 功能

- 5 小时用量与 7 天用量：显示剩余百分比、已用比例和重置时间。
- 任务栏悬浮条：显示两条用量进度和重置信息，可在右键菜单中开关。
- Hover 弹出组件：悬浮条移入显示主组件，移出后自动收起，并记忆收起前的展开状态。
- 重置机会：默认显示可用次数、即将过期、总剩余、最近到期；展开后显示每一次重置机会的到期时间。
- 订阅信息：显示套餐名称。
- 数据切换：手动选择 Codex / Claude，或每 5、10、30、60 秒自动轮播；悬浮条、面板与托盘同步切换。
- 显示器选择：右键菜单或显示设置中选择屏幕，悬浮条和面板一起移动；支持副屏任务栏、负坐标和不同缩放比例。所选屏幕断开时暂用主屏，重连后恢复。
- Token 与 API 等价成本：按真实重置窗口对比本周期和上周期，分别统计输入、缓存读取、缓存写入、输出、模型、成本，以及每消耗 1% 额度对应的本机 Token / 成本。
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

### Claude Code 接入

1. 在 Claude 桌面版打开 Code 页，或安装终端版 Claude Code。桌面版内置 Claude Code 需至少 v2.1.286，终端版需至少 v2.1.287。
2. 启动新版本 CodexInfo，打开“显示设置”，点击“接入 Claude Code”。组件通过 Claude 官方 CLI 安装本地 `codexinfo-usage` 插件；支持普通桌面版及 Microsoft Store 版的内置 CLI，终端版需可从 PATH 找到 `claude.exe`。
3. 在已打开的 Code 会话执行 `/reload-plugins` 并保持会话在线。插件启动时立即查询账号额度，此后每 2 分钟查询一次；Projects、网页和桌面聊天的消耗都属于同一账号额度。组件每 5 秒读取同步结果，不调用推理接口查询额度。Code 请求的 Token 及模型仍通过 `turn.step` 采集。

插件通过官方 Mods 的 `session.authorize` 获取不透明授权句柄，再由 Claude 宿主请求内置 `/api/oauth/usage` 账号额度接口，令牌留在 Claude 内，不会导出给插件或组件。只导出计数、时间、会话 / 请求标识和模型，不导出聊天文本及工具参数。多会话使用本地文件锁共享查询间隔，429 按服务端 `Retry-After` 等待；账号额度覆盖同一文件，请求 Token 按标识去重。移动免安装程序后应重新接入，以更新插件中的程序路径。

这条同步需要至少一个加载插件的 Code 会话在线，全部关闭后会暂停；不必向模型发送消息。账号查询失败会明确显示原因，不回退到旧会话快照；超过 5 分钟未更新会提示桥接离线。到期窗口不继续显示旧百分比，缺失窗口不补零。账号额度接口是 Claude 内置查询链路，尚无稳定的公开 API 契约，若字段或权限变化会明确报错。银行重置余额仍显示“尚未提供”；Codex 原有银行重置余额与到期详情保留。组件不执行银行重置。

官方资料：[Mods](https://code.claude.com/docs/en/plugins/mods/overview)、[Mods API](https://code.claude.com/docs/en/plugins/mods/reference)、[银行重置语义](https://support.claude.com/en/articles/17007452-what-is-a-limit-reset)。

### 周期与成本口径

- 周期从平台返回的重置时间和窗口长度确定，不再用自然周 / 自然月代替额度周期。上周期只使用实际观测过的窗口；没有历史时显示缺口。
- Claude Pro 的每周重置日和时刻由账号固定分配，不随开始使用或订阅开通日期改变，并非所有账号统一按自然周重置；以返回的重置时间为准。参见 [Pro 官方说明](https://support.claude.com/en/articles/8325606-what-is-the-pro-plan)。悬浮条倒计时保留天数与小时，悬停可查看完整日期。
- Codex 读取现存的 `sessions` 与 `archived_sessions` 中的 Token 事件，不再只截取最近 120 个日志文件。首次索引可能耗时，后续通过文件元数据缓存只重新解析有变化的文件。
- Claude 请求历史从插件接入后积累，不会把当前上下文 Token 当作累计用量，不伪造历史。其它设备、网页、未接入插件的会话不在本机 Token 统计范围内。
- 每 1% 的指标匹配同一时间区间的请求与额度变化。读数取整未增长时先累计请求，额度增长后再一并计入，尚未反映在额度中的尾部请求暂不计入。额度回落区间不参与该指标，也不自动归因为银行重置，因为可能来自平台调整。银行重置、跨端用量和采样缺口均会影响比较。
- API 等价成本按请求模型及输入 / 缓存 / 输出分别计算，统一使用 **2026-10-24 的官方标准短上下文单价**，用于跨周期比较；不是订阅的扣额公式或真实账单。不含工具费用、长上下文、速度、地域溢价。未知模型不套用默认单价，并显示缺失模型；缓存写入缺少 TTL 时显示价格区间。
- 当前进行中的周期与完整上周期总量不宜直接比较。每 1% 的指标也只反映观测期间的本机使用，必须结合模型和缓存比例判断，不能单独证明平台减少了额度。

定价来源：[OpenAI](https://developers.openai.com/api/docs/pricing)、[Claude](https://platform.claude.com/docs/en/about-claude/pricing)。

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

Claude 星芒图标复用 Claude 桌面版随包的 `TrayIconTemplate-Dark@3x.png`，用于标识对应的数据来源。

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

## 故障监控与日志

组件启动时自动开启独立监控进程，主进程退出后监控进程记录退出码并自行结束，不自动重启组件。监控每 30 秒检查内存、句柄、GDI / USER 对象数量和窗口响应情况，健康摘要每 10 分钟落盘一次；无响应、恢复、采样错误等状态变化及进程退出立即写入，退出记录附带最后一次内存采样及其时间间隔。Rust panic、菜单退出和窗口生命周期及时写入运行日志；前端错误首条立即记录，后续按分钟合并次数及最后一条错误，正常退出前也会汇总尚未写入的错误。

日志保存在 `%LOCALAPPDATA%\net.local.codex-info\logs`：

- `lifecycle.log`：启动、监控接入、前端错误、panic 和退出事件。
- `monitor-<PID>-<创建时间>.log`：独立心跳、进程退出码和退出分类。
- 同名 `.json`：本次运行阶段和主动退出标识，供退出分类核对。

单个日志文件最多 512 KiB，轮转后保留一份上一段日志；超长单条记录会明确标记截断。诊断文件总量限制为 8 MiB，已结束运行最多保留最近 12 次且不超过 7 天，达到总量上限时提前清理最旧记录。每次落盘前在跨进程目录锁内检查并清理，覆盖未完整启动留下的文件；活动运行通过 PID 和创建时间识别，遗留的就绪文件不会阻止清理。若活动记录已占满上限，写入明确失败，不继续增长。日志不会主动执行每条强制刷盘。监控无法独立启动时明确记录 `monitor_start_failed`，不会伪装成监控正常。日志仅保存在本机；分享前应检查错误文本中的本地路径及其它敏感信息。进程退出码可以确定终止结果，但不保证能识别执行强制结束的程序。

窗口检查还会记录悬浮窗坐标、隐藏、屏幕外、任务栏遮挡及全屏避让状态，能区分“进程退出”和“进程正常但看不见”。主进程在层级状态改变时记录 `dock_state`，恢复操作会回读真实层级；全屏期间隐藏悬浮窗并保留置顶状态，同屏全屏应用结束后通过原有显示流程恢复原窗口到任务栏前方。这些记录沿用上述降频和总量限制。

## 隐私

CodexInfo 不上传会话日志。Codex 使用本机登录信息查询官方账号接口；Claude 通过本地插件导出计数，CodexInfo 不读取 Claude 登录令牌。额度采样历史仅保存在应用数据目录，用于周期对比。
