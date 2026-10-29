import { ArrowLeftRight, ChevronDown, ChevronUp } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AppSettings, ThemeName, UsageMode, UsageProvider, UsageSnapshot, UsageWindow } from "../shared/types";
import { desktopApi } from "./desktopApi";
import { Settings2 } from "lucide-react";
import { DisplaySettings } from "./DisplaySettings";
import { UsageAnalytics, UsageComparison } from "./UsageAnalytics";
import dockIcon from "./assets/dock-icon.png";
import claudeIcon from "./assets/claude-icon.png";

const refreshIntervalMs = 5_000;
const numberFormatter = new Intl.NumberFormat("zh-CN");
const dateFormatter = new Intl.DateTimeFormat("zh-CN", {
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false
});
const fullDateFormatter = new Intl.DateTimeFormat("zh-CN", {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false
});

function ratio(value: number, total: number) {
  if (total <= 0) {
    return 0;
  }

  return Math.min(100, Math.max(0, (value / total) * 100));
}

function remainingOf(window: UsageWindow) {
  return Math.max(0, window.total - window.used);
}

function formatToken(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return "--";
  }

  if (value >= 100_000_000) {
    return `${trimFixed(value / 100_000_000, 2)}亿`;
  }
  if (value >= 10000) {
    return `${trimFixed(value / 10000, 1)}万`;
  }

  return numberFormatter.format(value);
}

function trimFixed(value: number, fractionDigits: number) {
  return value
    .toFixed(fractionDigits)
    .replace(/\.0+$/, "")
    .replace(/(\.\d*?)0+$/, "$1");
}

function setHoverRegion(region: "main" | "dock", active: boolean) {
  void desktopApi.setHoverRegion(region, active);
}

function formatDate(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "";
  }

  return dateFormatter.format(date);
}

function formatClock(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "--";
  }

  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false
  }).format(date);
}

function formatResetDistance(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "--";
  }

  const seconds = Math.max(0, Math.floor((date.getTime() - Date.now()) / 1000));
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  if (days > 0) {
    return `${days}天${hours}小时`;
  }
  return `${hours}小时`;
}

function formatFullDate(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "--";
  }

  return fullDateFormatter.format(date);
}

function remainingTimeText(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "--";
  }

  const seconds = Math.max(0, Math.floor((date.getTime() - Date.now()) / 1000));
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  if (days <= 0) {
    return `${hours} 小时`;
  }
  return hours > 0 ? `${days} 天 ${hours} 小时` : `${days} 天`;
}

function totalRemainingText(resets: UsageSnapshot["resets"]) {
  const seconds = resets.reduce((total, reset) => {
    const expiresAt = new Date(reset.expiresAt).getTime();
    if (Number.isNaN(expiresAt)) {
      return total;
    }
    return total + Math.max(0, Math.floor((expiresAt - Date.now()) / 1000));
  }, 0);

  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  return `${days} 天 ${hours} 小时`;
}

function countExpiringSoon(resets: UsageSnapshot["resets"]) {
  return resets.filter((reset) => {
    const expiresAt = new Date(reset.expiresAt).getTime();
    return !Number.isNaN(expiresAt) && expiresAt - Date.now() <= 7 * 86400 * 1000;
  }).length;
}

function formatDateParts(value: string) {
  const formatted = formatFullDate(value);
  if (formatted === "--") {
    return { date: "--", time: "" };
  }

  const [date, time] = formatted.split(" ");
  return { date: date ?? formatted, time: time ?? "" };
}

function compactPlanName(value: string | null | undefined) {
  const raw = value?.trim();
  if (!raw) {
    return "";
  }
  const normalized = raw.toLowerCase().replaceAll("_", "-").replaceAll(" ", "-");
  if (["prolite", "pro-lite", "pro-5x", "codex-pro-lite", "codex-pro-5x"].includes(normalized)) {
    return "Pro 5X";
  }
  if (["pro", "promax", "pro-max", "pro-20x", "chatgpt-pro", "codex-pro-20x"].includes(normalized)) {
    return "Pro 20X";
  }
  if (normalized.includes("plus")) {
    return "Plus";
  }
  if (normalized.includes("team")) {
    return "Team";
  }
  if (normalized.includes("business")) {
    return "Business";
  }
  if (normalized.includes("enterprise")) {
    return "Enterprise";
  }
  if (normalized.includes("free")) {
    return "Free";
  }
  return raw;
}

