import assert from "node:assert/strict";
import test from "node:test";
import { register } from "../claude-plugin/hooks/register.js";

function harness() {
  const handlers = new Map();
  const exports = [];
  register((event, handler) => handlers.set(event, handler));
  const api = {
    plugin: { root: "plugin" },
    session: { id: async () => "session:opaque/id" },
    fs: { read: async () => JSON.stringify({ executable: "collector.exe", directory: "readings" }) },
    process: { run: async (argv, options) => {
      exports.push({ argv, reading: JSON.parse(options.stdin) });
      return { exitCode: 0, stdout: "", stderr: "" };
    } }
  };
  return { api, handlers, exports };
}

test("measurement exports only quota metadata and hashes opaque identifiers", async () => {
  const { api, handlers, exports } = harness();
  const event = { rateLimits: [{ kind: "five_hour", percentUsed: 25 }], context: { privateText: "not exported" } };
  const result = { changed: ["rateLimits"] };
  assert.equal(await handlers.get("session.measure")(api, event, async () => result), result);
  assert.deepEqual(exports[0].argv, ["collector.exe", "--claude-usage", "readings"]);
  assert.match(exports[0].reading.sessionId, /^[a-f0-9]{64}$/);
  assert.deepEqual(Object.keys(exports[0].reading).sort(), ["rateLimits", "sessionId", "type", "updatedAt"]);
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
  const { api, handlers } = harness();
  api.process.run = async () => ({ exitCode: 1, stderr: "write failed" });
  await assert.rejects(handlers.get("session.measure")(api, { rateLimits: [] }, async () => ({})), /write failed/);
});
