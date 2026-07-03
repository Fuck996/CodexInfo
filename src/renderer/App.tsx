import { ChevronDown, ChevronUp, Database } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useMemo, useRef, useState } from "react";
import type { TokenPeriodId, TokenPeriodUsage, TokenUsage, UsageSnapshot, UsageWindow } from "../shared/types";
import { desktopApi } from "./desktopApi";
import appIcon from "../../src-tauri/icons/32x32.png";

const refreshIntervalMs = 60_000;
const apiCostPanelExtraHeight = 180;
const resetDetailBaseExtraHeight = 130;
const resetDetailRowExtraHeight = 42;
const numberFormatter = new Intl.NumberFormat("zh-CN");
const moneyFormatter = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 4,
  maximumFractionDigits: 4
});
type ApiCostPeriod = TokenPeriodId;

const apiCostPeriods: Array<{ id: ApiCostPeriod; label: string }> = [
  { id: "thisWeek", label: "本周" },
  { id: "lastWeek", label: "上周" },
  { id: "thisMonth", label: "本月" },
  { id: "lastMonth", label: "上月" }
];
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

function tokenText(value: number | null | undefined) {
  return formatToken(value);
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

function apiEquivalentCostFromUsage(tokenUsage: TokenUsage) {
  const input = tokenUsage.input ?? 0;
  const cached = Math.min(input, Math.max(0, tokenUsage.cachedInput ?? 0));
  const output = tokenUsage.output ?? 0;
  const uncached = Math.max(0, input - cached);
  const total = tokenUsage.total ?? uncached + cached + output;
  const estimated = (uncached / 1_000_000) * 5 + (cached / 1_000_000) * 0.5 + (output / 1_000_000) * 30;

  return { cached, estimated, output, total, uncached };
}

function apiEquivalentCost(snapshot: UsageSnapshot) {
  return apiEquivalentCostFromUsage(snapshot.tokenUsage);
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
    return `${days}天`;
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

function periodStartLabel(value: string | undefined) {
  return value?.split(" · ")[0] ?? "";
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
  const order = new Map<UsageWindow["id"], number>([
    ["fiveHour", 0],
    ["sevenDay", 1]
  ]);

  return [...windows].sort((a, b) => (order.get(a.id) ?? 99) - (order.get(b.id) ?? 99));
}

const currentWindowLabel = window.__TAURI_INTERNALS__ ? getCurrentWindow().label : "main";

export function App() {
  const [snapshot, setSnapshot] = useState<UsageSnapshot | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [tokenExpanded, setTokenExpanded] = useState(false);
  const [loading, setLoading] = useState(true);
  const [codexConnected, setCodexConnected] = useState(false);
  const [error, setError] = useState("");
  const snapshotRef = useRef<UsageSnapshot | null>(null);

  useEffect(() => {
    let mounted = true;

    function applySnapshot(usage: UsageSnapshot, connected?: boolean) {
      snapshotRef.current = usage;
      setSnapshot(usage);
      setError("");
      if (connected !== undefined) {
        setCodexConnected(connected);
      }
    }

    async function loadLatest() {
      try {
        const usage = await desktopApi.getUsageSnapshot();
        if (mounted) {
          applySnapshot(usage, usage.accountSource === "codexAppServer");
        }
      } catch (loadError) {
        if (mounted) {
          setCodexConnected(false);
        }
        if (mounted && !snapshotRef.current) {
          setError(loadError instanceof Error ? loadError.message : "读取失败");
        }
      } finally {
        if (mounted) {
          setLoading(false);
        }
      }
    }

    async function boot() {
      const cached = await desktopApi.getCachedUsageSnapshot();
      if (mounted && cached) {
        applySnapshot(cached);
        setLoading(false);
      }
      await loadLatest();
    }

    void boot();
    const timer = window.setInterval(loadLatest, refreshIntervalMs);

    return () => {
      mounted = false;
      window.clearInterval(timer);
    };
  }, []);

  const windows = useMemo(() => sortWindows(snapshot?.windows ?? []), [snapshot?.windows]);
  const sortedResets = useMemo(
    () =>
      [...(snapshot?.resets ?? [])].sort(
        (a, b) => new Date(a.expiresAt).getTime() - new Date(b.expiresAt).getTime()
      ),
    [snapshot?.resets]
  );
  const tokenUsed =
    snapshot && snapshot.tokenUsage.total !== null && snapshot.tokenUsage.total !== undefined
      ? snapshot.tokenUsage.total
      : snapshot && (snapshot.tokenUsage.input !== null || snapshot.tokenUsage.output !== null)
        ? (snapshot.tokenUsage.input ?? 0) + (snapshot.tokenUsage.output ?? 0)
        : null;
  const resetCount = snapshot?.resetCredits?.available ?? sortedResets.length;
  const canExpand = sortedResets.length > 0;
  const statusClass = codexConnected ? "is-connected" : loading ? "is-loading" : "is-offline";
  const statusTitle = codexConnected ? "Codex 已连接" : loading ? "正在连接 Codex" : "Codex 未连接";

  function expandedExtraHeight(resetOpen: boolean, costOpen: boolean) {
    const resetExtra = resetOpen && canExpand
      ? resetDetailBaseExtraHeight + resetDetailRowExtraHeight * Math.min(sortedResets.length, 12)
      : 0;
    return resetExtra + (costOpen ? apiCostPanelExtraHeight : 0);
  }

  if (currentWindowLabel === "dock") {
    return <DockBar loading={loading} snapshot={snapshot} windows={windows} />;
  }

  async function toggleExpanded() {
    if (!canExpand) {
      return;
    }

    const next = !expanded;
    setExpanded(next);
    await desktopApi.setExpanded(next || tokenExpanded, expandedExtraHeight(next, tokenExpanded));
  }

  async function toggleTokenExpanded() {
    const next = !tokenExpanded;
    setTokenExpanded(next);
    await desktopApi.setExpanded(expanded || next, expandedExtraHeight(expanded, next));
  }

  return (
    <main
      className={expanded ? "shell is-expanded" : "shell"}
      onMouseEnter={() => setHoverRegion("main", true)}
      onMouseLeave={() => setHoverRegion("main", false)}
    >
      <section className={canExpand ? "widget has-reset-details" : "widget"} aria-label="Codex 用量组件">
        <div className="chrome drag-region" data-tauri-drag-region>
          <div className="brand">
            <span className={`status-dot ${statusClass}`} title={statusTitle} />
            <span>CODEX 用量核心</span>
          </div>
          <div className="plan-meta">
            {snapshot?.subscriptionExpiresAt && (
              <span className="subscription-inline">到期 {formatDate(snapshot.subscriptionExpiresAt)}</span>
            )}
            <span className="plan-badge">{compactPlanName(snapshot?.planName)}</span>
          </div>
        </div>

        {snapshot ? (
          <>
            <div className="quota-grid">
              {windows.map((window) => (
                <UsageCard key={window.id} window={window} />
              ))}
            </div>

            <TokenBlock expanded={tokenExpanded} onToggle={toggleTokenExpanded} snapshot={snapshot} tokenUsed={tokenUsed} />

            {snapshot.subscriptionExpiresAt && (
              <div className="subscription-strip">
                <span>订阅到期</span>
                <strong>{formatDate(snapshot.subscriptionExpiresAt)}</strong>
              </div>
            )}

            {canExpand && (
              <ResetOverview
                resets={sortedResets}
                availableCount={resetCount}
                expanded={expanded}
                onToggle={toggleExpanded}
              />
            )}

            {expanded && sortedResets.length > 0 && (
              <ResetDetails resets={sortedResets} availableCount={resetCount} updatedAt={snapshot.updatedAt} />
            )}
          </>
        ) : (
          <div className="error-panel">{error || "正在读取数据"}</div>
        )}

        {loading && <div className="loading-sheen" />}
      </section>
    </main>
  );
}

function DockBar({
  loading,
  snapshot,
  windows
}: {
  loading: boolean;
  snapshot: UsageSnapshot | null;
  windows: UsageWindow[];
}) {
  const fiveHour = windows.find((window) => window.id === "fiveHour");
  const sevenDay = windows.find((window) => window.id === "sevenDay");
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
      <section className="dock-widget" aria-label="Codex 用量悬浮条">
        <img className={loading ? "dock-icon is-loading" : "dock-icon"} src={appIcon} alt="" />
        <div className="dock-meters">
          <DockMeter window={fiveHour} label="5 小时" resetText={fiveHour ? formatClock(fiveHour.resetAt) : "--"} />
          <DockMeter window={sevenDay} label="每周" resetText={sevenDay ? formatResetDistance(sevenDay.resetAt) : "--"} />
        </div>
      </section>
    </main>
  );
}

function DockMeter({
  window,
  label,
  resetText
}: {
  window: UsageWindow | undefined;
  label: string;
  resetText: string;
}) {
  const remainingPercent = window ? Math.round(ratio(remainingOf(window), window.total)) : 0;

  return (
    <div className="dock-meter">
      <div className="dock-track">
        <span style={{ width: `${remainingPercent}%` }} />
      </div>
      <strong>{label}</strong>
      <span>
        {window ? `${remainingPercent}% · ${resetText}` : "--"}
      </span>
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
        <span className="reset-overview-icon">{expanded ? <ChevronUp size={16} /> : <ChevronDown size={16} />}</span>
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

function TokenBlock({
  expanded,
  onToggle,
  snapshot,
  tokenUsed
}: {
  expanded: boolean;
  onToggle: () => void;
  snapshot: UsageSnapshot;
  tokenUsed: number | null;
}) {
  const [period, setPeriod] = useState<ApiCostPeriod>("thisWeek");
  const hasSplit = snapshot.tokenUsage.input !== null || snapshot.tokenUsage.output !== null;
  const tokenPeriods = snapshot.tokenPeriods ?? [];
  const selectedPeriod = tokenPeriods.find((item) => item.id === period) ?? tokenPeriods[0];
  const currentCost = selectedPeriod
    ? apiEquivalentCostFromUsage(selectedPeriod.tokenUsage)
    : apiEquivalentCost(snapshot);

  return (
    <div className="token-section">
      <div className="token-summary-grid">
        <button className={expanded ? "token-block is-expanded" : "token-block"} type="button" onClick={onToggle}>
          <div className="token-title">
            <Database size={16} />
            <span>Token</span>
          </div>
          <div className="token-total">{formatToken(tokenUsed)}</div>
          {hasSplit && (
            <div className="token-metrics">
              <span>
                <small>输入</small>
                <strong>{formatToken(snapshot.tokenUsage.input)}</strong>
              </span>
              <span>
                <small>输出</small>
                <strong>{formatToken(snapshot.tokenUsage.output)}</strong>
              </span>
              {snapshot.tokenUsage.limit !== null && (
                <span>
                  <small>上限</small>
                  <strong>{formatToken(snapshot.tokenUsage.limit)}</strong>
                </span>
              )}
            </div>
          )}
        </button>
        <button className={expanded ? "api-summary-card is-expanded" : "api-summary-card"} type="button" onClick={onToggle}>
          <div className="api-summary-head">
            <span>API 等价成本</span>
            <span className="token-chevron">{expanded ? <ChevronUp size={15} /> : <ChevronDown size={15} />}</span>
          </div>
          <strong>{moneyFormatter.format(currentCost.estimated)}</strong>
          <small>{selectedPeriod?.label ?? "本周"}</small>
        </button>
      </div>
      {expanded && (
        <ApiCostPanel
          cost={currentCost}
          period={period}
          periodUsage={selectedPeriod}
          snapshot={snapshot}
          onPeriodChange={setPeriod}
        />
      )}
    </div>
  );
}
function ApiCostPanel({
  cost,
  period,
  periodUsage,
  snapshot,
  onPeriodChange
}: {
  cost: ReturnType<typeof apiEquivalentCostFromUsage>;
  period: ApiCostPeriod;
  periodUsage: TokenPeriodUsage | undefined;
  snapshot: UsageSnapshot;
  onPeriodChange: (period: ApiCostPeriod) => void;
}) {
  const range = periodUsage?.rangeLabel;
  const computedAt = periodUsage?.computedAt ?? snapshot.updatedAt;
  const sourceText = periodUsage ? "本地会话" : "当前快照";
  const periodStart = periodStartLabel(range);

  return (
    <section className="api-cost-panel" aria-label="API 等价成本">
      <div className="api-period-tabs" role="tablist" aria-label="成本周期">
        {apiCostPeriods.map((item) => (
          <button
            aria-selected={period === item.id}
            className={period === item.id ? "is-active" : ""}
            key={item.id}
            onClick={() => onPeriodChange(item.id)}
            role="tab"
            type="button"
          >
            {item.label}
          </button>
        ))}
      </div>
      <div className="api-cost-head">
        <div>
          <span>API 等价成本</span>
          <small>
            计算于 {formatFullDate(computedAt)}{periodStart ? ` · ${periodStart}` : ""} · {sourceText}
          </small>
        </div>
        <strong>{moneyFormatter.format(cost.estimated)}</strong>
      </div>
      <div className="api-cost-grid">
        <span>
          <small>Tokens</small>
          <strong>{tokenText(cost.total)}</strong>
        </span>
        <span>
          <small>非缓存输入</small>
          <strong>{tokenText(cost.uncached)}</strong>
        </span>
        <span>
          <small>缓存输入</small>
          <strong>{tokenText(cost.cached)}</strong>
        </span>
        <span>
          <small>输出 tokens</small>
          <strong>{tokenText(cost.output)}</strong>
        </span>
      </div>
    </section>
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
        <span>重置 {formatDate(window.resetAt)}</span>
      </div>
      <div className="meter" aria-label={`${window.label} 已用 ${Math.round(usedPercent)}%`}>
        <span style={{ width: `${usedPercent}%` }} />
      </div>
    </article>
  );
}
