import assert from "node:assert/strict";
import test from "node:test";
import { setImmediate, setTimeout } from "node:timers/promises";
import { register } from "../claude-plugin/hooks/register.js";

async function waitForExport(exports, count) {
  const deadline = Date.now() + 1000;
  while (exports.length < count && Date.now() < deadline) await setTimeout(1);
  assert.equal(exports.length, count);
}

function harness() {
  const handlers = new Map();
  const exports = [];
  const requests = [];
  const logs = [];
  let tick;
  let cancelled = false;
  register((event, handler) => handlers.set(event, handler));
  const api = {
    plugin: { root: "plugin" },
    session: { id: async () => "session:opaque/id", authorize: async () => ({ handle: "opaque-handle", kind: "bearer" }) },
    clock: { every: (ms, callback) => { assert.equal(ms, 120_000); tick = callback; return { cancel: () => { cancelled = true; } }; } },
    ui: { log: (message) => logs.push(message) },
    http: { fetch: async (url, init) => {
      requests.push({ url, init });
      if (url.endsWith("/profile")) return { ok: true, status: 200, text: JSON.stringify({
        account: { email: "private@example.com" }, organization: { organization_type: "claude_pro" }
      }) };
      return { ok: true, status: 200, headers: {}, text: JSON.stringify({
        five_hour: { utilization: 42.5, resets_at: "2027-01-01T05:00:00Z" },
        seven_day: { utilization: 7, resets_at: "2027-01-07T00:00:00Z" }
      }) };
    } },
    fs: { read: async () => JSON.stringify({ executable: "collector.exe", directory: "readings" }) },
    process: { run: async (argv, options) => {
      if (argv[1] === "--claude-poll-claim") return { exitCode: 0, stdout: '{"granted":true}', stderr: "" };
      exports.push({ argv, reading: JSON.parse(options.stdin) });
      return { exitCode: 0, stdout: "", stderr: "" };
    } }
  };
  return { api, handlers, exports, requests, logs,
    start: async () => { await handlers.get("session.start")(api, {}, async () => ({})); await setImmediate(); },
    tick: async () => { tick(); await setImmediate(); },
    cancelled: () => cancelled
  };
}

test("idle bridge polls account usage without a model turn or exporting authorization", async () => {
  const h = harness();
  await h.start();
  await waitForExport(h.exports, 1);
  assert.equal(h.requests[0].url, "https://api.anthropic.com/api/oauth/usage");
  assert.equal(h.requests[0].init.auth, "opaque-handle");
  assert.equal(h.requests[1].url, "https://api.anthropic.com/api/oauth/profile");
  assert.equal(h.requests[1].init.auth, h.requests[0].init.auth);
  assert.equal(h.exports[0].reading.source, "account");
  assert.equal(h.exports[0].reading.planName, "Claude Pro");
  assert.equal(h.exports[0].reading.rateLimits[0].percentUsed, 42.5);
  assert.match(h.exports[0].reading.sessionId, /^[a-f0-9]{64}$/);
  assert.equal(JSON.stringify(h.exports).includes("opaque-handle"), false);
  assert.equal(JSON.stringify(h.exports).includes("private@example.com"), false);
  assert.equal(h.handlers.has("session.measure"), false);
  await h.tick();
  await waitForExport(h.exports, 2);
});

test("plan metadata failures are visible and do not discard account quotas", async () => {
  const h = harness();
  const fetch = h.api.http.fetch;
  h.api.http.fetch = async (url, init) => url.endsWith("/profile")
    ? { ok: false, status: 403 }
    : fetch(url, init);
  await h.start();
  await waitForExport(h.exports, 1);
  assert.equal(h.exports[0].reading.planName, null);
  assert.match(h.exports[0].reading.planError, /HTTP 403/);
  assert.equal(h.exports[0].reading.rateLimits[1].percentUsed, 7);
});

