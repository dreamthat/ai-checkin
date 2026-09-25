import { useEffect, useMemo, useRef, useState } from "react";
import { Navigate } from "react-router-dom";
import { toast } from "sonner";
import { FolderSearch, Loader2, Plus, RefreshCw, Save, Trash2 } from "lucide-react";

import { TraeAccountCard } from "@/components/trae/trae-account-card";
import { TraeAddAccountModal } from "@/components/trae/trae-add-account-modal";
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
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { TimePicker } from "@/components/ui/time-picker";
import * as api from "@/lib/api";
import { demoModeEnabled } from "@/lib/demo-mode";
import type { TraeAccount, TraeMigrationReport } from "@/lib/types";
import { initTraeEvents, useTraeStore } from "@/stores/trae";

/** 今日已签计数：优先 checkedToday，回退 lastCheckinAt 判定（与 trae-mate 同语义）。 */
function countCheckedToday(accounts: TraeAccount[]): number {
  const checked = accounts.filter((a) => a.checkedToday === true).length;
  if (checked > 0) return checked;
  const today = new Date().toDateString();
  return accounts.filter(
    (a) =>
      a.lastCheckinAt &&
      new Date(a.lastCheckinAt).toDateString() === today &&
      a.lastCheckinResult === "success",
  ).length;
}

