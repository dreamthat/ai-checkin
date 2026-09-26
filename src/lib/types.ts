// 与 Rust 后端命令返回结构对齐的类型定义（对照 server.py 各 API 响应）

/**
 * WorkBuddy 客户端档位：国内版（cn）/ 国际版（ai）。
 * 后端以字符串返回，历史数据与旧响应可能缺省该字段，读取时统一按国内版处理。
 */
export type WbVariant = "cn" | "ai";

export interface AccountMeta {
  id: string;
  uid: string | null;
  email: string | null;
  nickname: string | null;
  enterpriseName: string | null;
  expiresAt: number | null;
  refreshExpiresAt: number | null;
  refreshedAt: number | null;
  createdAt: number | null;
  needsRelogin: boolean;
  needsReloginReason: string | null;
  /** 账号所属档位；缺省（旧后端/历史账号）按国内版处理。 */
  variant?: WbVariant;
}

export interface AppStatus {
  running: boolean;
  authFile: string;
  current: {
    uid: string | null;
    nickname: string | null;
    email: string | null;
  } | null;
  appPath: string;
  version: string;
  /** 上述字段所属档位；缺省按国内版处理。 */
  variant?: WbVariant;
}

export interface OAuthStartResult {
  loginId: string;
  verificationUri: string;
  expiresIn: number;
}

export interface OAuthPollResult {
  done: boolean;
  result?: AccountMeta;
  error?: string;
}

/** 导出文件中的完整账号记录（含 token，仅导出命令返回；字段与账号库原始记录一致）。 */
export interface AccountRecord {
  id?: string;
  uid?: string | null;
  nickname?: string | null;
  email?: string | null;
  access_token?: string | null;
  refresh_token?: string | null;
  token_type?: string | null;
  domain?: string | null;
  expiresAt?: number | null;
  refreshExpiresAt?: number | null;
  auth_raw?: unknown;
  profile_raw?: unknown;
  createdAt?: number | null;
  [key: string]: unknown;
}

/** 导入文件账号的脱敏预览（不含 token）。 */
export interface ImportPreviewAccount {
  index: number;
  uid: string | null;
  nickname: string | null;
  email: string | null;
  hasToken: boolean;
}

/** 导入结果计数。 */
export interface ImportResult {
  ok: boolean;
  imported: number;
  skipped: number;
  overwritten: number;
}

export interface Session {
  id: string;
  title: string;
  cwd: string;
  updatedAt: number;
  hasHistory: boolean;
  /** WorkBuddy playground（侧栏「任务」）；缺省视为空间会话。 */
  isPlayground?: boolean;
}

/** 临时备份的清理状态：cleaned 已回收；pending 已保留待下次维护重试；legacyRetained 旧操作无生命周期记录。 */
export type SessionBackupCleanupState = "cleaned" | "pending" | "legacyRetained";

/**
 * 临时备份残留（待清理 / 待恢复）：复制、同步、恢复报告共用同一结构。
 * `cleanupPending` 表示已完成但本轮没清理成功（下次切号重试）；`needsRecovery`
 * 表示必须保留材料、需要恢复流程或人工确认。
 */
export interface TemporaryFileInfo {
  operationId: string;
  sessionId?: string;
  title?: string;
  state: "cleanupPending" | "needsRecovery";
  reason: string;
}

/** 本次新建的副本（目标 UUID 由后端预分配）。 */
export interface CopyResult {
  id: string;
  newId: string;
  groupId: string;
  /** 待清理位置（已清理为 null）；仅表示待清理，不是可撤销备份。 */
  backup: string | null;
  /** 成功后立即清理：cleaned 已回收 / pending 待下次维护重试；旧后端可能缺字段。 */
  cleanupState?: SessionBackupCleanupState;
  /** 清理失败原因（`cleanupState` 为 pending 时有值）。 */
  cleanupError?: string;
}

/** 目标账号上已有真实有效的副本：复用而不是重复复制。 */
export interface LinkedCopyResult {
  id: string;
  sessionId: string;
  groupId: string;
}

/** 切换时的会话复制报告；复制失败时后端只回 `error`（切换本身仍继续）。 */
export interface SessionCopyReport {
  sourceUid?: string;
  targetUid?: string;
  copied?: CopyResult[];
  alreadyLinked?: LinkedCopyResult[];
  errors?: { id: string; error: string }[];
  /** 仍有未完成的会话写入时为 true（失败项可重试，不会产生第二个副本）。 */
  needsRecovery?: boolean;
  /** 临时备份残留（待清理/待恢复）；无异常时为空数组。 */
  temporaryFiles?: TemporaryFileInfo[];
  error?: string;
}

/** 切换前对未完成会话写入的恢复结果。 */
export interface SessionRecoveryReport {
  recovered: number;
  abandoned: number;
  needsRecovery: { operationId: string; reason: string; retryable: boolean }[];
  /** 临时备份残留（待清理/待恢复）；无异常时为空数组。 */
  temporaryFiles?: TemporaryFileInfo[];
}

// ---------------------------------------------------------------------------
// 会话同步（关联组）：预览与执行契约，与 core / Tauri / HTTP 三端同形
// ---------------------------------------------------------------------------

