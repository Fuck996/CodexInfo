import { useEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import { ChevronDown, ChevronUp, Database, X } from "lucide-react";
import type { CycleSummary, ProviderComparison, ProviderPeriod, UsageSnapshot } from "../shared/types";
import { desktopApi } from "./desktopApi";

type View = "cycle" | "providers";
type Metric = "quota" | "tokens" | "cost" | "efficiency";
type ChartItem = { label: string; value: number | null; upper: number | null; text: string; note?: string };
const tokens = new Intl.NumberFormat("zh-CN", { notation: "compact", maximumFractionDigits: 2 });
const dollars = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 2 });
const date = new Intl.DateTimeFormat("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
const metricOrder: Metric[] = ["quota", "tokens", "cost", "efficiency"];
const metrics: Record<Metric, { label: string; title: string; unit: string }> = {
  quota: { label: "额度消耗", title: "额度消耗", unit: "百分点" },
  tokens: { label: "Token", title: "Token 总量", unit: "本机记录" },
  cost: { label: "API 成本", title: "API 等价成本", unit: "美元 · 估算" },
  efficiency: { label: "单位耗额", title: "每百万 Token 耗额", unit: "百分点 · 越低越省" }
};

function count(value: number | null | undefined) { return value == null ? "--" : tokens.format(value); }
function cost(low: number | null | undefined, high: number | null | undefined) {
  if (low == null || high == null) return "--";
  const first = dollars.format(low);
  const last = dollars.format(high);
  return first === last ? first : `${first}–${last}`;
}
function cycleCost(cycle: CycleSummary | null | undefined) {
  if (!cycle || cycle.sampleCount === 0) return "--";
  if (cycle.costLowUsd != null) return cost(cycle.costLowUsd, cycle.costHighUsd);
  return (cycle.pricedSampleCount ?? 0) > 0 ? `${cost(cycle.pricedCostLowUsd, cycle.pricedCostHighUsd)}（部分）` : "--";
}
function range(cycle: CycleSummary | null | undefined) {
  return cycle ? `${date.format(new Date(cycle.startAt))} — ${date.format(new Date(cycle.endAt))}` : "暂无记录";
}

export function UsageAnalytics({ snapshot, view, onToggle }: {
  snapshot: UsageSnapshot; view: View | null; onToggle: () => void;
}) {
  const comparisons = snapshot.cycleComparisons ?? [];
  const current = comparisons[comparisons.length - 1]?.current;
  const total = current && current.sampleCount > 0 ? current.tokenUsage.total : null;
  return <div className="token-section"><div className="token-summary-grid">
    <button className={view === "cycle" ? "token-block is-expanded" : "token-block"} onClick={onToggle} aria-expanded={view === "cycle"}>
      <div className="token-title"><Database size={16} /><span>本周期 Token</span></div>
      <div className="token-total">{count(total)}</div>
      <div className="token-metrics"><span><small>输入</small><strong>{total == null ? "--" : count(current?.tokenUsage.input)}</strong></span><span><small>输出</small><strong>{total == null ? "--" : count(current?.tokenUsage.output)}</strong></span></div>
    </button>
    <button className={view === "cycle" ? "api-summary-card is-expanded" : "api-summary-card"} onClick={onToggle} aria-expanded={view === "cycle"}>
      <div className="api-summary-head"><span>API 等价成本</span>{view === "cycle" ? <ChevronUp size={15} /> : <ChevronDown size={15} />}</div>
      <strong className={current?.unknownPriceModels.length ? "has-price-gap" : ""} title={cycleCost(current)}>{cycleCost(current)}</strong>
      <small>{current?.unknownPriceModels.length ? "部分请求尚无公开单价" : "点击查看周期图表"}</small>
    </button>
  </div></div>;
}

export function UsageComparison({ snapshot, view, onViewChange, contentRef }: {
  snapshot: UsageSnapshot | null; view: View | null; onViewChange: (view: View | null) => void;
  contentRef: RefObject<HTMLDivElement | null>;
}) {
  const closeButton = useRef<HTMLButtonElement>(null);
  const open = view !== null;
  useEffect(() => {
    if (!open) return;
    const previousFocus = document.activeElement;
    closeButton.current?.focus();
    return () => { if (previousFocus instanceof HTMLElement) previousFocus.focus(); };
  }, [open]);
  if (!view) return null;
  return <div className="comparison-scrim" onClick={() => onViewChange(null)}>
  <section id="usage-comparison-panel" className="cycle-panel" role="dialog" aria-modal="true" aria-labelledby="comparison-title"
    onClick={(event) => event.stopPropagation()} onKeyDown={(event) => {
      if (event.key === "Escape") onViewChange(null);
      if (event.key === "Tab") {
        const controls = event.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled), summary");
        const first = controls[0];
        const last = controls[controls.length - 1];
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
      }
    }}><div ref={contentRef} className="comparison-content">
    <header className="comparison-head"><strong id="comparison-title">用量对比</strong><button ref={closeButton} onClick={() => onViewChange(null)} aria-label="关闭用量对比"><X size={18} /></button></header>
    <div className="analytics-view-tabs" role="group" aria-label="对比方式">
      <button aria-pressed={view === "cycle"} onClick={() => onViewChange("cycle")}>本周期 / 上周期</button>
      <button aria-pressed={view === "providers"} onClick={() => onViewChange("providers")}>Codex / Claude</button>
    </div>
    {view === "providers" ? <CrossProviderComparison /> : <CycleChart snapshot={snapshot} />}
  </div></section></div>;
}