function sortWindows(windows: UsageWindow[]) {
  return [...windows].sort((a, b) => a.windowDurationMins - b.windowDurationMins);
}

const currentWindowLabel = window.__TAURI_INTERNALS__ ? getCurrentWindow().label : "main";

export function App() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [switching, setSwitching] = useState(false);
  const [snapshot, setSnapshot] = useState<UsageSnapshot | null>(null);
  const [expanded, setExpanded] = useState(true);
  const [analyticsView, setAnalyticsView] = useState<"cycle" | "providers" | null>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const comparisonRef = useRef<HTMLDivElement>(null);
  const [contentHeight, setContentHeight] = useState(470);
  const [loading, setLoading] = useState(true);
  const [codexConnected, setCodexConnected] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let disposed = false;
    const listeners: Array<() => void> = [];

    function applyTheme(theme: ThemeName) {
      document.documentElement.dataset.theme = theme;
    }

    void desktopApi.getSettings().then((settings) => {
      if (!disposed) {
        applyTheme(settings.theme);
        setSettings(settings);
      }
    }).catch((error) => { if (!disposed) setError(String(error)); });
    if (window.__TAURI_INTERNALS__) {
      function keepListener(unlisten: () => void) {
        if (disposed) {
          unlisten();
        } else {
          listeners.push(unlisten);
        }
      }
      void listen<ThemeName>("theme-changed", (event) => applyTheme(event.payload)).then(keepListener);
      void listen<AppSettings>("usage-display-changed", (event) => setSettings(event.payload)).then(keepListener);
      void listen<string>("settings-error", (event) => setError(event.payload)).then(keepListener);
    }

    return () => {
      disposed = true;
      listeners.forEach((unlisten) => unlisten());
    };
  }, []);

  const provider = settings?.activeProvider ?? "codex";
  const providerName = provider === "claude" ? "Claude" : "Codex";

  useEffect(() => {
    if (!settings) return;
    let mounted = true;
    let inFlight = false;
    setSnapshot(null);
    setLoading(true);
    setError("");
    setCodexConnected(false);

    function applySnapshot(usage: UsageSnapshot, connected?: boolean) {
      setSnapshot(usage);
      setError("");
      if (connected !== undefined) {
        setCodexConnected(connected);
      }
    }

    async function loadLatest() {
      if (inFlight) return;
      inFlight = true;
      try {
        const usage = await desktopApi.getUsageSnapshot(provider);
        if (mounted) {
          applySnapshot(usage, usage.accountSource === "codexAppServer" || usage.accountSource === "claudeMod");
        }
      } catch (loadError) {
        if (mounted) {
          setCodexConnected(false);
        }
        if (mounted) {
          setError(String(loadError));
          if (provider === "claude") setSnapshot(null);
        }
      } finally {
        inFlight = false;
        if (mounted) {
          setLoading(false);
        }
      }
    }

    async function boot() {
      try {
        const cached = await desktopApi.getCachedUsageSnapshot(provider);
        if (mounted && cached) { applySnapshot(cached); setLoading(false); }
      } catch (error) {
        if (mounted) setError(String(error));
      }
      if (mounted) await loadLatest();
    }

    void boot();
    const timer = window.setInterval(loadLatest, refreshIntervalMs);

    return () => {
      mounted = false;
      window.clearInterval(timer);
    };
  }, [provider, settings !== null]);

  const windows = useMemo(() => sortWindows(snapshot?.windows ?? []), [snapshot?.windows]);
  const sortedResets = useMemo(
    () =>
      [...(snapshot?.resets ?? [])].sort(
        (a, b) => new Date(a.expiresAt).getTime() - new Date(b.expiresAt).getTime()
      ),
    [snapshot?.resets]
  );
  const resetCount = snapshot?.resetCredits?.available ?? sortedResets.length;
  const canExpand = resetCount > 0 && sortedResets.length > 0;
  const resetExpanded = expanded && canExpand;
  const statusClass = codexConnected ? "is-connected" : loading ? "is-loading" : "is-offline";
  const statusTitle = codexConnected ? (provider === "claude" ? "Claude 账号额度已同步" : "Codex 已连接") : loading ? `正在读取 ${providerName}` : `${providerName} 数据未更新`;

  useLayoutEffect(() => {
    const content = contentRef.current;
    if (!content) return;
    const measure = () => setContentHeight(Math.ceil(Math.max(
      content.getBoundingClientRect().height + 2,
      comparisonRef.current ? comparisonRef.current.getBoundingClientRect().height + 26 : 0
    )));
    const observer = new ResizeObserver(measure);
    observer.observe(content);
    if (comparisonRef.current) observer.observe(comparisonRef.current);
    measure();
    return () => observer.disconnect();
  }, [analyticsView]);

  useEffect(() => {
    if (currentWindowLabel === "main") {
      void desktopApi.setExpanded(contentHeight > 470, Math.max(0, contentHeight - 470)).catch((error) => {
        console.error("调整组件高度失败", error);
      });
    }
  }, [contentHeight]);

  if (currentWindowLabel === "dock") {
    return <DockBar provider={provider} error={error} loading={loading} snapshot={snapshot} windows={windows} />;
  }

  async function changeMode(mode: UsageMode) {
    if (!settings) return;
    setSwitching(true);
    try { setSettings(await desktopApi.setUsageMode(mode, settings.rotationIntervalSecs)); }
    catch (error) { setError(String(error)); }
    finally { setSwitching(false); }
  }

  function toggleExpanded() {
    if (!canExpand) {
      return;
    }

    setExpanded((previous) => !previous);
  }

  function toggleTokenExpanded() {
    setAnalyticsView((previous) => previous === "cycle" ? null : "cycle");
  }

  return (
    <main
      className={resetExpanded ? "shell is-expanded" : "shell"}
      onMouseEnter={() => setHoverRegion("main", true)}
      onMouseLeave={() => setHoverRegion("main", false)}
    >
      <section className={canExpand ? "widget has-reset-details" : "widget"} aria-label={`${providerName} 用量组件`}>
        <div className="widget-scroll" role="region" aria-label="用量与重置详情" tabIndex={0} inert={analyticsView !== null || settingsOpen}><div className="widget-content" ref={contentRef}>
        <div className="chrome drag-region" data-tauri-drag-region>
          <div className="brand">
            <span className="brand-icon-wrap" title={statusTitle}>
              <img className="brand-icon" src={provider === "codex" ? dockIcon : claudeIcon} alt="" />
              <span className={`status-dot ${statusClass}`} />
            </span>
            <strong className="brand-title"><span>{providerName.toUpperCase()}</span><em>用量核心</em></strong>
          </div>
          <div className="plan-meta">
            <span className="plan-badge" title={snapshot?.planError ?? snapshot?.planName}>{provider === "claude" ? snapshot?.planName ?? "读取套餐中" : compactPlanName(snapshot?.planName)}</span>
          </div>
        </div>

        <div className="source-toolbar">
          <div className="source-tabs" role="group" aria-label="用量来源">
            {([{ mode: "codex", label: "Codex" }, { mode: "claude", label: "Claude" }, { mode: "rotate", label: "自动轮播" }] satisfies Array<{mode: UsageMode; label: string}>).map(({mode, label}) => (
              <button key={mode} aria-pressed={settings?.usageMode === mode} disabled={!settings || switching} onClick={() => void changeMode(mode)}>{label}</button>
            ))}
          </div>
          <div className="source-actions">
            <button className="comparison-button" aria-label="Codex 与 Claude 同周期对比" aria-expanded={analyticsView === "providers"} aria-controls="usage-comparison-panel"
              onClick={() => setAnalyticsView((previous) => previous === "providers" ? null : "providers")}><ArrowLeftRight size={14} />同周期对比</button>
            <button className="settings-button" title="显示设置：屏幕、轮播间隔、Claude 接入" aria-label="显示设置" disabled={!settings} onClick={() => setSettingsOpen(true)}><Settings2 size={16} /></button>
          </div>
        </div>
        {error && snapshot && <div className="data-warning" role="alert">更新失败，显示上次数据：{error}</div>}

        {snapshot ? (
          <>
            <div className={windows.length === 1 ? "quota-grid is-single" : "quota-grid"}>
              {windows.map((window) => (
                <UsageCard key={window.id} window={window} />
              ))}
            </div>

            <UsageAnalytics view={analyticsView} onToggle={toggleTokenExpanded} snapshot={snapshot} />
            {snapshot.planError && <div className="data-warning" role="status">套餐：{snapshot.planError}</div>}
            {snapshot.usageWarning && <div className="data-warning" role="status">{snapshot.usageWarning}</div>}

            {provider === "claude" ? (
              <div className="claude-source-note"><strong>Claude 账号额度</strong><p>最近同步：{formatFullDate(snapshot.updatedAt)}</p><p>在线 Code 桥接每 2 分钟查询账号额度，包含 Projects 等入口的消耗。关闭全部 Code 会话后同步暂停。</p><p>Token 明细仅统计已接入的 Code 请求。</p><p>银行重置余额：尚未提供。</p></div>
            ) : canExpand ? (
              <ResetOverview
                resets={sortedResets}
                availableCount={resetCount}
                expanded={resetExpanded}
                onToggle={toggleExpanded}
              />
            ) : (
              <div className="reset-empty" role="status">
                {resetCount === 0 ? "没有重置次数" : `重置次数：${resetCount}`}
              </div>
            )}

            {resetExpanded && (
              <ResetDetails resets={sortedResets} availableCount={resetCount} updatedAt={snapshot.updatedAt} />
            )}
          </>
        ) : <div className="error-panel">{error || "正在读取数据"}</div>}
        </div></div>
        <UsageComparison view={analyticsView} onViewChange={setAnalyticsView} snapshot={snapshot} contentRef={comparisonRef} />

        {loading && <div className="loading-sheen" />}
        {settingsOpen && settings && <DisplaySettings settings={settings} onChange={setSettings} onClose={() => setSettingsOpen(false)} />}
      </section>
    </main>
  );
}