/**
 * 同步判定结果（design §3.2 优先级表）：
 * `identical` 两边一致、`fastForward` 有新增可同步、`ahead` 仅目标账号有更新、
 * `diverge` 两边都改过需显式覆盖、`unknown` 无法确认。
 */
export type SessionSyncVerdict = "identical" | "fastForward" | "ahead" | "diverge" | "unknown";

/** 同步写入模式：只有后端 `availableModes` 里给出的模式才允许提交。 */
export type SessionSyncMode = "fastForward" | "overwrite";

/** 关联组成员（不含正文）：`state` 为 active 时才算该账号的有效成员。 */
export interface SessionLinkMember {
  memberId: string;
  uid: string;
  accountId: string | null;
  sessionId: string;
  state: "active" | "stale" | "superseded";
}

/** 关联组的预览项；`defaultChecked` 与 `availableModes` 是勾选权限的唯一来源。 */
export interface SessionLinkPreviewGroup {
  groupId: string;
  title: string;
  cwd: string;
  verdict: SessionSyncVerdict;
  /** 来源独有记录数（多重集差集，仅用于向用户解释）。 */
  extraA: number;
  /** 目标独有记录数（多重集差集，仅用于向用户解释）。 */
  extraB: number;
  common: number;
  defaultChecked: boolean;
  /** 为空表示该项不可勾选（identical / ahead / unknown / 预览凭据不可用）。 */
  availableModes: SessionSyncMode[];
  reason: string;
  /** 记录数（不是消息数）：不可验证时 source/target 为 0、baseline 为 null。 */
  recordCount: { source: number; target: number; baseline: number | null };
  source: SessionLinkMember | null;
  target: SessionLinkMember | null;
  /** 勾选时必须原样回传的预览凭据；缺失即不可勾选。 */
  previewToken?: string;
}

/** 关联会话预览：`supported` 为 false（或 storeStatus 为 unsupported）时不展示同步区块。 */
export interface SessionLinksPreview {
  supported: boolean;
  storeStatus: "ready" | "missing" | "unavailable" | "unsupported";
  storeError?: string;
  sourceUid: string;
  targetUid: string;
  groups: SessionLinkPreviewGroup[];
}

/** 一条同步选择：与预览凭据绑定，执行时后端会重新校验。 */
export interface SessionSyncSelection {
  groupId: string;
  previewToken: string;
  mode: SessionSyncMode;
}

/** 已同步的关联组（保留目标 sessionId 与标题）。 */
export interface SessionSyncResultItem {
  groupId: string;
  status: "synced";
  verdict: SessionSyncVerdict;
  mode: SessionSyncMode;
  sourceSessionId: string;
  targetSessionId: string;
  recordCount: { source: number; targetBefore: number; target: number };
  updatedAt: number;
  /** 待清理位置（已清理为 null）；旧操作可能仍返回目录路径。 */
  backup: string | null;
  backupManifest: string | null;
  /** 成功后立即清理：cleaned 表示临时备份已回收；pending 表示待下次维护重试。 */
  cleanupState?: SessionBackupCleanupState;
  cleanupError?: string;
  message: string;
}

/** 被跳过的关联组：`reasonCode` 为 previewStale 时说明预览已过期，不得显示为成功。 */
export interface SessionSyncSkippedItem {
  groupId: string;
  status: "skipped";
  reasonCode: string;
  message: string;
  verdict: SessionSyncVerdict | null;
}

/** 同步执行报告；`errors` 里可能是整批被拒（无 groupId）。 */
export interface SessionSyncReport {
  synced: SessionSyncResultItem[];
  skipped: SessionSyncSkippedItem[];
  errors: { groupId?: string; error: string }[];
  /** 仍有未完成/无法安全恢复的会话写入时为 true。 */
  needsRecovery?: boolean;
  /** 临时备份残留（待清理/待恢复）；无异常时为空数组。 */
  temporaryFiles?: TemporaryFileInfo[];
}

/**
 * 应用内通知存档条目：toast 只存活几秒，这里保存最近 100 条供事后回看
 * （支持排障与验收核对，例如切号成功后到底提示了什么）。
 */
export interface AppNotification {
  level: "success" | "error" | "warning" | "info";
  title: string;
  description?: string;
  /** 毫秒时间戳。 */
  at: number;
}

/**
 * 错误日志来源（`~/.wb-switch/error.log` 的 `kind` 字段）：
 * 渲染崩溃 / 未捕获异常或 Promise 拒绝 / 后端错误。
 */
export type ErrorLogKind = "frontend_crash" | "frontend_unhandled" | "backend";

export interface SwitchResult {
  ok: boolean;
  account: string;
  /** 目标账号自身档位；缺省按国内版处理。 */
  variant?: WbVariant;
  backup: string | null;
  sessionCopy?: SessionCopyReport;
  /** 本次的会话同步报告（未勾选同步时不返回）；含跳过与失败原因，不只是成功数。 */
  sessionSync?: SessionSyncReport;
  sessionRecovery?: SessionRecoveryReport;
}