function item(label: string, cycle: CycleSummary | null | undefined, metric: Metric): ChartItem {
  if (!cycle) return { label, value: null, upper: null, text: "暂无记录" };
  let value: number | null = null;
  let upper: number | null = null;
  let text = "未记录";
  let note: string | undefined;
  if (metric === "quota") {
    const observed = cycle.observedFrom && cycle.observedFrom !== cycle.observedUntil;
    if (observed && cycle.quotaDrops === 0) { value = cycle.observedPercent; text = `${value.toFixed(1)}%`; }
    else text = cycle.quotaDrops ? "有额度回落" : "采样不足";
  } else if (metric === "tokens") {
    if (cycle.sampleCount > 0) { value = cycle.tokenUsage.total ?? null; text = count(value); }
    else text = "无 Token 记录";
  } else if (metric === "cost") {
    if (cycle.sampleCount > 0) {
      value = cycle.costLowUsd ?? cycle.pricedCostLowUsd ?? null;
      upper = cycle.costHighUsd ?? cycle.pricedCostHighUsd ?? null;
      text = cost(value, upper);
      if (cycle.costLowUsd == null && value != null) note = "部分";
    }
  } else {
    value = cycle.tokensPerPercent ? 1_000_000 / cycle.tokensPerPercent : null;
    text = value == null ? "无法计算" : `${value.toFixed(2)}%`;
  }
  return { label, value, upper: upper ?? value, text, note };
}

function MetricTabs({ metric, onChange }: { metric: Metric; onChange: (metric: Metric) => void }) {
  return <div className="metric-tabs" role="group" aria-label="图表指标">{metricOrder.map((id) =>
    <button key={id} aria-pressed={metric === id} onClick={() => onChange(id)}>{metrics[id].label}</button>)}</div>;
}

