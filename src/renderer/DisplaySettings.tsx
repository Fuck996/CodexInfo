import { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import type { AppSettings, MonitorOption } from "../shared/types";
import { desktopApi } from "./desktopApi";

export function DisplaySettings({ settings, onChange, onClose }: {
  settings: AppSettings;
  onChange: (settings: AppSettings) => void;
  onClose: () => void;
}) {
  const [monitors, setMonitors] = useState<MonitorOption[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const closeButton = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let mounted = true;
    const previousFocus = document.activeElement;
    void desktopApi.setSettingsOpen(true).catch((error) => setError(String(error)));
    closeButton.current?.focus();
    async function load() {
      try {
        const options = await desktopApi.getMonitors();
        if (mounted) setMonitors(options);
      } catch (error) {
        if (mounted) setError(String(error));
      } finally { if (mounted) setLoading(false); }
    }
    void load();
    const timer = window.setInterval(load, 5000);
    return () => {
      mounted = false;
      window.clearInterval(timer);
      void desktopApi.setSettingsOpen(false).catch(console.error);
      if (previousFocus instanceof HTMLElement) previousFocus.focus();
    };
  }, []);

  async function save(operation: () => Promise<AppSettings>) {
    setSaving(true);
    setError("");
    try { onChange(await operation()); }
    catch (error) { setError(String(error)); }
    finally { setSaving(false); }
  }

  async function connect() {
    setConnecting(true);
    setError("");
    setMessage("");
    try { setMessage(await desktopApi.installClaudeBridge()); }
    catch (error) { setError(String(error)); }
    finally { setConnecting(false); }
  }

  const missingMonitor = settings.selectedMonitor !== null && !monitors.some((monitor) => monitor.id === settings.selectedMonitor);
  return (
    <div className="settings-scrim" onClick={onClose}>
      <section className="display-settings" role="dialog" aria-modal="true" aria-labelledby="settings-title"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
          if (event.key === "Tab") {
            const controls = event.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled), select:not(:disabled)");
            const target = event.shiftKey ? controls[controls.length - 1] : controls[0];
            const boundary = event.shiftKey ? controls[0] : controls[controls.length - 1];
            if (document.activeElement === boundary) { event.preventDefault(); target?.focus(); }
          }
        }}>
        <header><strong id="settings-title">显示设置</strong><button ref={closeButton} onClick={onClose} aria-label="关闭设置"><X size={17} /></button></header>
        <label htmlFor="display-monitor">显示在哪个屏幕</label>
        <select id="display-monitor" value={settings.selectedMonitor ?? ""} disabled={loading || saving}
          onChange={(event) => { const id = event.target.value || null; void save(() => desktopApi.setSelectedMonitor(id)); }}>
          <option value="">跟随主屏</option>
          {missingMonitor && <option value={settings.selectedMonitor ?? ""}>已选显示器未连接（暂用主屏）</option>}
          {monitors.map((monitor) => <option key={monitor.id} value={monitor.id}>{monitor.label}</option>)}
        </select>
        <p>悬浮条和展开面板一起移动。断开时暂用主屏，重连后恢复。</p>
        <label htmlFor="rotation-interval">自动轮播间隔</label>
        <select id="rotation-interval" value={settings.rotationIntervalSecs} disabled={saving}
          onChange={(event) => { const interval = Number(event.target.value); void save(() => desktopApi.setUsageMode(settings.usageMode, interval)); }}>
          {[5, 10, 30, 60].map((seconds) => <option key={seconds} value={seconds}>每 {seconds} 秒</option>)}
        </select>
        <div className="claude-connection">
          <strong>Claude 账号用量桥接</strong>
          <p>完成 Claude Code 官方登录后，组件会自动在后台每 2 分钟查询账号额度，包含 Projects 和普通聊天的消耗。无需打开 Code 页，授权始终由 Claude 持有。</p>
          <button className="connect-button" disabled={connecting} onClick={() => void connect()}>{connecting ? "正在接入…" : "接入 Claude Code"}</button>
        </div>
        {saving && <p role="status">正在保存…</p>}
        {message && <p className="settings-message" role="status">{message}</p>}
        {error && <p className="settings-error" role="alert">{error}</p>}
      </section>
    </div>
  );
}