export interface CheckinConfig {
  enabled: boolean;
  /** 关闭自动签到的账号 id；状态展示和刷新附带签到也跳过，主动手动签到不受影响。 */
  excluded_account_ids?: string[];
  /** 签到时间段（"HH:MM"，本地时区）；空串 = 不限制。两端都合法且 start < end 才生效。 */
  checkin_start: string;
  checkin_end: string;
  /** Legacy persisted fields; accepted by the backend but ignored by scheduling. */
  start_hour?: number;
  end_hour?: number;
  keepalive_days: number;
  lazy_refresh_hours: number;
}

export interface CheckinLog {
  ts: number;
  accountId: string | null;
  email: string;
  result: string;
  error?: string;
  /** 该行所属档位；历史日志缺省按国内版处理。 */
  variant?: WbVariant;
}

export interface CheckinResult {
  result: string;
  error?: string;
  /** 国际版签到活动未开放时的业务判定；不写成功日志、不计入失败重试。 */
  inactive?: boolean;
}

export interface TravelConfig {
  enabled: boolean;
}

export type TravelStatusLabel = "untraveled" | "no-buddy" | "traveling" | "finished";

export interface TravelStatus {
  label: TravelStatusLabel;
  rewardCredit: number | null;
  locationName?: string | null;
  arriveAt?: number | null;
}

/** 单个受限模型；`model` 为 null 表示日志里归因不到模型（显示「未知模型」，不猜测）。 */
export interface RateLimitEntry {
  model: string | null;
  /** 官方日志原文给出的恢复时刻（毫秒）。 */
  resetAt: number;
  /** 该事件首次出现的时刻（毫秒）。 */
  firstSeenAt: number;
  /** 去重前的原始命中行数（调试/排查用）。 */
  hitCount: number;
}

/** 一个账号当前受限的全部模型（按 `resetAt` 升序）。 */
export interface AccountRateLimits {
  accountId: string;
  limited: RateLimitEntry[];
}

/** 模型限额台账：一次返回全部账号的当前受限状态（数据来自本机日志）。 */
export interface RateLimitsPayload {
  scannedAt: number;
  /** 固定 2 天，回显便于调试。 */
  windowDays: number;
  /** 只包含至少有一个受限模型的账号。 */
  accounts: AccountRateLimits[];
}

/** 一处客户端 hook 配置的安装状态。 */
export interface RateLimitHookTarget {
  /** 备份标签（codebuddy / workbuddy / workbuddy-ai）。 */
  label: string;
  /** `settings.json` 路径。 */
  path: string;
  /** 该客户端数据根目录是否存在（唯一的存在性判据；不存在则不参与安装）。 */
  exists: boolean;
  /** 该配置里是否已注册本工具的 Stop / FinalStop。 */
  installed: boolean;
}

/**
 * 限额 hook 安装状态：脚本 + 三处客户端配置逐项结果。
 *
 * `installed` = 脚本存在且至少一处配置注册成功；`lastEventAt` 是最近一次由后端
 * 入账的 hook 限额事件时刻（null = 从未收到）。
 */
export interface RateLimitHookStatus {
  scriptPath: string;
  scriptExists: boolean;
  eventsPath: string;
  installed: boolean;
  lastEventAt?: number | null;
  targets: RateLimitHookTarget[];
}

/** 限额监听开关（`~/.wb-switch/rate_limit_config.json`）。 */
export interface RateLimitConfig {
  enabled: boolean;
  /** 用户点过「卸载 hook」→ 启动时不再自动接入；重新点「接入 hook」清除。 */
  hookOptOut: boolean;
  /**
   * 是否扫描两个 CodeBuddy IDE 的日志（默认 true）。
   * IDE 的 429 不触发任何 hook 事件，日志是它唯一的数据源；关闭只影响 IDE 两源，
   * CLI / WorkBuddy 的 hook 实时上报与未接 hook 时的日志兜底不变。
   */
  scanIdeLogs: boolean;
}

export interface AutoRotateConfig {
  enabled: boolean;
  check_interval_minutes: number;
  cooldown_minutes: number;
  min_gap_hours: number;
  min_urgency_hours: number;
  /** 配置键兼容保留：轮换已改用「会话存活门控」，该值不再参与决策，设置页也不再展示。 */
  active_guard_minutes: number;
  min_remaining_credits: number;
}

export interface RotateLog {
  ts: number;
  action: string;
  reason?: string | null;
  from?: { id: string; name?: string | null } | null;
  to?: { id: string; name?: string | null } | null;
}

export interface RotateStatus {
  config: AutoRotateConfig;
  cliConfigured: boolean;
  activeAccountId: string | null;
  activeAccountName: string | null;
  lastCheckAt: number | null;
  lastSwitchAt: number | null;
}

export interface CreditResource {
  packageCode: string | null;
  packageName: string | null;
  total: number;
  remaining: number;
  used: number;
  status: number | null;
  expireAt: number | null;
  expired: boolean;
  expiringSoon: boolean;
}

export interface CreditExpiry {
  ok: boolean;
  accountId?: string | null;
  accountName?: string;
  updatedAt?: number;
  totalCapacity?: number;
  totalRemaining?: number;
  expiringSoonRemaining?: number;
  expiredRemaining?: number;
  soonestExpireAt?: number | null;
  expiringSoon?: boolean;
  expired?: boolean;
  resources?: CreditResource[];
  error?: string;
}

