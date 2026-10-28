use super::{
    empty_token_usage, merge_token_usage, TokenUsage, TokenUsageSample, UsageSnapshot, UsageWindow,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
};

#[derive(Default)]
pub(super) struct History {
    observations: HashMap<String, Vec<Observation>>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Observation {
    at: DateTime<Utc>,
    windows: Vec<UsageWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CycleComparison {
    window_id: String,
    label: String,
    current: CycleSummary,
    previous: Option<CycleSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CycleSummary {
    #[serde(default)]
    priced_sample_count: usize,
    #[serde(default)]
    priced_cost_low_usd: Option<f64>,
    #[serde(default)]
    priced_cost_high_usd: Option<f64>,
    cache_write_tokens: f64,
    start_at: String,
    end_at: String,
    token_usage: TokenUsage,
    cost_low_usd: Option<f64>,
    cost_high_usd: Option<f64>,
    models: Vec<String>,
    unknown_price_models: Vec<String>,
    sample_count: usize,
    observed_from: Option<String>,
    observed_until: Option<String>,
    observed_percent: f64,
    tokens_per_percent: Option<f64>,
    cost_per_percent_low: Option<f64>,
    cost_per_percent_high: Option<f64>,
    quota_drops: usize,
}

pub(super) fn enrich(
    root: &Path,
    history: &mut History,
    snapshot: &mut UsageSnapshot,
    samples: &[TokenUsageSample],
) -> Result<(), String> {
    let provider = snapshot.provider.label().to_lowercase();
    let path = root.join(format!("quota-history-{provider}.jsonl"));
    if !history.observations.contains_key(&provider) {
        let mut observations = Vec::new();
        if path.exists() {
            for line in
                BufReader::new(fs::File::open(&path).map_err(|error| error.to_string())?).lines()
            {
                observations.push(
                    serde_json::from_str(&line.map_err(|error| error.to_string())?)
                        .map_err(|error| format!("用量历史无法解析: {error}"))?,
                );
            }
        }
        history.observations.insert(provider.clone(), observations);
    }
    let observations = history
        .observations
        .get_mut(&provider)
        .expect("history initialized");
    let at = DateTime::parse_from_rfc3339(&snapshot.updated_at)
        .map_err(|error| error.to_string())?
        .with_timezone(&Utc);
    if observations.last().is_none_or(|previous| previous.at < at) {
        let observation = Observation {
            at,
            windows: snapshot.windows.clone(),
        };
        let serialized = serde_json::to_string(&observation).map_err(|error| error.to_string())?;
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .map_err(|error| error.to_string())?;
        writeln!(file, "{serialized}").map_err(|error| format!("记录额度历史失败: {error}"))?;
        observations.push(observation);
    }
    let mut points = observations.clone();
    for sample in samples {
        if sample.windows.is_empty() {
            continue;
        }
        let at = DateTime::parse_from_rfc3339(&sample.timestamp)
            .map_err(|error| error.to_string())?
            .with_timezone(&Utc);
        points.push(Observation {
            at,
            windows: sample.windows.clone(),
        });
    }
    points.sort_by_key(|point| point.at);
    let mut timed_samples = samples
        .iter()
        .map(|sample| {
            DateTime::parse_from_rfc3339(&sample.timestamp)
                .map(|at| (at.with_timezone(&Utc), sample))
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    timed_samples.sort_by_key(|(at, _)| *at);
    snapshot.cycle_comparisons = snapshot
        .windows
        .iter()
        .filter_map(|window| {
            let end = DateTime::parse_from_rfc3339(&window.reset_at)
                .ok()?
                .with_timezone(&Utc);
            if window.window_duration_mins == 0 {
                return None;
            }
            let duration = Duration::minutes(window.window_duration_mins as i64);
            let previous_end = points
                .iter()
                .flat_map(|point| point.windows.iter())
                .filter(|candidate| candidate.window_duration_mins == window.window_duration_mins)
                .filter_map(|candidate| {
                    DateTime::parse_from_rfc3339(&candidate.reset_at)
                        .ok()
                        .map(|time| time.with_timezone(&Utc))
                })
                .filter(|time| *time < end && !same_cycle_end(*time, end, &provider))
                .max();
            Some(CycleComparison {
                window_id: window.id.clone(),
                label: window.label.clone(),
                current: summarize(
                    end - duration,
                    end,
                    window.window_duration_mins,
                    &points,
                    &timed_samples,
                    &provider,
                ),
                previous: previous_end.map(|end| {
                    summarize(
                        end - duration,
                        end,
                        window.window_duration_mins,
                        &points,
                        &timed_samples,
                        &provider,
                    )
                }),
            })
        })
        .collect();
    Ok(())
}

fn same_cycle_end(left: DateTime<Utc>, right: DateTime<Utc>, provider: &str) -> bool {
    // Claude 账号接口的同一重置秒存在小数秒漂移；仅归并统计，不改原始时间。
    left == right || (provider == "claude" && left.timestamp() == right.timestamp())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProviderComparison {
    start_at: String,
    end_at: String,
    codex: ProviderPeriod,
    claude: ProviderPeriod,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderPeriod {
    plan_name: String,
    recorded_sample_count: usize,
    last_recorded_at: Option<String>,
    summary: CycleSummary,
    quota_from: String,
    quota_until: String,
    endpoint_skew_seconds: i64,
    percent_per_million_tokens: Option<f64>,
}

struct WeeklyReadings<'a> {
    reset: DateTime<Utc>,
    points: Vec<&'a Observation>,
}

fn weekly_readings<'a>(history: &'a History, snapshot: &UsageSnapshot, provider: &str) -> Result<WeeklyReadings<'a>, String> {
    let window = snapshot.windows.iter().find(|window| window.window_duration_mins == 10_080)
        .ok_or_else(|| format!("{provider} 尚未提供周额度窗口"))?;
    let reset = DateTime::parse_from_rfc3339(&window.reset_at).map_err(|error| error.to_string())?.with_timezone(&Utc);
    let points = history.observations.get(provider).into_iter().flatten().filter(|point| {
        point.at >= reset - Duration::days(7) && point.at < reset && point.windows.iter().any(|candidate| {
            candidate.id == window.id && candidate.window_duration_mins == 10_080
                && DateTime::parse_from_rfc3339(&candidate.reset_at)
                    .is_ok_and(|end| same_cycle_end(end.with_timezone(&Utc), reset, provider))
        })
    }).collect::<Vec<_>>();
    if points.len() < 2 { return Err(format!("{provider} 本周额度采样不足，等待至少两次读数")); }
    Ok(WeeklyReadings { reset, points })
}

pub(super) fn compare_providers(
    history: &History,
    codex: &UsageSnapshot,
    claude: &UsageSnapshot,
    codex_samples: &[TokenUsageSample],
    claude_samples: &[TokenUsageSample],
) -> Result<ProviderComparison, String> {
    let gpt = weekly_readings(history, codex, "codex")?;
    let cl = weekly_readings(history, claude, "claude")?;
    let start = gpt.points[0].at.max(cl.points[0].at);
    let end = gpt.points.last().expect("two readings").at.min(cl.points.last().expect("two readings").at);
    if end <= start { return Err("两边本周额度尚无共同观测区间，等待后续采样".into()); }
    Ok(ProviderComparison {
        start_at: start.to_rfc3339(), end_at: end.to_rfc3339(),
        codex: compare_period(&gpt, start, end, codex_samples, "codex", &codex.plan_name)?,
        claude: compare_period(&cl, start, end, claude_samples, "claude", &claude.plan_name)?,
    })
}

fn compare_period(
    readings: &WeeklyReadings<'_>, start: DateTime<Utc>, end: DateTime<Utc>,
    samples: &[TokenUsageSample], provider: &str, plan: &str,
) -> Result<ProviderPeriod, String> {
    let first = readings.points.iter().rposition(|point| point.at <= start).expect("common start has prior reading");
    let last = readings.points.iter().rposition(|point| point.at <= end).expect("common end has prior reading");
    let from = readings.points[first].at;
    let until = readings.points[last].at;
    let skew = (start - from).num_seconds().max((end - until).num_seconds());
    if skew > 180 || from >= until { return Err(format!("{provider} 共同区间端点缺少 3 分钟内的额度读数，暂不能比较")); }
    let points = readings.points[first..=last].iter().map(|point| (**point).clone()).collect::<Vec<_>>();
    let mut timed = samples.iter().map(|sample| {
        DateTime::parse_from_rfc3339(&sample.timestamp)
            .map(|at| (at.with_timezone(&Utc), sample)).map_err(|error| error.to_string())
    }).collect::<Result<Vec<_>, _>>()?;
    timed.sort_by_key(|(at, _)| *at);
    let last_recorded_at = timed.last().map(|(at, _)| at.to_rfc3339());
    timed.retain(|(at, _)| *at > start && *at <= end);
    let mut summary = summarize(readings.reset - Duration::days(7), readings.reset, 10_080, &points, &timed, provider);
    summary.start_at = start.to_rfc3339();
    summary.end_at = end.to_rfc3339();
    // 跨平台指标要求额度与请求均落在共同区间；回落和不足 1 个百分点不排名。
    let percent_per_million_tokens = summary.token_usage.total.filter(|total| *total > 0.0)
        .filter(|_| summary.observed_percent >= 1.0 && summary.quota_drops == 0)
        .map(|total| summary.observed_percent / total * 1_000_000.0);
    Ok(ProviderPeriod { plan_name: plan.into(), recorded_sample_count: samples.len(), last_recorded_at,
        summary, quota_from: from.to_rfc3339(),
        quota_until: until.to_rfc3339(), endpoint_skew_seconds: skew, percent_per_million_tokens })
}

fn summarize(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    duration: u64,
    points: &[Observation],
    samples: &[(DateTime<Utc>, &TokenUsageSample)],
    provider: &str,
) -> CycleSummary {
    let cycle_samples = samples
        .iter()
        .filter(|(at, _)| *at >= start && *at < end)
        .map(|(_, sample)| *sample)
        .collect::<Vec<_>>();
    let mut usage = empty_token_usage();
    let mut low = 0.0;
    let mut high = 0.0;
    let mut priced_sample_count = 0;
    let mut models = BTreeSet::new();
    let mut unknown = BTreeSet::new();
    for sample in &cycle_samples {
        merge_token_usage(&mut usage, &sample.token_usage);
        models.insert(sample.model.clone());
        if let Some((min, max)) = cost(sample, provider) {
            priced_sample_count += 1;
            low += min;
            high += max;
        } else {
            unknown.insert(sample.model.clone());
        }
    }
    let readings = points
        .iter()
        .filter(|point| point.at >= start && point.at < end)
        .filter_map(|point| {
            let window = point.windows.iter().find(|window| {
                window.window_duration_mins == duration
                    && DateTime::parse_from_rfc3339(&window.reset_at)
                        .is_ok_and(|reset| same_cycle_end(reset.with_timezone(&Utc), end, provider))
            })?;
            Some((point.at, window.used * 100.0 / window.total))
        })
        .collect::<Vec<_>>();
    let mut drops = 0;
    let mut percent = 0.0;
    let mut matched_tokens = 0.0;
    let mut matched_low = 0.0;
    let mut matched_high = 0.0;
    let mut matched_priced = true;
    let mut pending_tokens = 0.0;
    let mut pending_low = 0.0;
    let mut pending_high = 0.0;
    let mut pending_priced = true;
    for pair in readings.windows(2) {
        let [(from, before), (until, after)] = pair else {
            unreachable!()
        };
        if until <= from {
            continue;
        }
        if after < before {
            drops += 1;
            pending_tokens = 0.0;
            pending_low = 0.0;
            pending_high = 0.0;
            pending_priced = true;
            continue;
        }
        let delta = after - before;
        let begin = samples.partition_point(|(at, _)| at <= from);
        let finish = samples.partition_point(|(at, _)| at <= until);
        for (_, sample) in &samples[begin..finish] {
            pending_tokens += sample.token_usage.total.unwrap_or(0.0);
            if let Some((min, max)) = cost(sample, provider) {
                pending_low += min;
                pending_high += max;
            } else {
                pending_priced = false;
            }
        }
        if delta > 0.0 {
            percent += delta;
            matched_tokens += pending_tokens;
            matched_low += pending_low;
            matched_high += pending_high;
            matched_priced &= pending_priced;
            pending_tokens = 0.0;
            pending_low = 0.0;
            pending_high = 0.0;
            pending_priced = true;
        }
    }
    let has_samples = !cycle_samples.is_empty();
    let has_cost = has_samples && unknown.is_empty();
    CycleSummary {
        priced_sample_count,
        priced_cost_low_usd: (priced_sample_count > 0).then_some(low),
        priced_cost_high_usd: (priced_sample_count > 0).then_some(high),
        cache_write_tokens: cycle_samples.iter().map(|sample| sample.cache_write).sum(),
        start_at: start.to_rfc3339(),
        end_at: end.to_rfc3339(),
        token_usage: usage,
        cost_low_usd: has_cost.then_some(low),
        cost_high_usd: has_cost.then_some(high),
        models: models.into_iter().collect(),
        unknown_price_models: unknown.into_iter().collect(),
        sample_count: cycle_samples.len(),
        observed_from: readings.first().map(|(at, _)| at.to_rfc3339()),
        observed_until: readings.last().map(|(at, _)| at.to_rfc3339()),
        observed_percent: percent,
        tokens_per_percent: (percent > 0.0 && matched_tokens > 0.0)
            .then(|| matched_tokens / percent),
        cost_per_percent_low: (percent > 0.0 && matched_priced && matched_tokens > 0.0)
            .then(|| matched_low / percent),
        cost_per_percent_high: (percent > 0.0 && matched_priced && matched_tokens > 0.0)
            .then(|| matched_high / percent),
        quota_drops: drops,
    }
}

// 统一按 2026-10-24 官方标准短上下文价格估算，便于跨周期比较；不是订阅扣额公式或实际账单。
fn cost(sample: &TokenUsageSample, provider: &str) -> Option<(f64, f64)> {
    let model = sample.model.as_str();
    let (input_price, cached_price, output_price, write_low, write_high) = if provider == "codex" {
        let (input, cached, output) = match model {
            "gpt-6.1-sol" => (2.0, 0.1, 10.0),
            "gpt-6.1-terra" => (1.0, 0.05, 5.0),
            "gpt-6-sol" => (2.0, 0.2, 10.0),
            "gpt-6-astra" => (10.0, 1.0, 50.0),
            "gpt-6-luna" => (0.1, 0.01, 0.5),
            "gpt-5.6-sol" => (4.0, 0.4, 20.0),
            "gpt-5.6-terra" => (2.0, 0.2, 12.0),
            "gpt-5.6-luna" => (0.2, 0.02, 1.2),
            "gpt-5.5" => (5.0, 0.5, 30.0),
            "gpt-5.4" => (2.5, 0.25, 15.0),
            "gpt-5.3-codex" => (1.75, 0.175, 14.0),
            _ => return None,
        };
        (input, cached, output, input * 1.25, input * 2.5)
    } else {
        let (input, cached, output) = match model {
            "claude-opus-5-5" => (4.0, 0.2, 20.0),
            "claude-sonnet-5-5" => (2.0, 0.1, 10.0),
            "claude-fable-5-1" | "claude-mythos-5-1" => (10.0, 0.25, 50.0),
            "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6"
            | "claude-opus-4-5" => (5.0, 0.5, 25.0),
            "claude-sonnet-5" => (2.0, 0.2, 10.0),
            "claude-sonnet-4-6" | "claude-sonnet-4-5" => (3.0, 0.3, 15.0),
            "claude-haiku-4-5" => (1.0, 0.1, 5.0),
            _ => return None,
        };
        (input, cached, output, input * 1.25, input * 2.0)
    };
    let input = sample.token_usage.input?;
    let cached = sample.token_usage.cached_input?;
    let output = sample.token_usage.output?;
    let uncached = input - cached - sample.cache_write;
    if uncached < 0.0 {
        return None;
    }
    let base = uncached * input_price + cached * cached_price + output * output_price;
    Some((
        (base + sample.cache_write * write_low) / 1_000_000.0,
        (base + sample.cache_write * write_high) / 1_000_000.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_subsecond_reset_drift_keeps_one_cycle_and_original_timestamps() {
        let readings = [
            ("2026-10-27T12:41:44Z", 9, "2026-10-27T17:30:00.324557Z"),
            ("2026-10-27T12:43:44Z", 10, "2026-10-27T17:30:00.544046Z"),
            ("2026-10-27T12:45:44Z", 11, "2026-10-27T17:30:00.744046Z"),
        ];
        let points = readings.into_iter().map(|(at, used, reset)| serde_json::from_value(json!({
            "at":at, "windows":[{"id":"five_hour","label":"5 小时用量","used":used,"total":100,"resetAt":reset,"windowDurationMins":300}]
        })).unwrap()).collect::<Vec<Observation>>();
        let mut snapshot: UsageSnapshot = serde_json::from_value(json!({
            "provider":"claude","planName":"Claude","subscriptionExpiresAt":"",
            "windows":points.last().unwrap().windows,"tokenUsage":empty_token_usage(),
            "creditsRemaining":0,"creditsTotal":0,"resets":[],"updatedAt":readings[2].0,
            "sourcePath":"fixture","accountSource":"claudeMod"
        })).unwrap();
        let mut history = History::default();
        history.observations.insert("claude".into(), points);
        enrich(Path::new("unused"), &mut history, &mut snapshot, &[]).unwrap();
        assert!(snapshot.cycle_comparisons[0].previous.is_none());
        assert_eq!(snapshot.cycle_comparisons[0].current.observed_percent, 2.0);
        assert_eq!(snapshot.windows[0].reset_at, readings[2].2);
        let first = DateTime::parse_from_rfc3339(readings[0].2).unwrap().with_timezone(&Utc);
        let last = DateTime::parse_from_rfc3339(readings[2].2).unwrap().with_timezone(&Utc);
        assert!(!same_cycle_end(first, last, "codex"));
    }

    fn sample(at: &str) -> TokenUsageSample {
        TokenUsageSample {
            timestamp: at.into(),
            model: "gpt-6.1-sol".into(),
            cache_write: 0.0,
            windows: Vec::new(),
            token_usage: TokenUsage {
                input: Some(1_000_000.0),
                cached_input: Some(500_000.0),
                output: Some(100_000.0),
                total: Some(1_100_000.0),
                reasoning_output: None,
                limit: None,
            },
        }
    }

    #[test]
    fn uses_model_price_and_counts_cached_input_once() {
        let mut sample = sample("2026-10-24T01:00:00Z");
        assert_eq!(cost(&sample, "codex"), Some((2.05, 2.05)));
        sample.model = "gpt-6.1-terra".into();
        assert_eq!(cost(&sample, "codex"), Some((1.025, 1.025)));
        sample.model = "codex-auto-review".into();
        assert_eq!(cost(&sample, "codex"), None);
    }

    #[test]
    fn unknown_models_preserve_priced_subtotal_without_inventing_a_total() {
        let known = sample("2026-10-24T01:00:00Z");
        let mut unknown = sample("2026-10-24T02:00:00Z");
        unknown.model = "codex-auto-review".into();
        let start = at("2026-10-24T00:00:00Z");
        let summary = summarize(start, start + Duration::days(7), 10080, &[],
            &[(at(&known.timestamp), &known), (at(&unknown.timestamp), &unknown)], "codex");
        assert_eq!(summary.cost_low_usd, None);
        assert_eq!(summary.priced_cost_low_usd, Some(2.05));
        assert_eq!(summary.priced_sample_count, 1);
        assert_eq!(summary.token_usage.total, Some(2_200_000.0));
    }

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Utc)
    }

    fn comparison_fixture(provider: &str, reset: &str, readings: &[(&str, f64)]) -> (UsageSnapshot, Vec<Observation>) {
        let points = readings.iter().map(|(time, used)| serde_json::from_value(json!({
            "at":time,"windows":[{"id":"weekly","label":"周用量","used":used,"total":100,"resetAt":reset,"windowDurationMins":10080}]
        })).unwrap()).collect::<Vec<Observation>>();
        let snapshot = serde_json::from_value(json!({
            "provider":provider,"planName":provider,"subscriptionExpiresAt":"","windows":points.last().unwrap().windows,
            "tokenUsage":empty_token_usage(),"creditsRemaining":0,"creditsTotal":0,"resets":[],"updatedAt":readings.last().unwrap().0,
            "sourcePath":"fixture","accountSource":"usageFile"
        })).unwrap();
        (snapshot, points)
    }

    #[test]
    fn cross_provider_uses_common_interval_despite_different_weekly_resets() {
        let (codex, gpt_points) = comparison_fixture("codex", "2026-10-30T00:00:00Z", &[
            ("2026-10-24T00:00:00Z", 10.0), ("2026-10-24T01:00:00Z", 12.0), ("2026-10-24T02:00:00Z", 14.0)]);
        let (claude, cl_points) = comparison_fixture("claude", "2026-10-31T00:00:00Z", &[
            ("2026-10-24T00:01:00Z", 20.0), ("2026-10-24T01:59:00Z", 21.0), ("2026-10-24T02:01:00Z", 22.0)]);
        let mut history = History::default();
        history.observations.insert("codex".into(), gpt_points);
        history.observations.insert("claude".into(), cl_points);
        let records = [sample("2026-10-24T00:00:30Z"), sample("2026-10-24T01:00:00Z"), sample("2026-10-24T02:00:30Z")];
        let result = compare_providers(&history, &codex, &claude, &records, &records).unwrap();
        assert_eq!(at(&result.start_at), at("2026-10-24T00:01:00Z"));
        assert_eq!(at(&result.end_at), at("2026-10-24T02:00:00Z"));
        assert_eq!(result.codex.summary.sample_count, 1);
        assert_eq!(result.claude.summary.sample_count, 1);
        assert!((result.codex.summary.observed_percent - 4.0).abs() < 1e-9);
        assert!((result.claude.summary.observed_percent - 1.0).abs() < 1e-9);
        assert_eq!(result.codex.endpoint_skew_seconds, 60);
        assert_eq!(result.claude.endpoint_skew_seconds, 60);
    }

    #[test]
    fn cross_provider_rejects_missing_endpoint_readings() {
        let (codex, gpt_points) = comparison_fixture("codex", "2026-10-30T00:00:00Z", &[
            ("2026-10-24T00:00:00Z", 10.0), ("2026-10-24T02:00:00Z", 12.0)]);
        let (claude, cl_points) = comparison_fixture("claude", "2026-10-31T00:00:00Z", &[
            ("2026-10-24T00:01:00Z", 20.0), ("2026-10-24T02:01:00Z", 21.0)]);
        let mut history = History::default();
        history.observations.insert("codex".into(), gpt_points);
        history.observations.insert("claude".into(), cl_points);
        assert!(compare_providers(&history, &codex, &claude, &[], &[]).err().unwrap().contains("端点"));
    }

    #[test]
    fn cross_provider_does_not_rank_quota_drops_or_missing_tokens() {
        let (snapshot, points) = comparison_fixture("codex", "2026-10-30T00:00:00Z", &[
            ("2026-10-24T00:00:00Z", 10.0), ("2026-10-24T00:01:00Z", 5.0), ("2026-10-24T00:02:00Z", 8.0)]);
        let mut history = History::default();
        history.observations.insert("codex".into(), points);
        let readings = weekly_readings(&history, &snapshot, "codex").unwrap();
        let records = [sample("2026-10-24T00:01:30Z")];
        let result = compare_period(&readings, readings.points[0].at, readings.points[2].at, &records, "codex", "Pro").unwrap();
        assert_eq!(result.summary.quota_drops, 1);
        assert_eq!(result.percent_per_million_tokens, None);
        let empty = compare_period(&readings, readings.points[1].at, readings.points[2].at, &[], "codex", "Pro").unwrap();
        assert_eq!(empty.summary.sample_count, 0);
        assert_eq!(empty.percent_per_million_tokens, None);
    }

    #[test]
    fn empty_comparison_exposes_existing_records_outside_the_current_period() {
        let (snapshot, points) = comparison_fixture("claude", "2026-10-30T00:00:00Z", &[
            ("2026-10-24T02:00:00Z", 10.0), ("2026-10-24T03:00:00Z", 12.0)]);
        let mut history = History::default();
        history.observations.insert("claude".into(), points);
        let readings = weekly_readings(&history, &snapshot, "claude").unwrap();
        let records = [sample("2026-10-24T01:00:00Z")];
        let result = compare_period(&readings, readings.points[0].at, readings.points[1].at, &records, "claude", "Claude Pro").unwrap();
        assert_eq!(result.recorded_sample_count, 1);
        assert_eq!(at(result.last_recorded_at.as_deref().unwrap()), at(&records[0].timestamp));
        assert_eq!(result.summary.sample_count, 0);
        assert_eq!(result.percent_per_million_tokens, None);
        let empty = compare_period(&readings, readings.points[0].at, readings.points[1].at, &[], "claude", "Claude Pro").unwrap();
        assert_eq!(empty.recorded_sample_count, 0);
        assert_eq!(empty.last_recorded_at, None);
    }

    #[test]
    fn unknown_cache_ttl_returns_a_price_range() {
        let mut sample = sample("2026-10-24T01:00:00Z");
        sample.model = "claude-opus-5-5".into();
        sample.cache_write = 100_000.0;
        let (low, high) = cost(&sample, "claude").unwrap();
        assert!((low - 4.2).abs() < 0.000001);
        assert!((high - 4.5).abs() < 0.000001);
    }

    #[test]
    fn quota_drop_does_not_inflate_observed_capacity() {
        let end = "2026-10-25T00:00:00Z";
        let points = [("2026-10-24T00:00:00Z", 10), ("2026-10-24T01:00:00Z", 20), ("2026-10-24T02:00:00Z", 0), ("2026-10-24T03:00:00Z", 10)]
            .into_iter().map(|(at, used)| serde_json::from_value(json!({"at": at, "windows": [{"id":"weekly","label":"7天","used":used,"total":100,"resetAt":end,"windowDurationMins":10080}]})).unwrap()).collect::<Vec<Observation>>();
        let records = [
            sample("2026-10-24T01:00:00Z"),
            sample("2026-10-24T02:00:00Z"),
            sample("2026-10-24T03:00:00Z"),
        ];
        let timed = records
            .iter()
            .map(|sample| {
                (
                    DateTime::parse_from_rfc3339(&sample.timestamp)
                        .unwrap()
                        .with_timezone(&Utc),
                    sample,
                )
            })
            .collect::<Vec<_>>();
        let end = DateTime::parse_from_rfc3339(end)
            .unwrap()
            .with_timezone(&Utc);
        let summary = summarize(
            end - Duration::days(7),
            end,
            10080,
            &points,
            &timed,
            "codex",
        );
        assert_eq!(summary.sample_count, 3);
        assert_eq!(summary.quota_drops, 1);
        assert_eq!(summary.observed_percent, 20.0);
        assert_eq!(summary.tokens_per_percent, Some(110_000.0));
    }

    #[test]
    fn samples_at_reset_time_belong_to_the_next_cycle() {
        let record = sample("2026-10-24T00:00:00Z");
        let end = DateTime::parse_from_rfc3339(&record.timestamp)
            .unwrap()
            .with_timezone(&Utc);
        let summary = summarize(
            end - Duration::days(7),
            end,
            10080,
            &[],
            &[(end, &record)],
            "codex",
        );
        assert_eq!(summary.sample_count, 0);
        assert_eq!(summary.cost_low_usd, None);
    }

    #[test]
    fn rounded_quota_readings_keep_requests_until_the_meter_advances() {
        let end = "2026-10-25T00:00:00Z";
        let points = [("2026-10-24T00:00:00Z", 10), ("2026-10-24T01:00:00Z", 10), ("2026-10-24T02:00:00Z", 11), ("2026-10-24T03:00:00Z", 11)]
            .into_iter().map(|(at, used)| serde_json::from_value(json!({"at": at, "windows": [{"id":"weekly","label":"7天","used":used,"total":100,"resetAt":end,"windowDurationMins":10080}]})).unwrap()).collect::<Vec<Observation>>();
        let records = [sample("2026-10-24T01:00:00Z"), sample("2026-10-24T02:00:00Z"), sample("2026-10-24T03:00:00Z")];
        let timed = records.iter().map(|sample| (DateTime::parse_from_rfc3339(&sample.timestamp).unwrap().with_timezone(&Utc), sample)).collect::<Vec<_>>();
        let end = DateTime::parse_from_rfc3339(end).unwrap().with_timezone(&Utc);
        let summary = summarize(end - Duration::days(7), end, 10080, &points, &timed, "codex");
        assert_eq!(summary.observed_percent, 1.0);
        assert_eq!(summary.tokens_per_percent, Some(2_200_000.0));
        assert_eq!(summary.cost_per_percent_low, Some(4.1));
    }
}
