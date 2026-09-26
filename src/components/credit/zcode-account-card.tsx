import { useCallback, useState } from "react";
import { toast } from "sonner";
import { Loader2, RefreshCw } from "lucide-react";

import { claimOutcomeBadge, creditCardClass, formatCreditTime, formatQuotaAmount } from "@/components/credit/credit-ui";
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
import type { ZcodeAccount } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useZcodeStore } from "@/stores/zcode";

interface ZcodeAccountCardProps {
  account: ZcodeAccount;
  /** 删除走页面级确认弹窗。 */
  onDelete: (account: ZcodeAccount) => void;
}

/**
 * ZCode 账号卡：与 qoder-account-card 同构（「领取」措辞）。
 * 额度展示 QuotaSnapshot（remaining/total/unit）；need-captcha 显示橙色「需手动领取」。
 */
export function ZcodeAccountCard({ account, onDelete }: ZcodeAccountCardProps) {
  const claimOne = useZcodeStore((s) => s.claimOne);
  const refreshAccounts = useZcodeStore((s) => s.refreshAccounts);

  const [claiming, setClaiming] = useState(false);
  const [refreshingQuota, setRefreshingQuota] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [draftName, setDraftName] = useState(account.name);
  const [savingEdit, setSavingEdit] = useState(false);

  const handleToggleEnabled = useCallback(
    async (enabled: boolean) => {
      try {
        await api.zcodeUpdateAccount(account.id, { enabled });
        await refreshAccounts();
      } catch (e) {
        toast.error("更新账号失败", { description: api.asError(e) });
      }
    },
    [account.id, refreshAccounts],
  );

  async function handleClaim() {
    if (claiming) return;
    setClaiming(true);
    try {
      const result = await claimOne(account.id);
      const message = result.message || "领取完成";
      if (result.outcome === "checked-in") {
        toast.success(message, { description: account.name });
      } else if (result.outcome === "already") {
        toast.success(message, { description: account.name });
      } else if (result.outcome === "need-captcha") {
        toast.warning("需手动领取", { description: message });
      } else {
        toast.error(message, { description: account.name });
      }
    } catch (e) {
      toast.error("领取失败", { description: api.asError(e) });
    } finally {
      setClaiming(false);
    }
  }

  async function handleRefreshQuota() {
    if (refreshingQuota) return;
    setRefreshingQuota(true);
    try {
      await api.zcodeGetAccountQuota(account.id);
      await refreshAccounts();
      toast.success("额度已刷新", { description: account.name });
    } catch (e) {
      toast.error("刷新额度失败", { description: api.asError(e) });
    } finally {
      setRefreshingQuota(false);
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
      await api.zcodeUpdateAccount(account.id, { name });
      await refreshAccounts();
      setEditOpen(false);
      toast.success("账号已更新");
    } catch (e) {
      toast.error("更新账号失败", { description: api.asError(e) });
    } finally {
      setSavingEdit(false);
    }
  }

  // ---- 展示态推导 ----
  const outcomeBadge = claimOutcomeBadge(account.lastResult);
  const quota = account.quota;
  const quotaText = (() => {
    if (!quota) return null;
    if (quota.empty) return "无有效套餐（仅免费额度）";
    return `${formatQuotaAmount(quota.remaining, quota.unit)} / ${formatQuotaAmount(quota.total, quota.unit)}`;
  })();

  return (
    <div className={cn(creditCardClass, !account.enabled && "opacity-55")}>
      {/* 头部：头像 + 名称 + 启用开关 */}
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-center gap-3">
          <div className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-primary text-sm font-bold text-primary-foreground">
            {account.name.charAt(0).toUpperCase() || "?"}
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <span className="truncate text-sm font-semibold">{account.name}</span>
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-1.5">
              <Badge variant={outcomeBadge.variant} className={cn("h-5 rounded-md px-1.5 text-[10px]", outcomeBadge.className)}>
                {outcomeBadge.text}
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
          <span className="text-muted-foreground">剩余额度</span>
          <span className="flex items-center gap-1 font-semibold text-amber-600 dark:text-amber-500">
            {quotaText ?? <span className="font-normal text-muted-foreground">未查询，点「刷新额度」</span>}
            <button
              type="button"
              className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
              title="刷新额度"
              onClick={() => void handleRefreshQuota()}
              disabled={refreshingQuota}
            >
              {refreshingQuota ? <Loader2 className="size-3 animate-spin" /> : <RefreshCw className="size-3" />}
            </button>
          </span>
        </div>
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted-foreground">上次领取</span>
          <span className="font-medium">
            {account.lastClaimAt ? formatCreditTime(account.lastClaimAt) : "从未领取"}
          </span>
        </div>
        {account.lastMessage && (
          <div className="flex items-start justify-between gap-2">
            <span className="shrink-0 text-muted-foreground">结果详情</span>
            <span
              className={cn(
                "line-clamp-2 text-right text-xs",
                account.lastResult === "need-captcha" && "text-amber-600 dark:text-amber-500",
              )}
            >
              {account.lastMessage}
            </span>
          </div>
        )}
        {(account.cooldownUntil ?? 0) > Date.now() && (
          <div className="flex items-center justify-between gap-2">
            <span className="text-muted-foreground">冷却中</span>
            <span className="text-xs font-medium text-red-600 dark:text-red-500">
              {account.cooldownReason || "领取失败后暂停"}
            </span>
          </div>
        )}
      </div>

      {/* 操作区 */}
      <div className="mt-4 flex flex-wrap gap-2">
        <Button
          size="sm"
          className="flex-1"
          onClick={() => void handleClaim()}
          disabled={claiming || !account.enabled}
        >
          {claiming && <Loader2 className="animate-spin" />}
          {claiming ? "领取中" : "立即领取"}
        </Button>
        <Button
          size="sm"
          variant="outline"
          className="flex-1"
          onClick={() => void handleRefreshQuota()}
          disabled={refreshingQuota}
        >
          {refreshingQuota && <Loader2 className="animate-spin" />}
          刷新额度
        </Button>
        <Button
          size="sm"
          variant="outline"
          className="flex-1"
          onClick={() => {
            setDraftName(account.name);
            setEditOpen(true);
          }}
        >
          编辑
        </Button>
        <Button size="sm" variant="outline" className="flex-1" onClick={() => onDelete(account)}>
          删除
        </Button>
      </div>

      {/* 编辑弹窗：名称 */}
      <Dialog open={editOpen} onOpenChange={setEditOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>编辑账号</DialogTitle>
            <DialogDescription>修改账号显示名称。</DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor={`zcode-name-${account.id}`}>名称</Label>
              <Input
                id={`zcode-name-${account.id}`}
                value={draftName}
                onChange={(e) => setDraftName(e.target.value)}
              />
            </div>
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