export interface CreditStatsSummary {
  currentRemaining: number;
  currentCapacity: number;
  usageToday: number;
  usage7Days: number;
  usageThisMonth: number;
  todayCheckedInAccounts: number;
  todaySuccess: number;
  todayAlready: number;
  todayFailed: number;
}

export interface CreditStatsDailyPoint {
  date: string;
  usage: number;
  /** 官方用量按模型聚合（全量，不受请求明细条数限制）；本地观察口径下为空 */
  models?: { model: string; requestCount: number; credit: number }[];
}

export interface CreditStatsAccount {
  accountId: string;
  accountName: string;
  isCurrent: boolean;
  currentRemaining: number | null;
  totalCapacity: number | null;
  lastSnapshotAt: number | null;
  usageToday: number;
  usage7Days: number;
  usageThisMonth: number;
  checkedInToday: boolean | null;
  checkinStatusToday: string | null;
  lastCheckinAt: number | null;
  lastCheckinResult: string | null;
  /** 按账号的逐日观察消耗（缺省兼容旧后端）；官方可用时趋势图优先使用官方 daily */
  daily?: CreditStatsDailyPoint[];
  /** 档位标记。后端当前不下发，前端容忍性读取；缺省时回退到按 accountId 的映射表 */
  variant?: WbVariant;
}

export interface CreditStatsUsageEvent {
  kind: "usage";
  ts: number;
  date: string;
  accountId: string;
  accountName: string;
  amount: number;
  /** 档位标记。后端当前不下发，前端容忍性读取；缺省时回退到按 accountId 的映射表 */
  variant?: WbVariant;
}

export interface CreditStatsCheckinEvent {
  kind: "checkin";
  ts: number;
  date: string;
  accountId: string | null;
  accountName: string;
  result: string;
  error?: string | null;
  /** 档位标记。后端当前不下发，前端容忍性读取；缺省时回退到按 accountId 的映射表 */
  variant?: WbVariant;
}

export type CreditStatsEvent = CreditStatsUsageEvent | CreditStatsCheckinEvent;

export type CreditOfficialUsageStatus = "complete" | "partial" | "unavailable";

export interface CreditOfficialUsageSummary {
  usageToday: number;
  usage7Days: number;
  usageThisMonth: number;
}

export interface CreditOfficialUsageModel {
  model: string;
  requestCount: number;
  credit: number;
}

export interface CreditOfficialUsageAccount {
  accountId: string;
  accountName: string;
  ok: boolean;
  requestCount: number;
  detailTruncated: boolean;
  usageToday: number | null;
  usage7Days: number | null;
  usageThisMonth: number | null;
  error?: string | null;
  reportedTotal?: number | null;
  fetchedCount?: number;
  /** 缺省兼容旧后端响应。 */
  models?: CreditOfficialUsageModel[];
  /** 按账号的逐日官方消耗（全量聚合，不受 requests 明细上限影响；缺省兼容旧后端） */
  daily?: CreditStatsDailyPoint[];
}

export interface CreditOfficialUsageRequest {
  accountId: string;
  accountName: string;
  requestId: string;
  credit: number;
  model: string;
  client: string;
  requestTime: string;
}

export interface CreditOfficialUsageError {
  accountId: string;
  accountName: string;
  error: string;
}

export interface CreditOfficialUsage {
  status: CreditOfficialUsageStatus;
  rangeStart: string;
  rangeEnd: string;
  /** 官方用量最近一次采集时间；缓存命中时保持采集当时的时间。 */
  collectedAt?: number;
  summary: CreditOfficialUsageSummary;
  daily: CreditStatsDailyPoint[];
  accounts: CreditOfficialUsageAccount[];
  requests: CreditOfficialUsageRequest[];
  /** 官方全部有效请求按模型汇总；不受 requests 明细上限影响。 */
  models?: CreditOfficialUsageModel[];
  detailLimitPerAccount: number;
  errors: CreditOfficialUsageError[];
}

export interface CreditStatistics {
  generatedAt: number;
  retentionDays: number;
  coverageStartAt: number | null;
  summary: CreditStatsSummary;
  daily: CreditStatsDailyPoint[];
  accounts: CreditStatsAccount[];
  events: CreditStatsEvent[];
  /** 官方接口不可用时仍使用上述本地观察字段；缺省兼容旧后端。 */
  officialUsage?: CreditOfficialUsage;
}

export interface TokenStatsTotals { total: number; input: number; output: number; cacheRead: number; cacheWrite: number; uncachedInput: number; records: number; cacheHitRate: number | null; }
export interface TokenStatsGroup extends TokenStatsTotals { key: string; title?: string | null; project?: string; sessionId?: string; }
/** 一次模型调用的明细行；`total = input + output + cacheWrite`，`uncachedInput = max(0, input - cacheRead)`，`thinking` 是 `output` 中思考过程的 token 数（回复内容 = max(0, output - thinking)），均与聚合口径一致。 */
export interface TokenStatsRequestRow { timestamp: number; model: string; project: string; sessionId: string; title?: string | null; input: number; output: number; cacheRead: number; cacheWrite: number; uncachedInput: number; thinking: number; total: number; }
/** `workbuddy-ai` 为国际版本地数据源，与国内版分开统计，数据源缺失时为空集。 */
export interface TokenStatsSource { source: "workbuddy" | "workbuddy-ai" | "codebuddy-cli" | "codebuddy-ide"; summary: TokenStatsTotals; models: TokenStatsGroup[]; projects: TokenStatsGroup[]; sessions: TokenStatsGroup[]; daily: TokenStatsGroup[]; /** Optional model-specific daily series for trend filtering. */ dailyByModel?: Record<string, TokenStatsGroup[]>; /** 仅 CodeBuddy CLI 来源返回的最近请求明细；旧后端或缺失时按空数组处理。 */ requests?: TokenStatsRequestRow[]; hours: TokenStatsGroup[]; filesScanned: number; parseErrors: number; coverageStartAt?: number | null; coverageEndAt?: number | null; }
export interface TokenStatistics { generatedAt: number; rangeDays?: number | null; sources: TokenStatsSource[]; }

