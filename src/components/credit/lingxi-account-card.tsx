import { useCallback, useState } from "react";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";

import { creditCardClass, formatCreditTime, lingxiOutcomeBadge } from "@/components/credit/credit-ui";
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
import type { LingxiAccount } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useLingxiStore } from "@/stores/lingxi";

interface LingxiAccountCardProps {
  account: LingxiAccount;
  /** 删除走页面级确认弹窗。 */
  onDelete: (account: LingxiAccount) => void;
}

/** 从签到 URL 提取展示域名（非法 URL 时显示原始串的前段）。 */
function displayHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url.slice(0, 40);
  }
}

/**
 * 灵犀账号卡：签到接口域名、上次签到结果（unknown 显示黄色「结果未知」）、
 * 最近成功日期、启用开关、立即签到 / 编辑（名称 + URL + Cookie）/ 删除（页面确认）。
 */
export function LingxiAccountCard({ account, onDelete }: LingxiAccountCardProps) {
  const checkinOne = useLingxiStore((s) => s.checkinOne);
  const refreshAccounts = useLingxiStore((s) => s.refreshAccounts);

  const [checking, setChecking] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [draftName, setDraftName] = useState(account.name);
  const [draftUrl, setDraftUrl] = useState(account.checkinUrl);
  const [draftCookie, setDraftCookie] = useState(account.cookie);
  const [savingEdit, setSavingEdit] = useState(false);

  const handleToggleEnabled = useCallback(
    async (enabled: boolean) => {
      try {
        await api.lingxiUpdateAccount(account.id, { enabled });
        await refreshAccounts();
      } catch (e) {
        toast.error("更新账号失败", { description: api.asError(e) });
      }
    },
    [account.id, refreshAccounts],
  );

  async function handleCheckin() {
    if (checking) return;
    setChecking(true);
    try {
      const result = await checkinOne(account.id);
      const message = result.message || "签到完成";
      if (result.outcome === "success") {
        toast.success(message, { description: account.name });
      } else if (result.outcome === "already") {
        toast.success("今日已签到", { description: account.name });
      } else if (result.outcome === "skipped") {
        toast.info("今日已成功签到，已跳过", { description: account.name });
      } else if (result.outcome === "unknown") {
        toast.warning("结果未知，稍后将自动重试", { description: message });
      } else {
        toast.error(message, { description: account.name });
      }
    } catch (e) {
      toast.error("签到失败", { description: api.asError(e) });
    } finally {
      setChecking(false);
    }
  }

  function openEdit() {
    setDraftName(account.name);
    setDraftUrl(account.checkinUrl);
    setDraftCookie(account.cookie);
    setEditOpen(true);
  }

  async function saveEdit() {
    const name = draftName.trim();
    const checkinUrl = draftUrl.trim();
    const cookie = draftCookie.trim();
    if (!checkinUrl || !cookie) {
      toast.error("checkinUrl 与 Cookie 不能为空");
      return;
    }
    setSavingEdit(true);
    try {
      await api.lingxiUpdateAccount(account.id, { name, checkinUrl, cookie });
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
  const outcomeBadge = lingxiOutcomeBadge(account.lastResult);

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
          <span className="shrink-0 text-muted-foreground">签到接口</span>
          <span className="truncate font-medium" title={account.checkinUrl}>
            {displayHost(account.checkinUrl)}
          </span>
        </div>
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted-foreground">上次签到</span>
          <span className="font-medium">
            {account.lastCheckinAt ? formatCreditTime(account.lastCheckinAt) : "从未签到"}
          </span>
        </div>
        {account.lastSuccessDate && (
          <div className="flex items-center justify-between gap-2">
            <span className="text-muted-foreground">最近成功日期</span>
            <span className="font-medium">{account.lastSuccessDate}</span>
          </div>
        )}
        {account.lastMessage && (
          <div className="flex items-start justify-between gap-2">
            <span className="shrink-0 text-muted-foreground">结果详情</span>
            <span
              className={cn(
                "line-clamp-2 text-right text-xs",
                account.lastResult === "unknown" && "text-amber-600 dark:text-amber-500",
              )}
            >
              {account.lastMessage}
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
          disabled={checking || !account.enabled}
        >
          {checking && <Loader2 className="animate-spin" />}
          {checking ? "签到中" : "立即签到"}
        </Button>
        <Button
          size="sm"
          variant="outline"
          className="flex-1"
          onClick={openEdit}
        >
          编辑
        </Button>
        <Button size="sm" variant="outline" className="flex-1" onClick={() => onDelete(account)}>
          删除
        </Button>
      </div>

      {/* 编辑弹窗：名称 / checkinUrl / Cookie */}
      <Dialog open={editOpen} onOpenChange={setEditOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>编辑账号</DialogTitle>
            <DialogDescription>修改名称、签到接口地址或 Cookie（均可从浏览器 F12 重新抓包获取）。</DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor={`lingxi-name-${account.id}`}>名称</Label>
              <Input
                id={`lingxi-name-${account.id}`}
                value={draftName}
                onChange={(e) => setDraftName(e.target.value)}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor={`lingxi-url-${account.id}`}>签到接口地址（checkinUrl）</Label>
              <Input
                id={`lingxi-url-${account.id}`}
                value={draftUrl}
                onChange={(e) => setDraftUrl(e.target.value)}
                placeholder="POST https://…"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor={`lingxi-cookie-${account.id}`}>Cookie</Label>
              <Input
                id={`lingxi-cookie-${account.id}`}
                value={draftCookie}
                onChange={(e) => setDraftCookie(e.target.value)}
                placeholder="粘贴完整 Cookie 请求头"
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
