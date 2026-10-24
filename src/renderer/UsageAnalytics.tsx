import { useState } from "react";
import { ChevronDown, ChevronUp, Database } from "lucide-react";
import type { CycleSummary, UsageSnapshot } from "../shared/types";

const tokens = new Intl.NumberFormat("zh-CN", { notation: "compact", maximumFractionDigits: 2 });
const dollars = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 4 });
const date = new Intl.DateTimeFormat("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
function cost(low: number | null | undefined, high: number | null | undefined) {
  if (low == null || high == null) return "--";
  return Math.abs(low - high) < 0.00005 ? dollars.format(low) : `${dollars.format(low)}–${dollars.format(high)}`;
}
function count(value: number | null | undefined) { return value == null ? "--" : tokens.format(value); }
function range(cycle: CycleSummary | null | undefined) {
  return cycle ? `${date.format(new Date(cycle.startAt))} — ${date.format(new Date(cycle.endAt))}` : "尚无历史窗口";
}

export function UsageAnalytics({ snapshot, expanded, onToggle }: { snapshot: UsageSnapshot; expanded: boolean; onToggle: () => void }) {
  const [windowId, setWindowId] = useState<string | null>(null);
  const comparisons = snapshot.cycleComparisons ?? [];
  const selected = comparisons.find((item) => item.windowId === windowId) ?? comparisons[comparisons.length - 1];
  const current = selected?.current;
  const previous = selected?.previous;
  const cycleTokenCount = current && current.sampleCount > 0 ? current.tokenUsage.total : null;
  const rows: Array<{label: string; value: (cycle: CycleSummary) => string}> = [
    { label: "Token 总量", value: (cycle) => cycle.sampleCount > 0 ? count(cycle.tokenUsage.total) : "未采集" },
    { label: "输入 / 缓存读取", value: (cycle) => cycle.sampleCount > 0 ? `${count(cycle.tokenUsage.input)} / ${count(cycle.tokenUsage.cachedInput)}` : "--" },
    { label: "输出", value: (cycle) => cycle.sampleCount > 0 ? count(cycle.tokenUsage.output) : "--" },
    { label: "缓存写入", value: (cycle) => cycle.sampleCount > 0 ? count(cycle.cacheWriteTokens) : "--" },
    { label: "API 等价成本", value: (cycle) => cost(cycle.costLowUsd, cycle.costHighUsd) },
    { label: "已观察额度消耗", value: (cycle) => cycle.observedFrom && cycle.observedFrom !== cycle.observedUntil ? `${cycle.observedPercent.toFixed(1)}%` : "采样不足" },
    { label: "每 1% 的 Token", value: (cycle) => count(cycle.tokensPerPercent) },
    { label: "每 1% 的等价成本", value: (cycle) => cost(cycle.costPerPercentLow, cycle.costPerPercentHigh) },
    { label: "额度回落记录", value: (cycle) => `${cycle.quotaDrops} 次` }
  ];
  return <div className="token-section">
    <div className="token-summary-grid">
      <button className={expanded ? "token-block is-expanded" : "token-block"} onClick={onToggle} aria-expanded={expanded}>
        <div className="token-title"><Database size={16} /><span>本周期 Token</span></div>
        <div className="token-total">{count(cycleTokenCount)}</div>
        <div className="token-metrics"><span><small>输入</small><strong>{cycleTokenCount == null ? "--" : count(current?.tokenUsage.input)}</strong></span><span><small>输出</small><strong>{cycleTokenCount == null ? "--" : count(current?.tokenUsage.output)}</strong></span></div>
      </button>
      <button className={expanded ? "api-summary-card is-expanded" : "api-summary-card"} onClick={onToggle} aria-expanded={expanded}>
        <div className="api-summary-head"><span>API 等价成本</span>{expanded ? <ChevronUp size={15} /> : <ChevronDown size={15} />}</div>
        <strong>{cost(current?.costLowUsd, current?.costHighUsd)}</strong>
        <small>{selected?.label ?? "等待周期数据"} · 点击对比</small>
      </button>
    </div>
    {expanded && <section className="cycle-panel" aria-label="实际额度周期对比">
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
        {(current?.unknownPriceModels.length ?? 0) > 0 && <p className="price-gap">未定价模型：{current?.unknownPriceModels.join("、")}。成本总额不完整，显示为 --。</p>}
        {(previous?.unknownPriceModels.length ?? 0) > 0 && <p className="price-gap">上周期未定价模型：{previous?.unknownPriceModels.join("、")}。</p>}
        <p>Token 仅统计本机已记录的请求；未采集的历史不补零。Claude 从插件接入后记录，Codex 可读取现存会话日志。</p>
        <p>按实际重置时间划分窗口。读数取整未增长时先累计请求，增长后再计入每 1%；尾部未反映的请求暂不计入。额度回落原因未确认，回落区间不参与该指标。</p>
        <p>总量受周期进度影响，优先比较每 1% 的 Token 与成本，并核对模型及缓存比例。跨设备、网页用量和采样缺口都会影响结果，不能仅凭此认定平台减额。</p>
        <p>按 2026-10-24 官方标准短上下文单价统一估算，不含工具等费用及速度、地域溢价。缓存写入未提供 TTL 时显示价格区间；这不是实际账单。</p>
      </div>
    </section>}
  </div>;
}
