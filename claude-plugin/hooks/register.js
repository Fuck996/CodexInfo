export function register(on) {
  on("turn.step", async function* ($, event, next) {
    const result = yield* next(event);
    if (result.usage) {
      await exportReading($, {
        type: "sample",
        id: `${await $.session.id()}-${event.turnId}-${event.agentId ?? "main"}-${event.index}`,
        timestamp: new Date().toISOString(),
        usage: result.usage
      });
    }
    return result;
  });
  on("session.measure", async ($, event, next) => {
    const result = await next(event);
    await exportReading($, {
      type: "reading",
      sessionId: await $.session.id(),
      updatedAt: new Date().toISOString(),
      rateLimits: event.rateLimits
    });
    return result;
  });
}

async function exportReading($, reading) {
  const config = JSON.parse(await $.fs.read(`${$.plugin.root}/bridge.json`));
  const exported = await $.process.run([config.executable, "--claude-usage", config.directory], {
    stdin: JSON.stringify(reading), timeoutMs: 5000
  });
  if (exported.exitCode !== 0) throw new Error(`CodexInfo 用量导出失败：${exported.stderr}`);
}
