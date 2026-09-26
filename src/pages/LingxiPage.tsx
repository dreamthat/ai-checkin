import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { Loader2, Plus, RefreshCw, Trash2 } from "lucide-react";

import { LingxiAccountCard } from "@/components/credit/lingxi-account-card";
import { LingxiAddAccountModal } from "@/components/credit/lingxi-add-account-modal";
import { formatCreditLogTime, isLingxiSuccess, lingxiOutcomeBadge } from "@/components/credit/credit-ui";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import * as api from "@/lib/api";
import type { LingxiAccount } from "@/lib/types";
import { useLingxiStore } from "@/stores/lingxi";

/** 今日日期（YYYY-MM-DD，Asia/Shanghai 固定 UTC+8，与 core 幂等口径一致）。 */
function shanghaiToday(): string {
  return new Date(Date.now() + 8 * 3_600_000).toISOString().slice(0, 10);
}

export default function LingxiPage() {
  const { accounts, logs, loading, error, fetchAll, checkinAll, clearLogs } = useLingxiStore();
  const [addOpen, setAddOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<LingxiAccount | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [checkinAllRunning, setCheckinAllRunning] = useState(false);

  useEffect(() => {
    void fetchAll();
  }, [fetchAll]);

  const signedTodayCount = useMemo(() => {
    const today = shanghaiToday();
    return accounts.filter((a) => a.lastSuccessDate === today && isLingxiSuccess(a.lastResult)).length;
  }, [accounts]);
  const enabledCount = useMemo(() => accounts.filter((a) => a.enabled).length, [accounts]);

  async function handleCheckinAll() {
    if (checkinAllRunning) return;
    setCheckinAllRunning(true);
    const toastId = toast.loading("一键签到进行中…");
    try {
      await checkinAll();
      toast.success("一键签到完成", { id: toastId, description: `已处理 ${enabledCount} 个启用账号` });
    } catch (e) {
      toast.error("一键签到失败", { id: toastId, description: api.asError(e) });
    } finally {
      setCheckinAllRunning(false);
    }
  }

  async function handleClearLogs() {
    try {
      await clearLogs();
      toast.success("签到日志已清空");
    } catch (e) {
      toast.error("清空日志失败", { description: api.asError(e) });
    }
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    const target = deleteTarget;
    setDeleteTarget(null);
    setDeleting(true);
    try {
      await api.lingxiDeleteAccount(target.id);
      await useLingxiStore.getState().refreshAccounts();
      toast.success("账号已删除", { description: target.name });
    } catch (e) {
      toast.error("删除失败", { description: api.asError(e) });
    } finally {
      setDeleting(false);
    }
  }

  return (
    <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
      <header className="mb-6">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="min-w-0">
            <h1 className="text-[28px] font-semibold tracking-tight">灵犀账号</h1>
            <p className="mt-2 text-sm leading-6 text-muted-foreground">
              管理金山灵犀账号，按每日设定时间点自动签到，支持多账号与一键手动签到。
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-2 pt-1">
            <Button
              variant="outline"
              onClick={() => void fetchAll()}
              disabled={loading}
              aria-label="刷新灵犀数据"
            >
              <RefreshCw className={loading ? "animate-spin" : undefined} />
              刷新
            </Button>
            <Button onClick={() => setAddOpen(true)}>
              <Plus />
              添加账号
            </Button>
          </div>
        </div>
      </header>

      <Tabs defaultValue="accounts">
        <TabsList>
          <TabsTrigger value="accounts">
            账号
            <Badge variant="secondary" className="ml-1.5 h-4 rounded-full px-1.5 text-[10px]">
              {accounts.length}
            </Badge>
          </TabsTrigger>
          <TabsTrigger value="logs">签到日志</TabsTrigger>
        </TabsList>

        {/* 账号 Tab */}
        <TabsContent value="accounts" className="mt-5">
          {error && (
            <Alert variant="destructive" className="mb-4">
              <AlertTitle>加载失败</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-3 text-sm text-muted-foreground">
              <span>
                今日已签 <span className="font-semibold text-foreground">{signedTodayCount}</span> /{" "}
                {accounts.length}
              </span>
              <span>启用 {enabledCount} 个</span>
            </div>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void handleCheckinAll()}
              disabled={checkinAllRunning || enabledCount === 0}
            >
              {checkinAllRunning && <Loader2 className="animate-spin" />}
              一键签到
            </Button>
          </div>
          {loading && accounts.length === 0 ? (
            <div className="flex items-center gap-2 py-16 text-sm text-muted-foreground">
              <Loader2 className="animate-spin" />
              加载账号…
            </div>
          ) : accounts.length === 0 ? (
            <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
              暂无灵犀账号。点击右上角「添加账号」，从浏览器 F12 抓包获取签到接口地址与 Cookie 后录入。
            </div>
          ) : (
            <div className="grid min-w-0 grid-cols-[repeat(auto-fit,minmax(min(100%,320px),1fr))] gap-5">
              {accounts.map((account) => (
                <LingxiAccountCard key={account.id} account={account} onDelete={setDeleteTarget} />
              ))}
            </div>
          )}
        </TabsContent>

        {/* 签到日志 Tab：后端已按新的在前排序，直接渲染 */}
        <TabsContent value="logs" className="mt-5">
          <div className="mb-3 flex items-center justify-between">
            <p className="text-sm text-muted-foreground">共 {logs.length} 条（新的在前）</p>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void handleClearLogs()}
              disabled={logs.length === 0}
            >
              <Trash2 />
              清空日志
            </Button>
          </div>
          {logs.length === 0 ? (
            <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
              暂无签到日志
            </div>
          ) : (
            <div className="overflow-hidden rounded-xl border border-border">
              <table className="w-full text-xs">
                <thead className="bg-muted/50 text-muted-foreground">
                  <tr>
                    <th className="px-3 py-2 text-left font-medium">时间</th>
                    <th className="px-3 py-2 text-left font-medium">账号</th>
                    <th className="px-3 py-2 text-left font-medium">结果</th>
                    <th className="px-3 py-2 text-left font-medium">详情</th>
                  </tr>
                </thead>
                <tbody>
                  {logs.map((log, i) => {
                    const badge = lingxiOutcomeBadge(log.result);
                    return (
                      <tr key={`${log.time}-${log.accountId}-${i}`} className="border-t align-top">
                        <td className="whitespace-nowrap px-3 py-2 text-muted-foreground">
                          {formatCreditLogTime(log.time)}
                        </td>
                        <td className="max-w-[140px] truncate px-3 py-2">{log.accountName}</td>
                        <td className="px-3 py-2">
                          <Badge
                            variant={badge.variant}
                            className={`h-5 rounded-md px-1.5 text-[10px] ${badge.className ?? ""}`}
                          >
                            {badge.text}
                          </Badge>
                        </td>
                        <td className="px-3 py-2 text-muted-foreground">{log.message}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </TabsContent>
      </Tabs>

      <LingxiAddAccountModal open={addOpen} onOpenChange={setAddOpen} />

      {/* 删除账号确认（桌面 App 不支持 window.confirm） */}
      <Dialog open={deleteTarget !== null} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>删除账号</DialogTitle>
            <DialogDescription>
              确定删除账号「{deleteTarget?.name}」？删除后需重新添加才能参与签到。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDeleteTarget(null)}>
              取消
            </Button>
            <Button variant="destructive" onClick={() => void confirmDelete()} disabled={deleting}>
              {deleting && <Loader2 className="animate-spin" />}
              删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