export interface CodeBuddyCliStatus {
  configured: boolean;
  authMode?: "settings-env" | "api-key-helper";
  environmentOverride?: boolean;
  settingsPresent: boolean;
  helperPresent: boolean;
  helperSupportsAccountIds: boolean;
  helperCurrent?: boolean;
  migrationRequired?: boolean;
  syncPending?: boolean;
  activeIndex: number | null;
  activeAccountId: string | null;
  activeAccountName: string | null;
  /** 当前 CLI 账号所属档位；尚未接入时缺省。 */
  activeAccountVariant?: WbVariant | null;
  accountCount: number;
  statePath: string;
}

export interface CodeBuddyCliSwitchResult {
  ok: boolean;
  configured: boolean;
  synced: boolean;
  verified?: boolean;
  authMode?: "settings-env" | "api-key-helper";
  activeIndex?: number;
  activeAccountId?: string;
  source?: string;
  skipped?: boolean;
  regionChanged?: boolean;
  cliClosed?: boolean;
  closedProcessCount?: number;
  message?: string;
  error?: string;
}

export interface CodeBuddyCliInstallResult {
  ok: boolean;
  configured: boolean;
  helperPresent: boolean;
  helperSupportsAccountIds: boolean;
  verified?: boolean;
  authMode?: "settings-env" | "api-key-helper";
  message?: string;
  error?: string;
}

export interface GithubConfig {
  owner?: string;
  repo?: string;
  proxy?: string;
}

export interface UpdateInfo {
  ok: boolean;
  current?: string;
  latest?: string;
  latestTag?: string;
  hasUpdate?: boolean;
  releaseName?: string;
  releaseUrl?: string;
  publishedAt?: string;
  error?: string;
  message?: string;
}

/** 统一更新服务的阶段（Rust 状态机，经 `update-state` 事件推送）。 */
export type UpdatePhase =
  | "idle"
  | "checking"
  | "upToDate"
  | "available"
  | "downloading"
  | "readyToRestart"
  | "error";

/**
 * 更新状态快照：托盘菜单与前端弹窗显示同一阶段（单一真相源在 Rust）。
 *
 * `latest` 无 `v` 前缀；`percent` 在下载总量未知时为 null；`message` 是错误 / 提示文案。
 */
export interface UpdateSnapshot {
  phase: UpdatePhase;
  latest: string | null;
  percent: number | null;
  message: string | null;
  checkedAt: number | null;
}

/** CodeBuddy CN IDE（桌面客户端）状态；与 CodeBuddy CLI 独立。 */
export interface CodeBuddyCnIdeStatus {
  installed: boolean;
  running: boolean;
  dataDir: string | null;
  dbPath: string | null;
  dbExists: boolean;
  appPath: string | null;
  activeAccountId: string | null;
  activeAccountName: string | null;
  detectedFrom?: string;
  statePath?: string;
}

export interface CodeBuddyCnIdeSwitchResult {
  ok: boolean;
  account: string;
  accountId: string;
  dbPath?: string;
  restarted?: boolean;
  message?: string;
}

/** VS Code 内 CodeBuddy 扩展（tencent-cloud.coding-copilot）状态；与 CN IDE / CLI 独立。 */
export interface VscodeExtStatus {
  /** VS Code 用户数据目录是否存在。 */
  installed: boolean;
  /** CodeBuddy 扩展是否已安装（globalStorage/<extensionId> 存在）。 */
  extensionInstalled: boolean;
  running: boolean;
  /**
   * 是否存在扩展登录态（`state.vscdb` 里有会话 secret 行）。
   *
   * 只读查询、不解密；查询失败或文件不存在时为 false。仅用于提示文案，
   * 不参与切换判定（false 时切换按新会话写入，同样可用）。
   */
  loggedIn: boolean;
  dataDir: string | null;
  dbPath: string | null;
  dbExists: boolean;
  activeAccountId: string | null;
  activeAccountName: string | null;
  detectedFrom?: string;
  statePath?: string;
}