function ComparisonBars({ metric, items }: { metric: Metric; items: ChartItem[] }) {
  const definition = metrics[metric];
  const max = Math.max(0, ...items.map((entry) => entry.upper ?? 0));
  const [first, second] = items;
  const a = first.value;
  const b = second.value;
  let verdict = "";
  if (a != null && b != null) {
    if (a === b) verdict = "两者相同";
    else {
      const higher = a > b ? first.label : second.label;
      const lower = Math.min(a, b);
      verdict = lower === 0 ? `${higher}消耗更多` : `${higher}为另一方的 ${(Math.max(a, b) / lower).toFixed(2)} 倍`;
    }
  }
  return <figure className="comparison-chart" aria-label={`${definition.title}对比柱状图`}>
    <figcaption><strong>{definition.title}</strong><span>{definition.unit}</span></figcaption>
    {items.map((entry, index) => <div className="chart-item" data-series={index === 0 ? "first" : "second"} key={entry.label}>
      <div className="chart-value"><span>{entry.label}</span><strong>{entry.text}{entry.note && <small>{entry.note}</small>}</strong></div>
      <div className={entry.value == null ? "chart-track is-missing" : "chart-track"} aria-hidden="true">
        {entry.value != null && max > 0 && <>
          <span className="chart-fill" style={{ width: `${entry.value / max * 100}%` }} />
          {entry.upper != null && entry.upper > entry.value && <span className="chart-range" style={{ left: `${entry.value / max * 100}%`, width: `${(entry.upper - entry.value) / max * 100}%` }} />}
        </>}
      </div>
    </div>)}
    {verdict && <p className="chart-verdict">{verdict}</p>}
  </figure>;
}

function DetailTable({ names, cycles }: { names: string[]; cycles: Array<CycleSummary | null | undefined> }) {
  const rows: Array<{ label: string; value: (cycle: CycleSummary) => string }> = [
    { label: "周期", value: range },
    { label: "Token 总量", value: (cycle) => cycle.sampleCount ? count(cycle.tokenUsage.total) : "--" },
    { label: "输入 / 输出", value: (cycle) => cycle.sampleCount ? `${count(cycle.tokenUsage.input)} / ${count(cycle.tokenUsage.output)}` : "--" },
    { label: "缓存读取 / 写入", value: (cycle) => cycle.sampleCount ? `${count(cycle.tokenUsage.cachedInput)} / ${count(cycle.cacheWriteTokens)}` : "--" },
    { label: "API 成本", value: cycleCost },
    { label: "每 1% 的 Token", value: (cycle) => count(cycle.tokensPerPercent) },
    { label: "每 1% 的 API 成本", value: (cycle) => cost(cycle.costPerPercentLow, cycle.costPerPercentHigh) },
    { label: "已观察额度消耗", value: (cycle) => cycle.observedFrom && cycle.observedFrom !== cycle.observedUntil ? `${cycle.observedPercent.toFixed(1)}%` : "采样不足" },
    { label: "额度回落", value: (cycle) => `${cycle.quotaDrops} 次` },
    { label: "模型", value: (cycle) => cycle.models.join("、") || "--" }
  ];
  return <table><thead><tr><th>明细</th>{names.map((name) => <th key={name}>{name}</th>)}</tr></thead><tbody>
    {rows.map((row) => <tr key={row.label}><th>{row.label}</th>{cycles.map((cycle, index) => <td key={index}>{cycle ? row.value(cycle) : "--"}</td>)}</tr>)}
  </tbody></table>;
}

function CycleChart({ snapshot }: { snapshot: UsageSnapshot | null }) {
  const [windowId, setWindowId] = useState<string | null>(null);
  const [metric, setMetric] = useState<Metric>("tokens");
  const comparisons = snapshot?.cycleComparisons ?? [];
  const selected = comparisons.find((entry) => entry.windowId === windowId) ?? comparisons[comparisons.length - 1];
  const cycles = [selected?.current, selected?.previous];
  return <div className="analytics-body">
    <div className="analytics-window-tabs" role="group" aria-label="额度窗口">{comparisons.map((entry) =>
      <button key={entry.windowId} aria-pressed={entry === selected} onClick={() => setWindowId(entry.windowId)}>{entry.label}</button>)}</div>
    <MetricTabs metric={metric} onChange={setMetric} />
    <ComparisonBars metric={metric} items={[item("本周期·进行中", cycles[0], metric), item("上周期", cycles[1], metric)]} />
    <details className="analytics-details"><summary>统计说明与明细</summary>
      <DetailTable names={["本周期", "上周期"]} cycles={cycles} />
      <p>进行中的本周期与完整上周期时长不同。单位耗额只使用已匹配到额度增长的请求。</p>
      <p>API 成本按官方标准短上下文单价估算；“部分”不含未定价请求，浅色条表示缓存写入的估算区间，不是实际账单。</p>
      {cycles.map((cycle, index) => cycle?.unknownPriceModels.length ? <p key={index}>未定价：{cycle.unknownPriceModels.join("、")}</p> : null)}
    </details>
  </div>;
}