function formatLogTime(ms: number): string {
  const d = new Date(ms);
  const pad = (v: number) => String(v).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** 设置 Tab：对应 trae-mate SettingsPanel.vue（自动签到/时间/重试/间隔/通知/自启/exe 路径）。 */
function TraeSettingsPanel() {
  const settings = useTraeStore((s) => s.settings);
  const nextRunAt = useTraeStore((s) => s.nextRunAt);
  const saveSettings = useTraeStore((s) => s.saveSettings);
  const startScheduler = useTraeStore((s) => s.startScheduler);
  const stopScheduler = useTraeStore((s) => s.stopScheduler);
  const [exePath, setExePath] = useState<string>("");
  const [exePathDraft, setExePathDraft] = useState("");
  /** exe 扫描按钮三态：idle 默认 / scanning 扫描中 / done 完成提示。 */
  const [scanState, setScanState] = useState<"idle" | "scanning" | "done">("idle");
  const [saving, setSaving] = useState(false);

  // 读取已保存的 TRAE exe 路径
  useEffect(() => {
    let cancelled = false;
    void api
      .traeGetTraeExePath()
      .then((path) => {
        if (cancelled) return;
        setExePath(path ?? "");
        setExePathDraft(path ?? "");
      })
      .catch(() => {
        /* 读取失败按未设置处理 */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  /** 立即保存单项设置（后端保存后会自动重启定时任务）。 */
  async function patch(partial: Partial<NonNullable<typeof settings>>) {
    if (!settings) return;
    try {
      await saveSettings(partial);
    } catch (e) {
      toast.error("保存设置失败", { description: api.asError(e) });
    }
  }

  async function handleSaveExePath() {
    const path = exePathDraft.trim();
    if (!path) {
      toast.error("请填写 TRAE exe 路径");
      return;
    }
    setSaving(true);
    try {
      await api.traeSetTraeExePath(path);
      setExePath(path);
      toast.success("TRAE 路径已保存");
    } catch (e) {
      toast.error("保存 TRAE 路径失败", { description: api.asError(e) });
    } finally {
      setSaving(false);
    }
  }

  // 三态扫描按钮：idle → scanning → done（1.5s 后回 idle）
  async function handleScanExePath() {
    if (scanState !== "idle") return;
    setScanState("scanning");
    try {
      const scanned = await api.traeScanTraeExePath();
      setExePath(scanned);
      setExePathDraft(scanned);
      setScanState("done");
      toast.success("已扫描到 TRAE 路径", { description: scanned });
      window.setTimeout(() => setScanState("idle"), 1500);
    } catch (e) {
      setScanState("idle");
      toast.error("扫描失败", { description: api.asError(e) });
    }
  }

  if (!settings) {
    return (
      <div className="flex items-center gap-2 py-10 text-sm text-muted-foreground">
        <Loader2 className="animate-spin" />
        加载设置…
      </div>
    );
  }

  return (
    <div className="max-w-[680px] space-y-6">
      {/* 自动签到 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">自动签到</h3>
        <div className="divide-y rounded-xl border border-border">
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">启用自动签到</p>
              <p className="mt-0.5 text-xs text-muted-foreground">
                按下方时间每日为启用账号自动签到
              </p>
            </div>
            <Switch
              checked={settings.autoCheckin}
              onCheckedChange={(checked) => void patch({ autoCheckin: checked })}
            />
          </div>
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">签到时间</p>
              <p className="mt-0.5 text-xs text-muted-foreground">每日该时刻执行自动签到</p>
            </div>
            <TimePicker
              value={settings.checkinTime}
              onChange={(value) => void patch({ checkinTime: value })}
            />
          </div>
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">重试次数</p>
              <p className="mt-0.5 text-xs text-muted-foreground">签到失败后的重试上限</p>
            </div>
            <Input
              type="number"
              min={0}
              className="w-24"
              defaultValue={settings.retryCount}
              onBlur={(e) => {
                const value = Number(e.target.value);
                if (Number.isFinite(value) && value >= 0 && value !== settings.retryCount) {
                  void patch({ retryCount: Math.floor(value) });
                }
              }}
            />
          </div>
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">重试延迟（秒）</p>
              <p className="mt-0.5 text-xs text-muted-foreground">两次重试之间的等待时间</p>
            </div>
            <Input
              type="number"
              min={0}
              className="w-24"
              defaultValue={settings.retryDelay}
              onBlur={(e) => {
                const value = Number(e.target.value);
                if (Number.isFinite(value) && value >= 0 && value !== settings.retryDelay) {
                  void patch({ retryDelay: Math.floor(value) });
                }
              }}
            />
          </div>
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">账号间隔（分钟）</p>
              <p className="mt-0.5 text-xs text-muted-foreground">
                定时签到时账号间的执行间隔（最短 3）
              </p>
            </div>
            <Input
              type="number"
              min={3}
              className="w-24"
              defaultValue={settings.autoCheckinIntervalMin}
              onBlur={(e) => {
                const value = Number(e.target.value);
                if (Number.isFinite(value) && value >= 3 && value !== settings.autoCheckinIntervalMin) {
                  void patch({ autoCheckinIntervalMin: Math.floor(value) });
                }
              }}
            />
          </div>
        </div>
      </section>

      {/* 通知 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">通知</h3>
        <div className="divide-y rounded-xl border border-border">
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <p className="text-sm font-medium">签到成功时通知</p>
            <Switch
              checked={settings.notifyOnSuccess}
              onCheckedChange={(checked) => void patch({ notifyOnSuccess: checked })}
            />
          </div>
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <p className="text-sm font-medium">签到失败时通知</p>
            <Switch
              checked={settings.notifyOnFailed}
              onCheckedChange={(checked) => void patch({ notifyOnFailed: checked })}
            />
          </div>
        </div>
      </section>

      {/* 调度 + 自启 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">定时任务</h3>
        <div className="divide-y rounded-xl border border-border">
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">下次执行时间</p>
              <p className="mt-0.5 text-xs text-muted-foreground">
                {nextRunAt ? formatLogTime(new Date(nextRunAt).getTime()) : "未启用定时任务"}
              </p>
            </div>
            <div className="flex shrink-0 gap-2">
              <Button size="sm" variant="outline" onClick={() => void startScheduler()}>
                启动调度
              </Button>
              <Button size="sm" variant="outline" onClick={() => void stopScheduler()}>
                停止调度
              </Button>
            </div>
          </div>
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">开机自启</p>
              <p className="mt-0.5 text-xs text-muted-foreground">随系统启动运行本应用</p>
            </div>
            <Switch
              checked={settings.launchAtLogin}
              onCheckedChange={(checked) => void patch({ launchAtLogin: checked })}
            />
          </div>
        </div>
      </section>

      {/* TRAE 客户端路径 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">TRAE 客户端路径</h3>
        <div className="space-y-3 rounded-xl border border-border px-4 py-3">
          <p className="text-xs leading-5 text-muted-foreground">
            启动独立实例需要指定 TRAE Work CN 客户端（TRAE SOLO CN.exe）。未设置时启动会自动扫描。
            {exePath ? `当前：${exePath}` : "当前未设置。"}
          </p>
          <div className="flex items-center gap-2">
            <Input
              value={exePathDraft}
              onChange={(e) => setExePathDraft(e.target.value)}
              placeholder="C:\\...\\TRAE SOLO CN.exe"
              className="min-w-0 flex-1"
            />
            <Button variant="outline" onClick={() => void handleSaveExePath()} disabled={saving}>
              {saving ? <Loader2 className="animate-spin" /> : <Save />}
              保存
            </Button>
            <Button
              variant="outline"
              onClick={() => void handleScanExePath()}
              disabled={scanState === "scanning"}
              title="自动扫描 TRAE 安装路径并保存"
            >
              {scanState === "scanning" ? (
                <>
                  <Loader2 className="animate-spin" />
                  扫描中
                </>
              ) : scanState === "done" ? (
                <>
                  <FolderSearch />
                  已扫描
                </>
              ) : (
                <>
                  <FolderSearch />
                  自动扫描
                </>
              )}
            </Button>
          </div>
        </div>
      </section>
    </div>
  );
}

export default function TraeAccountsPage() {
  const { accounts, logs, loading, error, fetchAll, checkinAll, clearLogs } = useTraeStore();
  const [addOpen, setAddOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<TraeAccount | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [checkinAllRunning, setCheckinAllRunning] = useState(false);
  /** 旧 TraeMate 迁移报告（幂等命令，挂载时执行一次）。 */
  const [migration, setMigration] = useState<TraeMigrationReport | null>(null);
  const migrationTried = useRef(false);

  // 演示模式侧栏隐藏：直接访问路由时兜底跳回首页（demoModeEnabled 全程不变，钩子顺序安全）
  useEffect(() => {
    if (migrationTried.current) return;
    migrationTried.current = true;
    void api
      .traeMigrateLegacyData()
      .then((report) => setMigration(report))
      .catch(() => {
        /* 迁移探测失败不打扰使用 */
      });
    void initTraeEvents();
    void fetchAll();
  }, [fetchAll]);

  if (demoModeEnabled) return <Navigate to="/" replace />;

  const checkedCount = useMemo(() => countCheckedToday(accounts), [accounts]);
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
      await api.traeDeleteAccount(target.id);
      await useTraeStore.getState().refreshAccounts();
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
            <h1 className="text-[28px] font-semibold tracking-tight">TRAE 账号</h1>
            <p className="mt-2 text-sm leading-6 text-muted-foreground">
              管理 TRAE 账号的多开实例、自动签到、积分与凭据状态。
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-2 pt-1">
            <Button
              variant="outline"
              onClick={() => void fetchAll()}
              disabled={loading}
              aria-label="刷新 TRAE 数据"
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
        {/* 迁移横幅：检测到旧数据时展示迁移结果；旧进程仍在运行时红色警告双开竞争 */}
        {migration?.detected && (
          <div className="mt-4 space-y-2">
            <Alert>
              <AlertTitle>已迁移旧 TraeMate 数据</AlertTitle>
              <AlertDescription>
                从 %APPDATA%\com.traecheck.app 迁移 {migration.accountsImported} 个账号、
                {migration.logsImported} 条日志
                {migration.exePathMigrated ? "，TRAE 路径配置已同步" : ""}
                {migration.skipped.length > 0 ? `；跳过 ${migration.skipped.length} 个文件（目标已存在）` : ""}。
              </AlertDescription>
            </Alert>
            {migration.legacyProcessRunning && (
              <Alert variant="destructive">
                <AlertTitle>旧 TraeMate 仍在运行</AlertTitle>
                <AlertDescription>
                  请退出并卸载旧版，避免双开竞争签到。
                </AlertDescription>
              </Alert>
            )}
          </div>
        )}
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
          <TabsTrigger value="settings">设置</TabsTrigger>
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
                今日已签 <span className="font-semibold text-foreground">{checkedCount}</span> /{" "}
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
              暂无 TRAE 账号。点击右上角「添加账号」导入桌面账号、新开实例登录或手动录入 JWT。
            </div>
          ) : (
            <div className="grid min-w-0 grid-cols-[repeat(auto-fit,minmax(min(100%,320px),1fr))] gap-5">
              {accounts.map((account) => (
                <TraeAccountCard key={account.id} account={account} onDelete={setDeleteTarget} />
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
                  {logs.map((log) => (
                    <tr key={log.id} className="border-t align-top">
                      <td className="whitespace-nowrap px-3 py-2 text-muted-foreground">
                        {formatLogTime(log.time)}
                      </td>
                      <td className="max-w-[140px] truncate px-3 py-2">{log.accountName}</td>
                      <td className="px-3 py-2">
                        <Badge
                          variant={log.result === "success" ? "default" : "destructive"}
                          className="h-5 rounded-md px-1.5 text-[10px]"
                        >
                          {log.result === "success" ? "成功" : "失败"}
                        </Badge>
                        {log.pointsGained ? (
                          <span className="ml-1.5 text-amber-600 dark:text-amber-500">
                            +{log.pointsGained}
                          </span>
                        ) : null}
                      </td>
                      <td className="px-3 py-2 text-muted-foreground">{log.message}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </TabsContent>

        {/* 设置 Tab */}
        <TabsContent value="settings" className="mt-5">
          <TraeSettingsPanel />
        </TabsContent>
      </Tabs>

      <TraeAddAccountModal open={addOpen} onOpenChange={setAddOpen} />

      {/* 删除账号确认（桌面 App 不支持 window.confirm） */}
      <Dialog open={deleteTarget !== null} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>删除账号</DialogTitle>
            <DialogDescription>
              确定删除账号「{deleteTarget?.name}」？其实例数据目录会保留，日后可通过「扫描已有多开目录」重新导入。
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