export interface VscodeExtSwitchResult {
  ok: boolean;
  account: string;
  accountId: string;
  dbPath?: string;
  /** 本次是否真的执行了「关闭并重新打开 VS Code」（切换前未运行时为 false）。 */
  restarted?: boolean;
  /** 本次切换是否由 wb-switch 关闭了 VS Code（为 false 时表示编辑器本来没运行）。 */
  closedByUs?: boolean;
  /** 切换前是否读到了既有会话；false = 扩展未登录，按新会话载荷写入。 */
  existingSession?: boolean;
  /**
   * 后端生成的生效说明：区分「已重新打开 VS Code」「自动重开失败（含原因）」
   * 「本来未运行」三种情形，不再出现「重载窗口生效」。
   */
  message?: string;
  /** 切换时复制会话的结果（未勾选复制时不返回）。 */
  sessionCopy?: VscodeSessionCopyResult;
  /** 切换时同步关联会话的结果（未勾选同步时不返回）。 */
  sessionSync?: VscodeSessionSyncReport;
}

/** VS Code 扩展的一条可复制会话。 */
export interface VscodeSession {
  /** 会话 id（32 位小写 hex）。 */
  id: string;
  /** 工作区目录名 md5(工作区)（32 位小写 hex，不可反解为路径）。 */
  workspaceHash: string;
  /** 会话标题；无标题时为 "(无标题)"。 */
  title: string;
  /** 最近消息时间（epoch 毫秒，0 表示未知）。 */
  updatedAt: number;
  /** 会话类型（如 "craft"）。 */
  type: string;
  /** 是否包含正文消息。 */
  hasHistory: boolean;
}

/** 复制某项会话的引用（工作区 hash + 会话 id）。 */
export interface VscodeSessionRef {
  workspaceHash: string;
  conversationId: string;
}

/** VS Code 扩展会话复制结果。 */
export interface VscodeSessionCopyResult {
  sourceUid: string | null;
  targetUid: string;
  copied: { workspaceHash: string; oldId: string; newId: string; messages: number }[];
  errors?: { workspaceHash: string; conversationId: string; error: string }[];
  /** 索引备份根目录（便于用户找回）。 */
  backup?: string;
  /**
   * 已复制但未能建立关联的条目（复制成功、登记失败；登记失败不回滚复制）。
   * 前端据此提示「已复制但未建立关联」，不得静默当成成功。
   */
  linkErrors?: { workspaceHash?: string; conversationId?: string; error: string }[];
}

/** VS Code 插件侧「关联会话」的同步结果项（与 core 报告同形）。 */
export interface VscodeSessionSyncResultItem {
  groupId: string;
  status: "synced";
  verdict: SessionSyncVerdict;
  mode: SessionSyncMode;
  /** 记录数（条）：source 为来源当前条数，targetBefore/target 为副本写入前后条数。 */
  recordCount: { source: number; targetBefore: number; target: number };
  /** 附件合并结果（快进模式补入的附件）。 */
  assets: { copied: number; overwritten: number };
  /** 本次备份目录（快进为索引备份，覆盖为整目录备份）。 */
  backup: string;
  message: string;
}

/** 被跳过的同步项（`reasonCode` 为 previewStale 时说明预览已过期，不得显示为成功）。 */
export interface VscodeSessionSyncSkippedItem {
  groupId: string;
  status: "skipped";
  reasonCode: string;
  message: string;
  verdict: SessionSyncVerdict | null;
}

/** VS Code 插件侧「关联会话」同步报告；`errors` 里可能是整批被拒（无 groupId）。 */
export interface VscodeSessionSyncReport {
  synced: VscodeSessionSyncResultItem[];
  skipped: VscodeSessionSyncSkippedItem[];
  errors: { groupId?: string; error: string }[];
}

// ---------------------------------------------------------------------------
// TRAE 子系统（trae-mate 合并）：与 crates/trae-core/src/models.rs serde camelCase 对齐
// ---------------------------------------------------------------------------

/** TRAE 账号来源：desktop = 桌面实例导入；jwt = 手动录入 JWT（参考模式）。 */
export type TraeSource = "desktop" | "jwt";

/** TRAE 账号（脱敏 PublicAccount 视图，不含加密凭据）。 */
export interface TraeAccount {
  id: string;
  name: string;
  cookie: string;
  createdAt: number;
  lastCheckinAt: number | null;
  /** "success" | "failed" | "pending" */
  lastCheckinResult: string | null;
  lastCheckinMessage: string | null;
  points: number | null;
  enabled: boolean;
  desktopUserId: string | null;
  /** "valid" | "expiring" | "expired" */
  credentialStatus: string | null;
  dataDir: string | null;
  machineId: string | null;
  source: TraeSource;
  userId: string | null;
  jwtExpTimestamp: number | null;
  /** "ok" | "warn" | "expired" | "unknown" */
  jwtStatus: string | null;
  hasRefreshToken: boolean;
  jwtAutoRefresh: boolean;
  remainingCredits: number | null;
  creditsExpireAt: number | null;
  deviceIdMasked: string | null;
  cooldownType: string | null;
  /** Unix 秒；SessionDead=9999999999 表示永久。 */
  cooldownUntil: number | null;
  cooldownReason: string | null;
  checkedToday: boolean | null;
}

/** TRAE 签到日志（后端已按新的在前排序）。 */
export interface TraeCheckinLog {
  id: string;
  accountId: string;
  accountName: string;
  time: number;
  /** "success" | "failed" */
  result: string;
  message: string;
  pointsGained?: number | null;
}