function DockBar({
  provider,
  error,
  loading,
  snapshot,
  windows
}: {
  provider: UsageProvider;
  error: string;
  loading: boolean;
  snapshot: UsageSnapshot | null;
  windows: UsageWindow[];
}) {
  const singleWindow = windows.length === 1;
  return (
    <main
      className="dock-shell"
      onMouseEnter={() => setHoverRegion("dock", true)}
      onMouseLeave={() => setHoverRegion("dock", false)}
      onContextMenu={(event) => {
        event.preventDefault();
        void desktopApi.showDockMenu();
      }}
      onDoubleClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
      }}
    >
      <section className={singleWindow ? "dock-widget is-single" : "dock-widget"} aria-label={`${provider} 用量悬浮条`} title={error || (snapshot ? `最近读数：${formatFullDate(snapshot.updatedAt)}` : "等待用量数据")}>
        <img className={loading ? "dock-icon is-loading" : "dock-icon"} src={provider === "codex" ? dockIcon : claudeIcon} alt={provider === "codex" ? "Codex" : "Claude"} />
        <div className={singleWindow ? "dock-meters is-single" : "dock-meters"}>
          {windows.length === 0 && <span className="dock-empty">{provider === "claude" ? "Claude" : "Codex"} · {loading ? "读取中…" : "等待数据 · 右键设置"}</span>}
          {windows.map((usageWindow) => (
            <DockMeter key={usageWindow.id} single={singleWindow} window={usageWindow} />
          ))}
        </div>
      </section>
    </main>
  );
}

