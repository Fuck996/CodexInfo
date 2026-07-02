export type UsageWindowId = "fiveHour" | "sevenDay";

export type UsageWindow = {
  id: UsageWindowId;
  label: string;
  used: number;
  total: number;
  resetAt: string;
  displayMode?: "count" | "percent";
};

export type TokenUsage = {
  input: number | null;
  cachedInput?: number | null;
  output: number | null;
  reasoningOutput?: number | null;
  total?: number | null;
  limit: number | null;
};

export type TokenPeriodId = "thisWeek" | "lastWeek" | "thisMonth" | "lastMonth";

export type TokenPeriodUsage = {
  id: TokenPeriodId;
  label: string;
  rangeLabel: string;
  tokenUsage: TokenUsage;
  computedAt: string;
  hasData: boolean;
};

export type ResetChance = {
  id: string;
  label: string;
  expiresAt: string;
};

export type ResetCredits = {
  available: number;
};

export type UsageSnapshot = {
  planName: string;
  subscriptionExpiresAt: string;
  windows: UsageWindow[];
  tokenUsage: TokenUsage;
  creditsRemaining: number;
  creditsTotal: number;
  resets: ResetChance[];
  resetCredits?: ResetCredits;
  updatedAt: string;
  sourcePath: string;
  accountSource: "codexAppServer" | "codexAuth" | "usageFile";
  cachedAt?: string;
  tokenPeriods?: TokenPeriodUsage[];
};

export type AppSettings = {
  launchAtStartup: boolean;
  mainVisible: boolean;
  keepAlwaysOnTop: boolean;
  trayUsageEnabled: boolean;
  dockEnabled: boolean;
  taskbarUsageEnabled: boolean;
};

export type DesktopApi = {
  getUsageSnapshot: () => Promise<UsageSnapshot>;
  getCachedUsageSnapshot: () => Promise<UsageSnapshot | null>;
  getSettings: () => Promise<AppSettings>;
  setLaunchAtStartup: (enabled: boolean) => Promise<AppSettings>;
  setTrayUsageEnabled: (enabled: boolean) => Promise<AppSettings>;
  setDockEnabled: (enabled: boolean) => Promise<AppSettings>;
  setTaskbarUsageEnabled: (enabled: boolean) => Promise<AppSettings>;
  setExpanded: (expanded: boolean, resetCount?: number) => Promise<void>;
};