/** TRAE 应用设置（auto_checkin_interval_min 最短 3，由后端钳制）。 */
export interface TraeSettings {
  autoCheckin: boolean;
  /** "HH:mm" */
  checkinTime: string;
  retryCount: number;
  /** 秒 */
  retryDelay: number;
  /** 定时签到时账号间执行间隔（分钟），最短 3 */
  autoCheckinIntervalMin: number;
  notifyOnSuccess: boolean;
  notifyOnFailed: boolean;
  launchAtLogin: boolean;
}

/** 单次签到结果。 */
export interface TraeCheckinResult {
  success: boolean;
  message: string;
  points?: number;
}

/** 积分查询结果。 */
export interface TraePointsResult {
  success: boolean;
  message: string;
  totalPoints?: number;
}

/** 多开启动结果（dataDir/machineId 由后端回写账号）。 */
export interface TraeLaunchResult {
  dataDir: string;
  machineId: string;
  launched: boolean;
}

/** 账号实例运行状态：main = 主实例，tool = 本工具启动的独立实例，none = 未运行。 */
export interface TraeInstanceState {
  running: boolean;
  source: "main" | "tool" | "none";
  isMainAccount: boolean;
}

/** 已有多开/登录临时目录的扫描结果。 */
export interface TraeInstanceDirInfo {
  dataDir: string;
  userId: string;
  accountName: string;
  /** token 过期时间（毫秒），0 表示未知。 */
  expiresAt: number;
  /** 含设备签名密钥（可刷新 token）。 */
  hasSigningKey: boolean;
  running: boolean;
  /** 已被应用内账号绑定。 */
  bound: boolean;
}

/** JWT 解析预览（不校验签名，仅本地展示）。 */
export interface TraeJwtInfo {
  userId: string | null;
  expHours: number | null;
  expTimestamp: number | null;
}

/** 每日积分快照（积分看板三线趋势数据源）。 */
export interface TraeCreditsDailySnapshot {
  date: string;
  total: number;
  earned: number;
  consumed: number;
}

/** 旧 TraeMate 数据迁移报告（幂等，可重复触发）。 */
export interface TraeMigrationReport {
  detected: boolean;
  accountsImported: number;
  logsImported: number;
  files: string[];
  skipped: string[];
  exePathMigrated: boolean;
  legacyProcessRunning: boolean;
}

/** 可复制会话列表。 */
export interface VscodeSessionList {
  sourceUid: string | null;
  sessions: VscodeSession[];
  /** 无法解析（损坏）的工作区索引数量。 */
  skipped?: number;
  /**
   * 扩展数据根目录；`null` 表示未找到（与「有目录但没有会话」区分）。
   * 可选：旧后端不返回该字段时为 `undefined`，前端按旧文案处理。
   */
  dataRoot?: string | null;
}

// ---------------------------------------------------------------------------
// Qoder / ZCode 领取子系统（creditdaddy 合并）：与 crates/credit-core/src/models.rs serde camelCase 对齐
// ---------------------------------------------------------------------------

/** 领取/签到结果（协议口径，与 CreditDaddy checkinOne/zcodeAutoClaim 一致）。 */
export type ClaimOutcome =
  | "checked-in"
  | "already"
  | "no-activity"
  | "failed"
  | "need-captcha";

/** Qoder 区域：intl = 国际版，cn = 国内版。 */
export type CreditRegion = "cn" | "intl";

/**
 * Qoder 额度缓存（/api/v2/quota/usage 原始 JSON，credit-core 存为 Value）。
 * 常用字段：userQuota / addOnQuota / orgResourcePackage 各含 total/used/remaining。
 */
export interface QoderQuotaUsage {
  userQuota?: { total?: number; used?: number; remaining?: number } | null;
  addOnQuota?: { total?: number; used?: number; remaining?: number } | null;
  orgResourcePackage?: { total?: number; used?: number; remaining?: number } | null;
  isQuotaExceeded?: boolean;
  [key: string]: unknown;
}

/** Qoder 账号（credit-core QoderAccount；字段缺失容忍旧数据）。 */
export interface QoderAccount {
  id: string;
  name: string;
  token: string;
  refreshToken?: string | null;
  /** 首次领取后从 campaigns 响应回写。 */
  userId?: string | null;
  email?: string | null;
  region: CreditRegion;
  enabled: boolean;
  createdAt?: number;
  /** 凭据过期时间（ISO 字符串，本机客户端导入时携带）。 */
  expiresAt?: string | null;
  /** "manual" 手动录入 | "local-app" 本机导入 | "device" 浏览器授权。 */
  source?: string;
  lastClaimAt?: number | null;
  lastResult?: ClaimOutcome | string | null;
  lastMessage?: string | null;
  lastClaimedAmount?: number | null;
  /** 额度查询原始 JSON 缓存（见 QoderQuotaUsage）。 */
  quota?: QoderQuotaUsage | null;
  quotaUpdatedAt?: number | null;
  /** 冷却截止（Unix 毫秒；null = 不在冷却）。 */
  cooldownUntil?: number | null;
  cooldownReason?: string | null;
}

