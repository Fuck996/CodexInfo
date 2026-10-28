import { useEffect, useState } from "react";
import { ChevronDown, ChevronUp, Database } from "lucide-react";
import type { CycleSummary, ProviderComparison, ProviderPeriod, UsageSnapshot } from "../shared/types";
import { desktopApi } from "./desktopApi";

const tokens = new Intl.NumberFormat("zh-CN", { notation: "compact", maximumFractionDigits: 2 });
const dollars = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 4 });
const date = new Intl.DateTimeFormat("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
function cost(low: number | null | undefined, high: number | null | undefined) {
  if (low == null || high == null) return "--";
  return Math.abs(low - high) < 0.00005 ? dollars.format(low) : `${dollars.format(low)}–${dollars.format(high)}`;
}
function count(value: number | null | undefined) { return value == null ? "--" : tokens.format(value); }
function cycleCost(cycle: CycleSummary | null | undefined) {
  if (!cycle) return "--";
  if (cycle.costLowUsd != null) return cost(cycle.costLowUsd, cycle.costHighUsd);
  return (cycle.pricedSampleCount ?? 0) > 0 ? `${cost(cycle.pricedCostLowUsd, cycle.pricedCostHighUsd)}（部分）` : "--";
}
function range(cycle: CycleSummary | null | undefined) {
  return cycle ? `${date.format(new Date(cycle.startAt))} — ${date.format(new Date(cycle.endAt))}` : "尚无历史窗口";
}

export function UsageAnalytics({ snapshot, view, onViewChange, onToggle }: {
  snapshot: UsageSnapshot | null;
  view: "cycle" | "providers" | null;
  onViewChange: (view: "cycle" | "providers") => void;
  onToggle: () => void;
}) {
  const [windowId, setWindowId] = useState<string | null>(null);
  const expanded = view !== null;
  const crossProvider = view === "providers";
  const comparisons = snapshot?.cycleComparisons ?? [];
  const selected = comparisons.find((item) => item.windowId === windowId) ?? comparisons[comparisons.length - 1];
  const current = selected?.current;
  const previous = selected?.previous;
  const cycleTokenCount = current && current.sampleCount > 0 ? current.tokenUsage.total : null;
  const rows: Array<{label: string; value: (cycle: CycleSummary) => string}> = [
    { label: "Token 总量", value: (cycle) => cycle.sampleCount > 0 ? count(cycle.tokenUsage.total) : "未采集" },
    { label: "输入 / 缓存读取", value: (cycle) => cycle.sampleCount > 0 ? `${count(cycle.tokenUsage.input)} / ${count(cycle.tokenUsage.cachedInput)}` : "--" },
    { label: "输出", value: (cycle) => cycle.sampleCount > 0 ? count(cycle.tokenUsage.output) : "--" },
    { label: "缓存写入", value: (cycle) => cycle.sampleCount > 0 ? count(cycle.cacheWriteTokens) : "--" },
    { label: "API 等价成本", value: cycleCost },
    { label: "已观察额度消耗", value: (cycle) => cycle.observedFrom && cycle.observedFrom !== cycle.observedUntil ? `${cycle.observedPercent.toFixed(1)}%` : "采样不足" },
    { label: "每 1% 的 Token", value: (cycle) => count(cycle.tokensPerPercent) },
    { label: "每 1% 的等价成本", value: (cycle) => cost(cycle.costPerPercentLow, cycle.costPerPercentHigh) },
    { label: "额度回落记录", value: (cycle) => `${cycle.quotaDrops} 次` }
  ];
  return <div className="token-section">
    {snapshot && <div className="token-summary-grid">
      <button className={expanded ? "token-block is-expanded" : "token-block"} onClick={onToggle} aria-expanded={expanded}>
        <div className="token-title"><Database size={16} /><span>本周期 Token</span></div>
        <div className="token-total">{count(cycleTokenCount)}</div>
        <div className="token-metrics"><span><small>输入</small><strong>{cycleTokenCount == null ? "--" : count(current?.tokenUsage.input)}</strong></span><span><small>输出</small><strong>{cycleTokenCount == null ? "--" : count(current?.tokenUsage.output)}</strong></span></div>
      </button>
      <button className={expanded ? "api-summary-card is-expanded" : "api-summary-card"} onClick={onToggle} aria-expanded={expanded}>
        <div className="api-summary-head"><span>API 等价成本</span>{expanded ? <ChevronUp size={15} /> : <ChevronDown size={15} />}</div>
        <strong className={current?.unknownPriceModels.length ? "has-price-gap" : ""} title={cycleCost(current)}>{cycleCost(current)}</strong>
        <small>{current?.unknownPriceModels.length ? `完整总额未知 · ${current.pricedSampleCount ?? 0}/${current.sampleCount} 条已定价` : `${selected?.label ?? "等待周期数据"} · 点击对比`}</small>
      </button>
    </div>}
    {expanded && <section id="usage-comparison-panel" className="cycle-panel" aria-label="实际额度周期对比">
      <div className="analytics-view-tabs" role="group" aria-label="对比方式">
        <button aria-pressed={!crossProvider} onClick={() => onViewChange("cycle")}>本周期 / 上周期</button>
        <button aria-pressed={crossProvider} onClick={() => onViewChange("providers")}>Codex / Claude 同周期</button>
      </div>
      {crossProvider ? <CrossProviderComparison /> : <>
      <div className="api-period-tabs" role="group" aria-label="额度窗口">
        {comparisons.map((item) => <button key={item.windowId} aria-pressed={item === selected} className={item === selected ? "is-active" : ""} onClick={() => setWindowId(item.windowId)}>{item.label}</button>)}
      </div>
      <div className="cycle-scroll">
        <table><thead><tr><th>实际额度周期</th><th>本周期（进行中）</th><th>上周期</th></tr></thead>
          <tbody><tr><th>起止时间</th><td>{range(current)}</td><td>{range(previous)}</td></tr>
            {rows.map((row) => <tr key={row.label}><th>{row.label}</th><td>{current ? row.value(current) : "--"}</td><td>{previous ? row.value(previous) : "--"}</td></tr>)}
            <tr><th>模型</th><td>{current?.models.join("、") || "--"}</td><td>{previous?.models.join("、") || "--"}</td></tr>
          </tbody></table>
        {current?.observedFrom && <p>本周期额度观察：{date.format(new Date(current.observedFrom))} — {date.format(new Date(current.observedUntil ?? current.observedFrom))}</p>}
        {(current?.unknownPriceModels.length ?? 0) > 0 && <p className="price-gap">未定价模型：{current?.unknownPriceModels.join("、")}。完整总额未知；“部分”仅为已定价请求的小计，未定价请求仍计入 Token。</p>}
        {(previous?.unknownPriceModels.length ?? 0) > 0 && <p className="price-gap">上周期未定价模型：{previous?.unknownPriceModels.join("、")}。</p>}
        <p>Token 仅统计本机已记录的请求；未采集的历史不补零。Claude 从插件接入后记录，Codex 可读取现存会话日志。</p>
        <p>按实际重置时间划分窗口。读数取整未增长时先累计请求，增长后再计入每 1%；尾部未反映的请求暂不计入。额度回落原因未确认，回落区间不参与该指标。</p>
        <p>总量受周期进度影响，优先比较每 1% 的 Token 与成本，并核对模型及缓存比例。跨设备、网页用量和采样缺口都会影响结果，不能仅凭此认定平台减额。</p>
        <p>按 2026-10-24 官方标准短上下文单价统一估算，不含工具等费用及速度、地域溢价。缓存写入未提供 TTL 时显示价格区间；这不是实际账单。</p>
      </div>
      </>}
    </section>}
  </div>;
}

function metricState(period: ProviderPeriod) {
  if (period.summary.sampleCount === 0) return "区间内无 Token 记录";
  if (period.summary.quotaDrops > 0) return "额度有回落，暂停比较";
  if (period.summary.observedPercent < 1) return "增长不足 1 个百分点";
  return period.percentPerMillionTokens == null ? "Token 数据不足" : `${period.percentPerMillionTokens.toFixed(2)} 个百分点`;
}

function relative(left: number | null | undefined, right: number | null | undefined, metric: string) {
  if (left == null || right == null || left <= 0 || right <= 0) return `${metric}：数据不足，暂不排名。`;
  const difference = Math.abs(left - right) / Math.min(left, right);
  if (difference < 0.01) return `${metric}：两者相近（差异不足 1%）。`;
  return `${metric}：${left > right ? "Codex" : "Claude"} 为另一方的 ${(Math.max(left, right) / Math.min(left, right)).toFixed(2)} 倍。`;
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
      } catch (error) {
        if (active) { setData(null); setError(String(error)); }
      } finally { inFlight = false; }
    }
    void load();
    const timer = window.setInterval(load, 30_000);
    return () => { active = false; window.clearInterval(timer); };
  }, []);
  if (error) return <div className="comparison-state" role="status"><strong>暂不能对比</strong><p>{error}</p><p>保持两边采集在线，数据更新后自动重算。</p></div>;
  if (!data) return <div className="comparison-state" role="status">正在对齐两边的周额度与请求记录…</div>;
  const periods = [data.codex, data.claude];
  const rows: Array<{ label: string; value: (period: ProviderPeriod) => string }> = [
    { label: "套餐", value: (period) => period.planName },
    { label: "Token 总量", value: ({ summary }) => summary.sampleCount ? count(summary.tokenUsage.total) : "区间内无记录" },
    { label: "输入 / 输出", value: ({ summary }) => summary.sampleCount ? `${count(summary.tokenUsage.input)} / ${count(summary.tokenUsage.output)}` : "--" },
    { label: "缓存读取占输入", value: ({ summary }) => summary.tokenUsage.input ? `${((summary.tokenUsage.cachedInput ?? 0) / summary.tokenUsage.input * 100).toFixed(1)}%` : "--" },
    { label: "API 等价成本", value: ({ summary }) => cycleCost(summary) },
    { label: "周额度增长", value: ({ summary }) => `${summary.observedPercent.toFixed(2)} 个百分点${summary.quotaDrops ? "（有回落）" : ""}` },
    { label: "每百万 Token 耗周额度 ↓", value: metricState },
    { label: "端点读数偏差", value: (period) => `≤ ${period.endpointSkewSeconds} 秒` },
    { label: "最近 Token 记录", value: (period) => period.lastRecordedAt ? date.format(new Date(period.lastRecordedAt)) : "从未收到" },
    { label: "模型", value: ({ summary }) => summary.models.join("、") || "--" }
  ];
  const gptTotal = data.codex.summary.sampleCount ? data.codex.summary.tokenUsage.total : null;
  const claudeTotal = data.claude.summary.sampleCount ? data.claude.summary.tokenUsage.total : null;
  const quotasComparable = periods.every((period) => period.summary.quotaDrops === 0 && period.summary.observedPercent >= 1);
  return <div className="cycle-scroll">
    <p className="comparison-range">共同时间：{range({ ...data.codex.summary, startAt: data.startAt, endAt: data.endAt })}</p>
    {periods.map((period, index) => period.summary.sampleCount === 0 && <div key={index} className="token-gap" role="status">
      <strong>{index === 0 ? "Codex" : "Claude"} 周额度有读数，当前区间没有 Token 记录</strong>
      {period.lastRecordedAt ? <p>本机已有 {period.recordedSampleCount} 条请求记录，最近一次：{date.format(new Date(period.lastRecordedAt))}。{new Date(period.lastRecordedAt) <= new Date(data.startAt) ? "最近记录早于上面的共同起点，未计入本轮对比。" : "已有记录均不在上面的共同时间内。"}</p> : <p>本机尚未收到请求 Token 记录。</p>}
      {index === 1 && <p>额度查询不会返回 Token 数；插件只记录已接入的 Code 请求。Projects / 普通聊天不经过这个插件，无法计入 Token。若正在使用 Code，请检查该会话是否加载了用量插件。</p>}
    </div>)}
    <div className="comparison-verdict" aria-label="用量对比结论">
      <strong>本机观测估计</strong>
      <p>{gptTotal == null || claudeTotal == null ? "Token 总量：当前区间缺少请求记录，无法判断谁更多。" : relative(gptTotal, claudeTotal, "Token 总量")}</p>
      <p>{quotasComparable ? relative(data.codex.summary.observedPercent, data.claude.summary.observedPercent, "周额度消耗") : "周额度消耗：采样不足或额度回落，暂不排名。"}</p>
      <p>{relative(data.codex.percentPerMillionTokens, data.claude.percentPerMillionTokens, "每百万 Token 耗额（越低越省）")}</p>
    </div>
    <table><thead><tr><th>同周期观测</th><th>Codex（GPT）</th><th>Claude</th></tr></thead><tbody>
      {rows.map((row) => <tr key={row.label}><th>{row.label}</th>{periods.map((period, index) => <td key={index}>{row.value(period)}</td>)}</tr>)}
    </tbody></table>
    {periods.map((period, index) => period.summary.unknownPriceModels.length > 0 && <p key={index} className="price-gap">{index === 0 ? "Codex" : "Claude"} 未定价：{period.summary.unknownPriceModels.join("、")}；“部分”为已定价小计。</p>)}
    <p>算法：取两边当前周额度的共同观测区间，Token 使用同一起止时间；周耗额累加区间内正向增长。每百万 Token 耗额 = 周额度增长 ÷ Token 总量 × 100 万。端点取此前最近读数，偏差超过 3 分钟不计算；增长不足 1 个百分点或存在回落不排名。</p>
    <p>这是观测估计：周额度覆盖账号各入口，Token 仅覆盖本机已采集请求。Claude 网页 / Projects、跨设备用量和漏采会使耗额比偏高；缓存比例、模型及两家分词器也不同，不能据此认定同等任务更省或平台减额。</p>
  </div>;
}