function DockMeter({
  single,
  window
}: {
  single: boolean;
  window: UsageWindow;
}) {
  const remainingPercent = Math.round(ratio(remainingOf(window), window.total));
  const label = window.label.replace(/用量$/, "");
  const resetText = window.windowDurationMins >= 1440
    ? formatResetDistance(window.resetAt)
    : formatClock(window.resetAt);

  return (
    <div className="dock-meter">
      <div className="dock-track">
        <span style={{ width: `${remainingPercent}%` }} />
      </div>
      {single && <span className="dock-percent">{remainingPercent}%</span>}
      <strong>{label}</strong>
      <span className="dock-reset" title={`重置时间：${formatFullDate(window.resetAt)}`}>{single ? `· ${resetText}` : `${remainingPercent}% · ${resetText}`}</span>
    </div>
  );
}

function ResetOverview({
  resets,
  availableCount,
  expanded,
  onToggle
}: {
  resets: UsageSnapshot["resets"];
  availableCount: number;
  expanded: boolean;
  onToggle: () => void;
}) {
  const totalCount = Math.max(availableCount, resets.length);
  const nearest = resets[0]?.expiresAt;
  const expiringSoon = countExpiringSoon(resets);

  return (
    <div
      className="reset-overview"
      role="button"
      tabIndex={0}
      onClick={onToggle}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onToggle();
        }
      }}
      aria-expanded={expanded}
    >
      <div className="reset-overview-head">
        <div className="reset-overview-main">
          <span>重置次数</span>
          <strong>
            {availableCount}/{totalCount}
          </strong>
        </div>
        <span className="reset-overview-icon">{expanded ? "收起明细" : "查看每次过期时间"}{expanded ? <ChevronUp size={16} /> : <ChevronDown size={16} />}</span>
      </div>
      <div className="reset-summary-grid">
        <SummaryCard tone="green" label="可用" value={`${availableCount}`} />
        <SummaryCard tone="yellow" label="即将过期" value={`${expiringSoon}`} />
        <SummaryCard tone="blue" label="总剩余" value={totalRemainingText(resets)} />
        <SummaryCard tone="orange" label="最近到期" value={nearest ? remainingTimeText(nearest) : "--"} />
      </div>
    </div>
  );
}