function CrossProviderComparison() {
  const [data, setData] = useState<ProviderComparison | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    let inFlight = false;
    async function load() {
      if (inFlight) return;
      inFlight = true;
      try {
        const result = await desktopApi.getUsageComparison();
        if (active) { setData(result); setError(""); }
      } catch (error) { if (active) { setData(null); setError(String(error)); } }
      finally { inFlight = false; }
    }
    void load();
    const timer = window.setInterval(load, 30_000);
    return () => { active = false; window.clearInterval(timer); };
  }, []);
  if (error) return <div className="comparison-state" role="status"><strong>暂时无法对比</strong><p>{error}</p></div>;
  if (!data) return <div className="comparison-state" role="status">正在对齐两边读数…</div>;
  return <QuotaComparison data={data} />;
}

export function QuotaComparison({ data }: { data: ProviderComparison }) {
  const periods = [data.codex, data.claude];
  const names = ["Codex", "Claude"];
  const hours = (Date.parse(data.endAt) - Date.parse(data.startAt)) / 3_600_000;
  const duration = hours >= 1 ? `${hours.toFixed(1)} 小时` : hours >= 1 / 60 ? `${Math.round(hours * 60)} 分钟` : `${Math.round(hours * 3600)} 秒`;
  const a = data.codex.consumedPercent;
  const b = data.claude.consumedPercent;
  let verdict = "额度有回落，本段暂不判断高低";
  if (a != null && b != null) {
    const difference = Math.abs(a - b);
    verdict = difference < 1e-9 ? "本段周额度消耗相同"
      : `${a > b ? "Codex" : "Claude"}多消耗 ${difference.toFixed(1)} 个百分点`;
  }
  return <div className="analytics-body">
    <p className="comparison-period">{date.format(new Date(data.startAt))} — {date.format(new Date(data.endAt))} · {duration}</p>
    <div className="quota-totals" aria-label="本段周额度消耗">{periods.map((period, index) =>
      <div className="quota-provider" data-series={index} key={names[index]}>
        <span className="quota-provider-name">{names[index]}</span>
        <strong>{period.consumedPercent == null ? "有回落" : period.consumedPercent.toFixed(1)}{period.consumedPercent != null && <small>个百分点</small>}</strong>
        <span>{period.percentPerHour == null ? "本段不比较" : `平均 ${period.percentPerHour.toFixed(2)} / 小时`}</span>
      </div>)}</div>
    <p className="quota-verdict" role="status">{verdict}</p>
    <QuotaTrend data={data} />
    <p className="quota-scope">比较真实账号耗额，包含 Projects 和普通聊天。</p>
    <details className="analytics-details"><summary>统计说明与明细</summary>
      <table><thead><tr><th>周额度</th>{names.map((name) => <th key={name}>{name}</th>)}</tr></thead><tbody>
        <tr><th>套餐</th>{periods.map((period, index) => <td key={index}>{period.planName ?? "暂未记录"}</td>)}</tr>
        <tr><th>起点已用</th>{periods.map((period, index) => <td key={index}>{period.usedFromPercent.toFixed(1)}%</td>)}</tr>
        <tr><th>终点已用</th>{periods.map((period, index) => <td key={index}>{period.usedUntilPercent.toFixed(1)}%</td>)}</tr>
        <tr><th>有效采样</th>{periods.map((period, index) => <td key={index}>{period.trend.length} 次</td>)}</tr>
        <tr><th>端点偏差</th>{periods.map((period, index) => <td key={index}>≤ {period.endpointSkewSeconds} 秒</td>)}</tr>
        <tr><th>额度回落</th>{periods.map((period, index) => <td key={index}>{period.quotaDrops} 次</td>)}</tr>
      </tbody></table>
      <p>对比使用本地已记录的额度。两边使用同一时间段，端点采用此前最近且不超过 3 分钟的读数，不跨周重置。平均每小时耗额为本段周额度涨幅除以区间小时数。</p>
      <p>折线从各自起点归零。阶梯表示实际采样，虚线连接超过 3 分钟的采样间断，不推测间断内的消耗时刻。额度回落时保留原始起止读数，本段不排名。</p>
      <p>此处不依赖 Token，也不估算普通聊天的 Token。消耗高低反映各自实际使用情况，不代表相同任务的模型效率或套餐总容量。单个平台的 Token 和 API 成本仍在「本周期 / 上周期」中查看。</p>
    </details>
  </div>;
}

