// TRAE 平台独立设置页(对应 trae-mate SettingsPanel.vue):
// 自动签到/时间/重试/间隔/通知/定时任务/自启/exe 路径/维护(清空冷却)。
// 从账号页的设置 Tab 拆出,供侧栏「设置」直达,与 WorkBuddy 全局设置互不混淆。

import { useEffect, useState } from "react";
import { toast } from "sonner";
import { FolderSearch, Loader2, Save } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { TimePicker } from "@/components/ui/time-picker";
import * as api from "@/lib/api";
import { useTraeStore } from "@/stores/trae";

function formatLogTime(ms: number): string {
  return new Date(ms).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

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

      {/* 维护：对应 trae-mate SettingsPanel 的「清空冷却」 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">维护</h3>
        <div className="flex items-center justify-between gap-3 rounded-xl border border-border px-4 py-3">
          <div className="min-w-0">
            <p className="text-sm font-medium">清空冷却</p>
            <p className="mt-0.5 text-xs text-muted-foreground">
              重置全部账号的签到错误冷却状态（SessionDead 永久冷却除外，需重新登录）
            </p>
          </div>
          <Button
            variant="outline"
            onClick={() => {
              void api
                .traeCooldownClearAll()
                .then((n) => toast.success(`已清空 ${n} 个冷却状态`))
                .catch((e) => toast.error("清空冷却失败", { description: api.asError(e) }));
            }}
          >
            清空冷却
          </Button>
        </div>
      </section>
    </div>
  );
}

export default function TraeSettingsPage() {
  return (
    <div className="mx-auto w-full max-w-[820px] px-8 py-8">
      <h1 className="text-2xl font-semibold tracking-[-0.02em]">设置</h1>
      <p className="mt-1 text-sm text-muted-foreground">TRAE 签到调度、通知与多开实例配置。</p>
      <div className="mt-6">
        <TraeSettingsPanel />
      </div>
    </div>
  );
}