function ResetDetails({
  resets,
  availableCount,
  updatedAt
}: {
  resets: UsageSnapshot["resets"];
  availableCount: number;
  updatedAt: string;
}) {
  const totalCount = Math.max(availableCount, resets.length);

  return (
    <section className="reset-detail-panel">
      <div className="reset-detail-head">
        <div>
          <h2>重置次数详情</h2>
          <span>更新于 {formatFullDate(updatedAt)}</span>
        </div>
        <strong>
          {availableCount}/{totalCount}
        </strong>
      </div>

      <div className="reset-table">
        <div className="reset-row reset-row-head">
          <span>次数</span>
          <span>状态</span>
          <span>到期时间</span>
          <span>剩余</span>
        </div>
        <div className="reset-scroll">
          {resets.map((reset, index) => {
            const expiresAt = formatDateParts(reset.expiresAt);

            return (
              <div className="reset-row" key={reset.id}>
                <strong>次数 {index + 1}</strong>
                <span className="status-pill">可用</span>
                <span className="reset-date">
                  <span>{expiresAt.date}</span>
                  {expiresAt.time && <small>{expiresAt.time}</small>}
                </span>
                <span>{remainingTimeText(reset.expiresAt)}</span>
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}

function SummaryCard({
  tone,
  label,
  value
}: {
  tone: "green" | "yellow" | "blue" | "orange";
  label: string;
  value: string;
}) {
  return (
    <div className="reset-summary-card">
      <span className={`summary-dot is-${tone}`} />
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function UsageCard({ window }: { window: UsageWindow }) {
  const remaining = remainingOf(window);
  const remainingPercent = ratio(remaining, window.total);
  const usedPercent = ratio(window.used, window.total);
  const isPercent = window.displayMode === "percent";

  return (
    <article className="usage-card">
      <div className="usage-head">
        <span>{window.label}</span>
        <strong>{Math.round(remainingPercent)}% 剩余</strong>
      </div>
      <div className="usage-main">
        <strong>{numberFormatter.format(Math.round(remaining))}</strong>
        <span>{isPercent ? "% 可用" : `/ ${numberFormatter.format(window.total)}`}</span>
      </div>
      <div className="usage-sub">
        <span>
          已用 {numberFormatter.format(Math.round(window.used))}
          {isPercent ? "%" : ""}
        </span>
        <span>{window.resetAt ? `重置 ${formatDate(window.resetAt)}` : "重置时间暂未提供"}</span>
      </div>
      <div className="meter" aria-label={`${window.label} 已用 ${Math.round(usedPercent)}%`}>
        <span style={{ width: `${usedPercent}%` }} />
      </div>
    </article>
  );
}
