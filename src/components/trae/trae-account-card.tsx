import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { KeyRound, Loader2, RefreshCw } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import type { TraeAccount, TraeInstanceState } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useTraeStore } from "@/stores/trae";

/** 永久冷却的特殊时间戳（SessionDead，见 trae-core cooldown）。 */
const PERMANENT_COOLDOWN_UNTIL = 9999999999;

function formatDateTime(ms: number): string {
  const d = new Date(ms);
  const pad = (v: number) => String(v).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

interface TraeAccountCardProps {
  account: TraeAccount;
  /** 删除走页面级确认弹窗。 */
  onDelete: (account: TraeAccount) => void;
}

/**
 * TRAE 账号卡（对应 trae-mate AccountCard.vue 全部功能）：
 * 启用开关、积分/剩余积分/过期时间、今日已签、凭据/JWT 状态徽标、冷却倒计时、
 * 设备标识脱敏、签到、启动实例/聚焦（三态）、刷新凭证、JWT 手动刷新、
 * 冷却清除、设备重置、编辑（名称/启用/JWT/refreshToken）、删除（页面确认）。
 */
export function TraeAccountCard({ account, onDelete }: TraeAccountCardProps) {
  const checkinOne = useTraeStore((s) => s.checkinOne);
  const refreshAccounts = useTraeStore((s) => s.refreshAccounts);
  const isJwt = account.source === "jwt";
  /** 冷却 / 设备 / JWT 相关操作统一按 userId 定位（jwt 账号取 userId，desktop 取 desktopUserId）。 */
  const userId = account.userId ?? account.desktopUserId;

  const [instance, setInstance] = useState<TraeInstanceState>({
    running: false,
    source: "none",
    isMainAccount: false,
  });
  const [checkingIn, setCheckingIn] = useState(false);
  const [launching, setLaunching] = useState(false);
  const [refreshingCred, setRefreshingCred] = useState(false);
  const [refreshingJwt, setRefreshingJwt] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  /** 编辑草稿：JWT / refreshToken 留空 = 不修改。 */
  const [draftName, setDraftName] = useState(account.name);
  const [draftJwt, setDraftJwt] = useState("");
  const [draftRefreshToken, setDraftRefreshToken] = useState("");
  const [savingEdit, setSavingEdit] = useState(false);
  /** 冷却倒计时的每 30s 时钟（跨过 until 自动恢复显示）。 */
  const [now, setNow] = useState(() => Date.now());

  // 挂载时查询实例三态（未运行 / 主实例 / 工具实例）
  useEffect(() => {
    let cancelled = false;
    void api
      .traeGetAccountInstanceState(account.id)
      .then((state) => {
        if (!cancelled) setInstance(state);
      })
      .catch(() => {
        /* 查询失败按未运行处理 */
      });
    return () => {
      cancelled = true;
    };
  }, [account.id]);

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  const handleToggleEnabled = useCallback(
    async (enabled: boolean) => {
      try {
        await api.traeUpdateAccount(account.id, { enabled });
        await refreshAccounts();
      } catch (e) {
        toast.error("更新账号失败", { description: api.asError(e) });
      }
    },
    [account.id, refreshAccounts],
  );

  async function handleCheckin() {
    if (checkingIn) return;
    setCheckingIn(true);
    try {
      const result = await checkinOne(account.id);
      let message = result.message || (result.success ? "签到成功" : "签到失败");
      if (message.includes("频繁")) message += "，请稍候一分钟再试";
      if (result.success) toast.success(message, { description: account.name });
      else toast.error(message, { description: account.name });
    } catch (e) {
      toast.error("签到失败", { description: api.asError(e) });
    } finally {
      setCheckingIn(false);
    }
  }

  async function handleLaunch() {
    if (launching) return;
    setLaunching(true);
    try {
      await api.traeLaunchAccountMulti(account.id);
      setInstance((prev) => ({ ...prev, running: true, source: "tool" }));
      toast.success("实例已启动", { description: account.name });
    } catch (e) {
      toast.error("启动失败", { description: api.asError(e) });
      // 失败可能因状态过时（如该账号已在主实例运行），重新查询修正显示
      try {
        setInstance(await api.traeGetAccountInstanceState(account.id));
      } catch {
        /* 忽略 */
      }
    } finally {
      setLaunching(false);
    }
  }

  async function handleFocus() {
    try {
      await api.traeFocusAccountInstance(account.id);
    } catch (e) {
      toast.error("聚焦失败", { description: api.asError(e) });
    }
  }

  // 回读最新凭证：打开 TRAE 实例让它自行刷新 token，点此同步到应用
  async function handleRefreshCredential() {
    if (refreshingCred) return;
    setRefreshingCred(true);
    try {
      await api.traeRefreshAccountCredential(account.id);
      await refreshAccounts();
      toast.success("已回读 TRAE 最新凭证", { description: account.name });
    } catch (e) {
      toast.error("回读凭证失败", { description: api.asError(e) });
    } finally {
      setRefreshingCred(false);
    }
  }

  // JWT 账号手动刷新（refresh_token -> ExchangeToken）
  async function handleRefreshJwt() {
    if (refreshingJwt || !userId) return;
    setRefreshingJwt(true);
    try {
      await api.traeRefreshJwtAccount(userId);
      await refreshAccounts();
      toast.success("JWT 已刷新", { description: account.name });
    } catch (e) {
      toast.error("JWT 刷新失败", { description: api.asError(e) });
    } finally {
      setRefreshingJwt(false);
    }
  }

  async function handleClearCooldown() {
    if (!userId) return;
    try {
      await api.traeCooldownClear(userId);
      await refreshAccounts();
      toast.success("冷却已清除", { description: account.name });
    } catch (e) {
      toast.error("清除冷却失败", { description: api.asError(e) });
    }
  }

  async function handleDeviceReset() {
    if (!userId) return;
    try {
      await api.traeDeviceReset(userId);
      await refreshAccounts();
      toast.success("设备身份已重置，下次签到将重新派生", { description: account.name });
    } catch (e) {
      toast.error("设备重置失败", { description: api.asError(e) });
    }
  }

  async function saveEdit() {
    const name = draftName.trim();
    if (!name) {
      toast.error("账号名称不能为空");
      return;
    }
    setSavingEdit(true);
    try {
      // jwt / refreshToken 留空 = 不修改（后端对空 jwt 跳过、空 refreshToken 视为清除，这里统一不传）
      const updates: Record<string, unknown> = { name, enabled: account.enabled };
      if (isJwt && draftJwt.trim()) updates.jwt = draftJwt.trim();
      if (isJwt && draftRefreshToken.trim()) updates.refreshToken = draftRefreshToken.trim();
      await api.traeUpdateAccount(account.id, updates);
      await refreshAccounts();
      setEditOpen(false);
      setDraftJwt("");
      setDraftRefreshToken("");
      toast.success("账号已更新");
    } catch (e) {
      toast.error("更新账号失败", { description: api.asError(e) });
    } finally {
      setSavingEdit(false);
    }
  }

  // ---- 展示态推导（与 AccountCard.vue 同语义） ----
  const statusText = (() => {
    if (!account.enabled) return "已停用";
    if (account.checkedToday === true) return "今日已签";
    if (!account.lastCheckinAt) return "未签到";
    const today = new Date().toDateString();
    if (new Date(account.lastCheckinAt).toDateString() === today) {
      return account.lastCheckinResult === "success" ? "今日已签" : "今日失败";
    }
    return "待签到";
  })();
  const statusVariant: "secondary" | "outline" | "default" | "destructive" = (() => {
    if (!account.enabled) return "secondary";
    if (account.checkedToday === true) return "default";
    if (!account.lastCheckinAt) return "outline";
    const today = new Date().toDateString();
    if (new Date(account.lastCheckinAt).toDateString() === today) {
      return account.lastCheckinResult === "success" ? "default" : "destructive";
    }
    return "outline";
  })();

  const credentialBadge = (() => {
    if (isJwt) {
      const status = account.jwtStatus;
      const text =
        status === "expired"
          ? "JWT 已过期"
          : status === "unknown"
            ? "JWT 未知"
            : status === "warn" && account.jwtExpTimestamp
              ? `JWT ${((account.jwtExpTimestamp - now / 1000) / 3600).toFixed(1)}h 后过期`
              : "JWT 有效";
      const variant =
        status === "expired"
          ? "destructive"
          : status === "warn"
            ? "outline"
            : status === "ok"
              ? "default"
              : "secondary";
      return { text, variant } as const;
    }
    const status = account.credentialStatus;
    const text =
      status === "expired"
        ? "凭证已过期"
        : status === "expiring"
          ? "凭证即将过期"
          : status
            ? "凭证有效"
            : "凭证未知";
    const variant =
      status === "expired" ? "destructive" : status === "expiring" ? "outline" : "default";
    return { text, variant } as const;
  })();

  const cooldownText = (() => {
    const until = account.cooldownUntil ?? 0;
    if (until >= PERMANENT_COOLDOWN_UNTIL) return "永久冷却（需重新登录/刷新凭证）";
    const mins = Math.ceil((until - now / 1000) / 60);
    return `剩余 ${Math.max(mins, 0)} 分钟`;
  })();

  return (
    <div
      className={cn(
        "flex min-w-0 flex-col rounded-xl border border-border bg-card p-5 shadow-sm",
        !account.enabled && "opacity-55",
      )}
    >
      {/* 头部：头像 + 名称 + 状态徽标 + 启用开关 */}
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-center gap-3">
          <div className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-primary text-sm font-bold text-primary-foreground">
            {account.name.charAt(0).toUpperCase() || "?"}
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <span className="truncate text-sm font-semibold">{account.name}</span>
              {instance.isMainAccount && (
                <Badge className="h-5 rounded-md px-1.5 text-[10px]">主账号</Badge>
              )}
              {isJwt && (
                <Badge variant="outline" className="h-5 gap-0.5 rounded-md px-1.5 text-[10px]">
                  <KeyRound className="size-2.5" />
                  JWT
                </Badge>
              )}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-1.5">
              <Badge variant={statusVariant} className="h-5 rounded-md px-1.5 text-[10px]">
                {statusText}
              </Badge>
              <Badge
                variant={credentialBadge.variant}
                className="h-5 rounded-md px-1.5 text-[10px]"
              >
                {credentialBadge.text}
              </Badge>
            </div>
          </div>
        </div>
        <Switch
          checked={account.enabled}
          onCheckedChange={(checked) => void handleToggleEnabled(checked)}
          aria-label={`启用 ${account.name}`}
        />
      </div>

      {/* 信息区 */}
      <div className="mt-4 space-y-1.5 text-[13px]">
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted-foreground">上次签到</span>
          <span className="font-medium">
            {account.lastCheckinAt ? formatDateTime(account.lastCheckinAt) : "从未签到"}
          </span>
        </div>
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted-foreground">总积分</span>
          <span className="font-semibold text-amber-600 dark:text-amber-500">
            {account.points ?? 0} 分
          </span>
        </div>
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted-foreground">剩余积分</span>
          <span className="font-medium">
            {account.remainingCredits == null ? "—" : account.remainingCredits}
            {account.creditsExpireAt ? (
              <span className="ml-1.5 text-xs text-muted-foreground">
                {formatDateTime(account.creditsExpireAt)} 到期
              </span>
            ) : null}
          </span>
        </div>
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted-foreground">设备标识</span>
          <span className="flex items-center gap-1 font-mono text-xs">
            {account.deviceIdMasked || "—"}
            {userId && (
              <button
                type="button"
                className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
                title="重置伪设备身份（下次签到重新派生）"
                onClick={() => void handleDeviceReset()}
              >
                <RefreshCw className="size-3" />
              </button>
            )}
          </span>
        </div>
        {/* 凭证行（desktop 账号）：状态 + 手动回读小按钮 */}
        {!isJwt && (
          <div className="flex items-center justify-between gap-2">
            <span className="text-muted-foreground">桌面凭证</span>
            <span className="flex items-center gap-1">
              <span
                className={cn(
                  "text-xs font-medium",
                  account.credentialStatus === "expired"
                    ? "text-red-600 dark:text-red-500"
                    : account.credentialStatus === "expiring"
                      ? "text-amber-600 dark:text-amber-500"
                      : "text-emerald-600 dark:text-emerald-500",
                )}
              >
                {account.credentialStatus === "expired"
                  ? "已失效，打开 TRAE 实例刷新"
                  : account.credentialStatus === "expiring"
                    ? "即将过期，打开 TRAE 续期"
                    : "有效"}
              </span>
              <button
                type="button"
                className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
                title="回读 TRAE 最新凭证（打开 TRAE 让它自行刷新后，点此同步）"
                onClick={() => void handleRefreshCredential()}
                disabled={refreshingCred}
              >
                {refreshingCred ? (
                  <Loader2 className="size-3 animate-spin" />
                ) : (
                  <RefreshCw className="size-3" />
                )}
              </button>
            </span>
          </div>
        )}
        {/* JWT 行：状态 + 手动刷新小按钮 */}
        {isJwt && (
          <div className="flex items-center justify-between gap-2">
            <span className="text-muted-foreground">JWT</span>
            <span className="flex items-center gap-1">
              <span
                className={cn(
                  "text-xs font-medium",
                  account.jwtStatus === "expired"
                    ? "text-red-600 dark:text-red-500"
                    : account.jwtStatus === "warn"
                      ? "text-amber-600 dark:text-amber-500"
                      : account.jwtStatus === "ok"
                        ? "text-emerald-600 dark:text-emerald-500"
                        : "text-muted-foreground",
                )}
              >
                {account.hasRefreshToken ? "可自动刷新" : "无 refresh_token"}
              </span>
              {account.hasRefreshToken && (
                <button
                  type="button"
                  className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
                  title="用 refresh_token 自动刷新 JWT"
                  onClick={() => void handleRefreshJwt()}
                  disabled={refreshingJwt}
                >
                  {refreshingJwt ? (
                    <Loader2 className="size-3 animate-spin" />
                  ) : (
                    <RefreshCw className="size-3" />
                  )}
                </button>
              )}
            </span>
          </div>
        )}
        {/* 冷却行：倒计时 + 清除按钮 */}
        {account.cooldownType && (
          <div className="flex items-center justify-between gap-2">
            <span className="text-muted-foreground">冷却中</span>
            <span className="flex items-center gap-1">
              <span className="text-xs font-medium text-red-600 dark:text-red-500">
                {account.cooldownType}
                {account.cooldownReason ? `：${account.cooldownReason}` : ""} · {cooldownText}
              </span>
              <button
                type="button"
                className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
                title="清除冷却状态"
                onClick={() => void handleClearCooldown()}
              >
                <RefreshCw className="size-3" />
              </button>
            </span>
          </div>
        )}
      </div>

      {/* 操作区 */}
      <div className="mt-4 flex flex-wrap gap-2">
        <Button
          size="sm"
          className="flex-1"
          onClick={() => void handleCheckin()}
          disabled={checkingIn || !account.enabled || account.checkedToday === true}
          title={account.checkedToday === true ? "今日已签到，明日自动继续" : undefined}
        >
          {checkingIn && <Loader2 className="animate-spin" />}
          {checkingIn ? "签到中" : account.checkedToday === true ? "今日已签到" : "立即签到"}
        </Button>
        {!isJwt &&
          (instance.source === "none" ? (
            <Button
              size="sm"
              variant="outline"
              className="flex-1"
              onClick={() => void handleLaunch()}
              disabled={launching}
              title={account.dataDir ? "启动该账号的独立实例" : "首次启动：创建独立实例并启动"}
            >
              {launching && <Loader2 className="animate-spin" />}
              {launching ? "启动中" : "启动"}
            </Button>
          ) : (
            <Button
              size="sm"
              variant="outline"
              className="flex-1"
              onClick={() => void handleFocus()}
              title={
                instance.source === "main"
                  ? "该账号在主实例运行，点击聚焦主实例窗口"
                  : "该账号工具实例已运行，点击聚焦其窗口"
              }
            >
              聚焦
            </Button>
          ))}
        <Button
          size="sm"
          variant="outline"
          className="flex-1"
          onClick={() => {
            setDraftName(account.name);
            setDraftJwt("");
            setDraftRefreshToken("");
            setEditOpen(true);
          }}
        >
          编辑
        </Button>
        <Button size="sm" variant="outline" className="flex-1" onClick={() => onDelete(account)}>
          删除
        </Button>
      </div>

      {/* 编辑弹窗：名称 / 启用 /（JWT 账号）JWT 与 refreshToken */}
      <Dialog open={editOpen} onOpenChange={setEditOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>编辑账号</DialogTitle>
            <DialogDescription>
              {isJwt
                ? "JWT 与 refresh_token 留空表示保持不变。"
                : "修改账号显示名称。"}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor={`trae-name-${account.id}`}>名称</Label>
              <Input
                id={`trae-name-${account.id}`}
                value={draftName}
                onChange={(e) => setDraftName(e.target.value)}
              />
            </div>
            <div className="flex items-center justify-between">
              <Label htmlFor={`trae-enabled-${account.id}`}>启用自动签到</Label>
              <Switch
                id={`trae-enabled-${account.id}`}
                checked={account.enabled}
                onCheckedChange={(checked) => void handleToggleEnabled(checked)}
              />
            </div>
            {isJwt && (
              <>
                <div className="space-y-2">
                  <Label htmlFor={`trae-jwt-${account.id}`}>JWT（留空不修改）</Label>
                  <Input
                    id={`trae-jwt-${account.id}`}
                    value={draftJwt}
                    onChange={(e) => setDraftJwt(e.target.value)}
                    placeholder="粘贴新的 JWT"
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor={`trae-rt-${account.id}`}>refresh_token（留空不修改）</Label>
                  <Input
                    id={`trae-rt-${account.id}`}
                    value={draftRefreshToken}
                    onChange={(e) => setDraftRefreshToken(e.target.value)}
                    placeholder="粘贴新的 refresh_token"
                  />
                </div>
              </>
            )}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setEditOpen(false)}>
              取消
            </Button>
            <Button onClick={() => void saveEdit()} disabled={savingEdit}>
              {savingEdit && <Loader2 className="animate-spin" />}
              保存
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