function quotaPaths(period: ProviderPeriod, start: number, end: number, max: number) {
  const x = (at: string) => 44 + Math.max(0, Date.parse(at) - start) / (end - start) * 340;
  const y = (used: number) => 174 - (used - period.usedFromPercent) / max * 144;
  let solid = "";
  let gaps = "";
  for (let i = 0; i < period.trend.length; i++) {
    const point = period.trend[i];
    const previous = period.trend[i - 1];
    if (!previous) solid = `M${x(point.at)},${y(point.usedPercent)}`;
    else if (Date.parse(point.at) - Date.parse(previous.at) > 180_000) {
      gaps += `M${x(previous.at)},${y(previous.usedPercent)} L${x(point.at)},${y(point.usedPercent)}`;
      solid += ` M${x(point.at)},${y(point.usedPercent)}`;
    } else solid += ` H${x(point.at)} V${y(point.usedPercent)}`;
  }
  return { solid, gaps, endpointX: x(period.quotaUntil) };
}

function QuotaTrend({ data }: { data: ProviderComparison }) {
  const periods = [data.codex, data.claude];
  const names = ["Codex", "Claude"];
  const start = Date.parse(data.startAt);
  const end = Date.parse(data.endAt);
  const max = Math.max(1, ...periods.map((period) => period.consumedPercent ?? 0));
  const clock = new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
  return <figure className="comparison-chart quota-trend">
    <figcaption><strong>累计消耗走势</strong><span>周额度百分点</span></figcaption>
    <div className="quota-legend" aria-label="折线图图例">{names.map((name, index) => <span key={name} data-series={index}><i />{name}</span>)}</div>
    <svg viewBox="0 0 400 210" role="img" aria-label="Codex 与 Claude 同一时段累计周额度消耗折线图">
      <desc>两条线从各自起点归零，越高表示本段消耗越多。主卡片显示最终消耗和平均速度，统计说明中提供原始起止读数。</desc>
      {[0, max / 2, max].map((tick) => <g key={tick}><line className="quota-grid" x1="44" x2="384" y1={174 - tick / max * 144} y2={174 - tick / max * 144} /><text x="36" y={178 - tick / max * 144} textAnchor="end">{tick.toFixed(1)}</text></g>)}
      <text x="44" y="201">{clock.format(new Date(start))}</text><text x="384" y="201" textAnchor="end">{clock.format(new Date(end))}</text>
      {periods.map((period, index) => {
        if (period.consumedPercent == null) return null;
        const paths = quotaPaths(period, start, end, max);
        return <g className="quota-series" data-series={index} key={names[index]}>
          <path d={paths.solid} /><path className="quota-gap" d={paths.gaps} />
          <circle cx={paths.endpointX} cy={174 - period.consumedPercent / max * 144} r={index === 0 ? 4 : 6} tabIndex={0} aria-label={`${names[index]}本段消耗 ${period.consumedPercent.toFixed(1)} 个百分点`}><title>{`${names[index]}：${period.consumedPercent.toFixed(1)} 个百分点`}</title></circle>
        </g>;
      })}
    </svg>
  </figure>;
}
