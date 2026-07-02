import { invoke } from "@tauri-apps/api/core";
import type { AppSettings, DesktopApi, UsageSnapshot } from "../shared/types";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

const tauriApi: DesktopApi = {
  getUsageSnapshot: () => invoke<UsageSnapshot>("get_usage_snapshot"),
  getCachedUsageSnapshot: () => invoke<UsageSnapshot | null>("get_cached_usage_snapshot"),
  getSettings: () => invoke<AppSettings>("get_settings"),
  setLaunchAtStartup: (enabled: boolean) => invoke<AppSettings>("set_launch_at_startup", { enabled }),
  setTrayUsageEnabled: (enabled: boolean) => invoke<AppSettings>("set_tray_usage_enabled", { enabled }),
  setDockEnabled: (enabled: boolean) => invoke<AppSettings>("set_dock_enabled", { enabled }),
  setTaskbarUsageEnabled: (enabled: boolean) => invoke<AppSettings>("set_taskbar_usage_enabled", { enabled }),
  setExpanded: (expanded: boolean, extraHeight?: number) => invoke<void>("set_expanded", { expanded, extraHeight }),
  setHoverRegion: (region: "main" | "dock", active: boolean) => invoke<void>("set_hover_region", { region, active })
};

const unavailableApi: DesktopApi = {
  getUsageSnapshot: async () => {
    throw new Error("Tauri 桥接不可用，请使用 npm run tauri:dev 或打包后的程序启动。");
  },
  getCachedUsageSnapshot: async () => null,
  getSettings: async () => ({
    launchAtStartup: false,
    mainVisible: false,
    keepAlwaysOnTop: true,
    trayUsageEnabled: true,
    dockEnabled: true,
    taskbarUsageEnabled: true
  }),
  setLaunchAtStartup: async () => {
    throw new Error("Tauri 桥接不可用");
  },
  setTrayUsageEnabled: async () => {
    throw new Error("Tauri 桥接不可用");
  },
  setDockEnabled: async () => {
    throw new Error("Tauri bridge unavailable");
  },
  setTaskbarUsageEnabled: async () => {
    throw new Error("Tauri 桥接不可用");
  },
  setExpanded: async () => undefined,
  setHoverRegion: async () => undefined
};

export const desktopApi = window.__TAURI_INTERNALS__ ? tauriApi : unavailableApi;