test("profile display names follow vendor validation and unknown plans stay explicit", async () => {
  for (const [organization, expected, failure] of [
    [{ plan_display_name: "Max 20x", organization_type: "claude_max" }, "Claude Max 20x", null],
    [{ plan_display_name: "<invalid>", organization_type: "claude_pro" }, null, /显示名无效/],
    [{ organization_type: "constructor" }, null, /未提供可识别/]
  ]) {
    const h = harness();
    const fetch = h.api.http.fetch;
    h.api.http.fetch = async (url, init) => url.endsWith("/profile")
      ? { ok: true, text: JSON.stringify({ organization }) }
      : fetch(url, init);
    await h.start();
    await waitForExport(h.exports, 1);
    assert.equal(h.exports[0].reading.planName, expected);
    if (failure) assert.match(h.exports[0].reading.planError, failure);
  }
});

test("another session's claim prevents duplicate account requests", async () => {
  const h = harness();
  h.api.process.run = async () => ({ exitCode: 0, stdout: '{"granted":false}' });
  await h.start();
  await h.tick();
  assert.equal(h.requests.length, 0);
  assert.equal(h.exports.length, 0);
});

test("rate limiting is exported with server retry delay and no stale snapshot", async () => {
  const h = harness();
  h.api.http.fetch = async () => ({ ok: false, status: 429, headers: { "retry-after": "700" } });
  await h.start();
  assert.equal(h.exports[0].reading.type, "syncFailure");
  assert.equal(h.exports[0].reading.retryAfterSecs, 700);
  assert.match(h.exports[0].reading.message, /HTTP 429/);
});

test("invalid account fields and missing authorization fail explicitly", async () => {
  const h = harness();
  h.api.http.fetch = async () => ({ ok: true, text: '{"five_hour":{"utilization":150,"resets_at":"2027-01-01T00:00:00Z"}}' });
  await h.start();
  assert.equal(h.exports[0].reading.type, "syncFailure");
  assert.match(h.exports[0].reading.message, /额度字段无效/);
  const missing = harness();
  missing.api.session.authorize = async () => null;
  await missing.start();
  assert.match(missing.exports[0].reading.message, /订阅授权/);
  assert.equal(missing.requests.length, 0);
});

test("ending session cancels polling and discards outstanding response", async () => {
  const h = harness();
  let resolve;
  h.api.http.fetch = () => new Promise((done) => { resolve = done; });
  await h.start();
  await h.handlers.get("session.end")(h.api, {}, async () => ({}));
  resolve({ ok: true, text: '{"five_hour":{"utilization":10,"resets_at":"2027-01-01T00:00:00Z"}}' });
  await setImmediate();
  await h.tick();
  assert.equal(h.cancelled(), true);
  assert.equal(h.exports.length, 0);
});

test("clearing or resuming a conversation keeps the loaded bridge polling", async () => {
  const h = harness();
  await h.start();
  await h.handlers.get("session.end")(h.api, { reason: "clear" }, async () => ({}));
  await h.tick();
  assert.equal(h.cancelled(), false);
  assert.equal(h.requests.length, 4);
});

test("request observer preserves the original stream and exports usage once", async () => {
  const { api, handlers, exports } = harness();
  const chunks = [{ kind: "text", text: "private response" }, { kind: "stop" }];
  const result = { usage: { model: "claude-opus-5-5", input_tokens: 10, output_tokens: 2, cache_read_input_tokens: 20, cache_creation_input_tokens: 0 } };
  async function* next() { yield* chunks; return result; }
  const stream = handlers.get("turn.step")(api, { turnId: "turn:1", index: 0 }, next);
  assert.equal((await stream.next()).value, chunks[0]);
  assert.equal((await stream.next()).value, chunks[1]);
  assert.deepEqual(await stream.next(), { done: true, value: result });
  assert.equal(exports.length, 1);
  assert.match(exports[0].reading.id, /^[a-f0-9]{64}$/);
  assert.equal(JSON.stringify(exports).includes("private response"), false);
});

test("collector failures are surfaced instead of reporting a successful export", async () => {
  const h = harness();
  h.api.process.run = async () => ({ exitCode: 1, stderr: "write failed" });
  await h.start();
  assert.match(h.logs[0], /write failed/);
});
