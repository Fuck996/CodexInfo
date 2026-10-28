export type ThemeName = "orange" | "green";
export type UsageProvider = "codex" | "claude";
export type UsageMode = UsageProvider | "rotate";
export type MonitorOption = { id: string; label: string };

export type UsageWindow = {
  id: string;
  label: string;
  used: number;
  total: number;
  resetAt: string;
  windowDurationMins: number;
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

export type ResetChance = {
  id: string;
  label: string;
  expiresAt: string;
};

export type ResetCredits = {
  available: number;
};

export type UsageSnapshot = {
  cycleComparisons?: CycleComparison[];
  provider: UsageProvider;
  planName: string;
  planError?: string | null;
  subscriptionExpiresAt: string;
  windows: UsageWindow[];
  tokenUsage: TokenUsage;
  creditsRemaining: number;
  creditsTotal: number;
  resets: ResetChance[];
  resetCredits?: ResetCredits;
  updatedAt: string;
  sourcePath: string;
  accountSource: "codexAppServer" | "codexAuth" | "usageFile" | "claudeMod";
  cachedAt?: string;
};

export type CycleSummary = {
  pricedSampleCount?: number;
  pricedCostLowUsd?: number | null;
  pricedCostHighUsd?: number | null;
  cacheWriteTokens: number;
  startAt: string;
  endAt: string;
  tokenUsage: TokenUsage;
  costLowUsd: number | null;
  costHighUsd: number | null;
  models: string[];
  unknownPriceModels: string[];
  sampleCount: number;
  observedFrom: string | null;
  observedUntil: string | null;
  observedPercent: number;
  tokensPerPercent: number | null;
  costPerPercentLow: number | null;
  costPerPercentHigh: number | null;
  quotaDrops: number;
};

export type CycleComparison = {
  windowId: string;
  label: string;
  current: CycleSummary;
  previous: CycleSummary | null;
};

export type ProviderPeriod = {
  planName: string;
  recordedSampleCount: number;
  lastRecordedAt: string | null;
  summary: CycleSummary;
  quotaFrom: string;
  quotaUntil: string;
  endpointSkewSeconds: number;
  percentPerMillionTokens: number | null;
};

export type ProviderComparison = {
  startAt: string;
  endAt: string;
  codex: ProviderPeriod;
  claude: ProviderPeriod;
};

export type AppSettings = {
  usageMode: UsageMode;
  activeProvider: UsageProvider;
  rotationIntervalSecs: number;
  selectedMonitor: string | null;
  launchAtStartup: boolean;
  mainVisible: boolean;
  keepAlwaysOnTop: boolean;
  trayUsageEnabled: boolean;
  dockEnabled: boolean;
  taskbarUsageEnabled: boolean;
  theme: ThemeName;
};

export type DesktopApi = {
  reportFrontendError: (source: "error" | "unhandledrejection", message: string) => Promise<void>;
  getUsageSnapshot: (provider: UsageProvider) => Promise<UsageSnapshot>;
  getUsageComparison: () => Promise<ProviderComparison>;
  getCachedUsageSnapshot: (provider: UsageProvider) => Promise<UsageSnapshot | null>;
  setUsageMode: (mode: UsageMode, intervalSecs: number) => Promise<AppSettings>;
  getMonitors: () => Promise<MonitorOption[]>;
  setSelectedMonitor: (monitorId: string | null) => Promise<AppSettings>;
  installClaudeBridge: () => Promise<string>;
  setSettingsOpen: (open: boolean) => Promise<void>;
  getSettings: () => Promise<AppSettings>;
  setLaunchAtStartup: (enabled: boolean) => Promise<AppSettings>;
  setTrayUsageEnabled: (enabled: boolean) => Promise<AppSettings>;
  setDockEnabled: (enabled: boolean) => Promise<AppSettings>;
  setTaskbarUsageEnabled: (enabled: boolean) => Promise<AppSettings>;
  setExpanded: (expanded: boolean, extraHeight?: number) => Promise<void>;
  setHoverRegion: (region: "main" | "dock", active: boolean) => Promise<void>;
  showDockMenu: () => Promise<void>;
};
