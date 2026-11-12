const defaultPollDelayMs = 150_000;
const maxTimerDelayMs = 2_147_483_647;

async function synchronize($, state) {
  if (state.inFlight || state.stopped) return;
  state.inFlight = true;
  let claimed = false;
  let pollDelayMs = defaultPollDelayMs;
  try {
    const config = JSON.parse(await $.fs.read(`${$.plugin.root}/bridge.json`));
    const result = await $.process.run([config.executable, "--claude-poll-claim", config.directory], { timeoutMs: 5000 });
    if (result.exitCode !== 0) throw new Error(`账号查询限频失败：${result.stderr}`);
    const lease = JSON.parse(result.stdout);
    if (typeof lease.granted !== "boolean") throw new Error("账号查询限频响应无效");
    // Older collectors only return whether the claim was granted.
    pollDelayMs = lease.retryInMs ?? defaultPollDelayMs;
    if (!Number.isSafeInteger(pollDelayMs) || pollDelayMs <= 0) throw new Error("账号查询限频等待时间无效");
    if (!lease.granted) return pollDelayMs;
    claimed = true;
    const authorization = await $.session.authorize();
    if (!authorization || authorization.kind !== "bearer") throw new Error("当前 Code 会话没有 Claude 订阅授权");
    const response = await $.http.fetch("https://api.anthropic.com/api/oauth/usage", {
      auth: authorization.handle,
      headers: { "anthropic-beta": "oauth-2025-04-20" }
    });
    if (!response.ok) throw new QuotaReadError(`Claude 账号用量查询失败（HTTP ${response.status}）`, response.status, retryDelay(response));
    let body;
    try { body = JSON.parse(response.text); }
    catch { throw new Error("Claude 账号用量响应不是有效 JSON"); }
    const rateLimits = quotaWindows(body);
    if (state.stopped) return;
    const plan = await subscriptionPlan($, authorization);
    if (!state.stopped) await exportReading($, {
      type: "reading", source: "account",
      sessionId: await fileIdentifier(await $.session.id()),
      updatedAt: new Date().toISOString(), rateLimits, ...plan
    });
    return pollDelayMs;
  } catch (error) {
    if (!claimed || state.stopped) throw error;
    const retryAfterSecs = error instanceof QuotaReadError ? error.retryAfterSecs : 120;
    await exportReading($, {
      type: "syncFailure", updatedAt: new Date().toISOString(),
      message: error instanceof Error ? error.message : String(error),
      statusCode: error instanceof QuotaReadError ? error.statusCode : null,
      retryAfterSecs
    });
    return Math.max(pollDelayMs, retryAfterSecs * 1000);
  } finally { state.inFlight = false; }
}

async function subscriptionPlan($, authorization) {
  try {
    const response = await $.http.fetch("https://api.anthropic.com/api/oauth/profile", {
      auth: authorization.handle,
      headers: { "Cache-Control": "no-cache" }
    });
    if (!response.ok) throw new Error(`Claude 套餐查询失败（HTTP ${response.status}）`);
    const profile = JSON.parse(response.text);
    const organization = profile?.organization;
    // 与 Claude Code 2.1.309 的 profile 显示名校验及组织类型映射保持一致。
    const display = organization?.plan_display_name;
    if (display != null) {
      if (typeof display !== "string" || !/^[\p{L}\p{N}](?:[\p{L}\p{N} .+-]{0,38}[\p{L}\p{N}+])?$/u.test(display)) throw new Error("Claude 套餐显示名无效");
      return { planName: display.startsWith("Claude") ? display : `Claude ${display}`, planError: null };
    }
    const names = new Map([["claude_pro", "Claude Pro"], ["claude_max", "Claude Max"], ["claude_team", "Claude Team"], ["claude_enterprise", "Claude Enterprise"]]);
    const name = names.get(organization?.organization_type);
    if (!name) throw new Error("Claude 账号未提供可识别的套餐类型");
    return { planName: name, planError: null };
  } catch (error) {
    return { planName: null, planError: error instanceof Error ? error.message : String(error) };
  }
}

function tick($, state) {
  const schedule = (delay = defaultPollDelayMs) => {
    if (!state.stopped) state.timer = $.clock.after(Math.min(delay, maxTimerDelayMs), () => tick($, state));
  };
  void synchronize($, state).then(schedule, (error) => {
    $.ui.log(`CodexInfo 同步失败：${error instanceof Error ? error.message : String(error)}`);
    schedule();
  });
}

export function register(on) {
  const state = { timer: undefined, stopped: false, inFlight: false };
  on("session.start", async ($, event, next) => {
    const result = await next(event);
    state.stopped = false;
    state.timer?.cancel();
    tick($, state);
    return result;
  });
  on("session.end", async ($, event, next) => {
    if (event.reason === "clear" || event.reason === "resume") return next(event);
    state.stopped = true;
    state.timer?.cancel();
    state.timer = undefined;
    return next(event);
  });
  on("turn.step", async function* ($, event, next) {
    const result = yield* next(event);
    if (result.usage) {
      await exportReading($, {
        type: "sample",
        id: await fileIdentifier(`${await $.session.id()}-${event.turnId}-${event.agentId ?? "main"}-${event.index}`),
        timestamp: new Date().toISOString(),
        usage: result.usage
      });
    }
    return result;
  });
}

class QuotaReadError extends Error {
  constructor(message, statusCode, retryAfterSecs) {
    super(message);
    this.statusCode = statusCode;
    this.retryAfterSecs = retryAfterSecs;
  }
}

function retryDelay(response) {
  const value = response.headers["retry-after"];
  const seconds = value && /^\d+$/.test(value) ? Number(value) : (Date.parse(value) - Date.now()) / 1000;
  if (Number.isFinite(seconds) && seconds > 0) return Math.max(120, Math.ceil(seconds));
  return response.status === 429 ? 600 : 120;
}

function quotaWindows(body) {
  if (!body || typeof body !== "object") throw new Error("Claude 账号用量结构无效");
  const windows = [];
  for (const kind of ["five_hour", "seven_day"]) {
    const value = body[kind];
    if (value === null || value === undefined) continue;
    if (typeof value !== "object" || Array.isArray(value)) throw new Error(`Claude ${kind} 额度结构无效`);
    if (value.utilization !== null && (!Number.isFinite(value.utilization)
        || value.utilization < 0 || value.utilization > 100)) {
      throw new Error(`Claude ${kind}.utilization 无效：应为 0–100 的数值或 null`);
    }
    // Claude 的窗口读数和重置时间可独立为 null，不能补成零或虚构时间。
    if (value.resets_at !== null && (typeof value.resets_at !== "string" || !Number.isFinite(Date.parse(value.resets_at)))) {
      throw new Error(`Claude ${kind}.resets_at 无效：应为时间字符串或 null`);
    }
    windows.push({ kind, percentUsed: value.utilization, resetsAt: value.resets_at });
  }
  if (!windows.length) throw new Error("Claude 账号接口未返回有效额度窗口");
  return windows;
}

async function fileIdentifier(value) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function exportReading($, reading) {
  const config = JSON.parse(await $.fs.read(`${$.plugin.root}/bridge.json`));
  const exported = await $.process.run([config.executable, "--claude-usage", config.directory], {
    stdin: JSON.stringify(reading), timeoutMs: 5000
  });
  if (exported.exitCode !== 0) throw new Error(`CodexInfo 用量导出失败：${exported.stderr}`);
}