/** ZCode 账号（credit-core ZCodeAccount；region 为预留字段，固定 intl）。 */
export interface ZcodeAccount {
  id: string;
  name: string;
  /** 手动录入为直接 token；本机导入为标记 "zcode-creds:<uid>"（真实凭据在 credentials 快照）。 */
  token: string;
  refreshToken?: string | null;
  userId?: string | null;
  email?: string | null;
  region?: string;
  enabled: boolean;
  createdAt?: number;
  source?: string;
  lastClaimAt?: number | null;
  lastResult?: ClaimOutcome | string | null;
  lastMessage?: string | null;
  lastClaimedAmount?: number | null;
  /** QuotaSnapshot JSON 缓存（见 ZcodeQuotaSnapshot）。 */
  quota?: ZcodeQuotaSnapshot | null;
  quotaUpdatedAt?: number | null;
  cooldownUntil?: number | null;
  cooldownReason?: string | null;
  /** 本机导入的 credentials.json 快照（各字段保持 enc:v1 密文）。 */
  credentials?: unknown;
  config?: unknown;
  deviceMid?: string | null;
  canonicalHash?: string | null;
}

/** ZCode 统一额度快照（credit-core zcode::client::QuotaSnapshot）。 */
export interface ZcodeQuotaSnapshot {
  /** "bigmodel" | "zcode.z.ai" | "" */
  source: string;
  total: number;
  used: number;
  remaining: number;
  unit: string;
  plan?: string | null;
  planExpiresAt?: string | null;
  parts: {
    name: string;
    /** 次 / 分钟 / Token；空串表示纯数值。 */
    unit: string;
    total: number;
    used: number;
    remaining: number;
    expiresAt?: string | null;
  }[];
  empty: boolean;
}

/** Qoder 设置（intervalMin 最短 30，由后端钳制）。 */
export interface QoderSettings {
  autoClaimEnabled: boolean;
  intervalMin: number;
  /** 新账号默认区域。 */
  region: CreditRegion;
}

/** ZCode 设置。 */
export interface ZcodeSettings {
  autoClaimEnabled: boolean;
  intervalMin: number;
}

/** 领取/签到日志（credit-core LogEntry；后端已按新的在前排序）。 */
export interface CreditLogEntry {
  time: number;
  /** "qoder" | "zcode" */
  platform: string;
  accountId: string;
  accountName: string;
  /** ClaimOutcome 字符串（容忍后端扩展）。 */
  result: string;
  message: string;
}

/** Qoder 单账号领取结果（credit-core QoderCheckinOutcome）。 */
export interface QoderCheckinResult {
  outcome: ClaimOutcome | string;
  message: string;
  /** 本次领取到的 Credits 总额。 */
  claimedAmount?: number;
  uid?: string | null;
  /** 本次是否用上了设备风控身份。 */
  risk?: boolean;
}

/** ZCode 单账号领取结果（credit-core ZcodeClaimOutcome）。 */
export interface ZcodeClaimResult {
  outcome: ClaimOutcome | string;
  message: string;
  claims?: unknown[];
}

/** 一键领取返回的 [账号ID, 结果] 对（与 trae_checkin_all 同形）。 */
export type QoderCheckinAllResult = [string, QoderCheckinResult][];
export type ZcodeClaimAllResult = [string, ZcodeClaimResult][];

// ---------------------------------------------------------------------------
// 灵犀签到子系统（金山 Lingxi）：与 crates/credit-core/src/lingxi/mod.rs serde camelCase 对齐。
// checkinUrl 与 Cookie 均由用户从浏览器 F12 抓包手动获取，无本地凭据检测。
// ---------------------------------------------------------------------------

/** 灵犀签到结果口径（credit-core lingxi::Outcome；unknown 表示响应文本无法判定，稍后重试）。 */
export type LingxiOutcome = "success" | "already" | "unknown" | "failed" | "skipped";

/** 灵犀账号（credit-core LingxiAccount；字段缺失容忍旧数据）。 */
export interface LingxiAccount {
  id: string;
  name: string;
  /** 签到接口地址（浏览器 F12 抓包获取，每账号独立）。 */
  checkinUrl: string;
  /** 签到请求的 Cookie 请求头（与 checkinUrl 同源抓包）。 */
  cookie: string;
  enabled: boolean;
  createdAt?: number;
  lastCheckinAt?: number | null;
  lastResult?: LingxiOutcome | string | null;
  lastMessage?: string | null;
  /** 最近成功签到日期 YYYY-MM-DD（Asia/Shanghai），当日幂等跳过依据。 */
  lastSuccessDate?: string | null;
}

/** 灵犀设置（checkinTimes 为每日定点 HH:MM 列表，Asia/Shanghai；列表为空即不调度）。 */
export interface LingxiSettings {
  checkinTimes: string[];
  notifyOnSuccess: boolean;
  notifyOnFailed: boolean;
}

/** 灵犀单账号签到结果（credit-core LingxiCheckinOutcome）。 */
export interface LingxiCheckinResult {
  outcome: LingxiOutcome | string;
  message: string;
}

/** 一键签到返回项：桌面端键名 accountId / webui 键名 id（outcome/message 相同）。 */
export interface LingxiCheckinAllItem extends LingxiCheckinResult {
  accountId?: string;
  id?: string;
}

/** 一键签到返回（全部启用账号；当日已成功 → skipped）。 */
export type LingxiCheckinAllResult = LingxiCheckinAllItem[];

