// 灵犀平台独立设置页（仿 QoderSettingsPage）：
// 每日签到时间点列表（HH:MM，Asia/Shanghai）/ 成功与失败通知开关 / 下次执行时间 / 清空签到日志。
// 无单独的自动签到开关：时间点列表非空即参与调度，清空列表即停。

import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Loader2, Plus, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import { useLingxiStore } from "@/stores/lingxi";

/** HH:MM（24 小时制，分钟两位）。 */
const HHMM_RE = /^([01]?\d|2[0-3]):[0-5]\d$/;

/** 规范化为两位 HH:MM（非法输入返回 null）。 */
function normalizeHhmm(raw: string): string | null {
  const text = raw.trim();
  if (!HHMM_RE.test(text)) return null;
  const [h, m] = text.split(":");
  return `${h.padStart(2, "0")}:${m}`;
}

function formatNextRun(iso: string): string {
  return new Date(iso).toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function LingxiSettingsPanel() {
  const settings = useLingxiStore((s) => s.settings);
  const nextRunAt = useLingxiStore((s) => s.nextRunAt);
  const fetchAll = useLingxiStore((s) => s.fetchAll);
  const saveSettings = useLingxiStore((s) => s.saveSettings);
  const clearLogs = useLingxiStore((s) => s.clearLogs);

  // 时间点草稿：编辑中不受 settings 刷新打断，失焦时校验并提交
  const [draftTimes, setDraftTimes] = useState<string[]>([]);

  // 直达设置页时（未经过账号页）补拉一次数据
  useEffect(() => {
    if (!settings) void fetchAll();
  }, [settings, fetchAll]);

  useEffect(() => {
    if (settings) setDraftTimes(settings.checkinTimes);
  }, [settings]);

  /** 立即保存单项设置（后端保存后会按时间点重启定时轮）。 */
  async function patch(partial: Partial<NonNullable<typeof settings>>) {
    if (!settings) return;
    try {
      await saveSettings(partial);
    } catch (e) {
      toast.error("保存设置失败", { description: api.asError(e) });
    }
  }

  /** 提交整个时间点列表（后端会过滤非法格式并规范化）。 */
  async function commitTimes(times: string[]) {
    const normalized = times.map(normalizeHhmm).filter((v): v is string => v !== null);
    await patch({ checkinTimes: normalized });
  }

  function updateTimeAt(index: number, value: string) {
    setDraftTimes((prev) => prev.map((t, i) => (i === index ? value : t)));
  }

  async function handleTimeBlur(index: number) {
    const raw = draftTimes[index] ?? "";
    if (!settings) return;
    if (!raw.trim()) {
      // 清空视为删除该行
      const next = draftTimes.filter((_, i) => i !== index);
      setDraftTimes(next);
      await commitTimes(next);
      return;
    }
    const normalized = normalizeHhmm(raw);
    if (!normalized) {
      toast.error("时间格式应为 HH:MM（如 08:30）");
      setDraftTimes(settings.checkinTimes);
      return;
    }
    const next = draftTimes.map((t, i) => (i === index ? normalized : t));
    setDraftTimes(next);
    if (normalized !== settings.checkinTimes[index] || next.join() !== settings.checkinTimes.join()) {
      await commitTimes(next);
    }
  }

  async function addTime() {
    const next = [...draftTimes, "09:00"];
    setDraftTimes(next);
    await commitTimes(next);
  }

  async function removeTime(index: number) {
    const next = draftTimes.filter((_, i) => i !== index);
    setDraftTimes(next);
    await commitTimes(next);
  }

  async function handleClearLogs() {
    try {
      await clearLogs();
      toast.success("签到日志已清空");
    } catch (e) {
      toast.error("清空日志失败", { description: api.asError(e) });
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
      {/* 定时签到 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">定时签到</h3>
        <div className="space-y-3 rounded-xl border border-border px-4 py-4">
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">每日签到时间点</p>
              <p className="mt-0.5 text-xs text-muted-foreground">
                到点对全部启用账号各执行一轮签到（北京时间）；清空全部时间点即停止调度
              </p>
            </div>
            <Button variant="outline" size="sm" onClick={() => void addTime()}>
              <Plus />
              添加
            </Button>
          </div>
          {draftTimes.length === 0 ? (
            <p className="rounded-lg border border-dashed px-3 py-4 text-center text-sm text-muted-foreground">
              暂无时间点，点击「添加」新增（如 08:30、16:30）。
            </p>
          ) : (
            <div className="space-y-2">
              {draftTimes.map((time, index) => (
                <div key={index} className="flex items-center gap-2">
                  <Input
                    className="w-28"
                    value={time}
                    placeholder="HH:MM"
                    onChange={(e) => updateTimeAt(index, e.target.value)}
                    onBlur={() => void handleTimeBlur(index)}
                  />
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-8 text-muted-foreground hover:text-destructive"
                    aria-label={`删除时间点 ${time}`}
                    onClick={() => void removeTime(index)}
                  >
                    <Trash2 className="size-4" />
                  </Button>
                </div>
              ))}
            </div>
          )}
        </div>
      </section>

      {/* 通知 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">通知</h3>
        <div className="divide-y rounded-xl border border-border">
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">签到成功通知</p>
              <p className="mt-0.5 text-xs text-muted-foreground">签到成功或已签过时发送系统通知</p>
            </div>
            <Switch
              checked={settings.notifyOnSuccess}
              onCheckedChange={(checked) => void patch({ notifyOnSuccess: checked })}
            />
          </div>
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">签到失败通知</p>
              <p className="mt-0.5 text-xs text-muted-foreground">签到失败时发送系统通知（结果未知不打扰，下个时间点自动重试）</p>
            </div>
            <Switch
              checked={settings.notifyOnFailed}
              onCheckedChange={(checked) => void patch({ notifyOnFailed: checked })}
            />
          </div>
        </div>
      </section>

      {/* 调度 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">定时任务</h3>
        <div className="flex items-center justify-between gap-3 rounded-xl border border-border px-4 py-3">
          <div className="min-w-0">
            <p className="text-sm font-medium">下次执行时间</p>
            <p className="mt-0.5 text-xs text-muted-foreground">
              {nextRunAt
                ? formatNextRun(nextRunAt)
                : settings.checkinTimes.length > 0
                  ? "调度启动中，稍后自动显示"
                  : "未启用（添加签到时间点后自动调度）"}
            </p>
          </div>
        </div>
        <p className="text-xs leading-5 text-muted-foreground">
          调度随时间点列表自动启停：列表非空即运行，清空即停止；时间点调整后自动重启下一轮计划。
        </p>
      </section>

      {/* 维护 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">维护</h3>
        <div className="flex items-center justify-between gap-3 rounded-xl border border-border px-4 py-3">
          <div className="min-w-0">
            <p className="text-sm font-medium">清空签到日志</p>
            <p className="mt-0.5 text-xs text-muted-foreground">删除全部历史签到记录（不影响账号）</p>
          </div>
          <Button variant="outline" onClick={() => void handleClearLogs()}>
            <Trash2 />
            清空日志
          </Button>
        </div>
      </section>
    </div>
  );
}

export default function LingxiSettingsPage() {
  return (
    <div className="mx-auto w-full max-w-[820px] px-8 py-8">
      <h1 className="text-2xl font-semibold tracking-[-0.02em]">设置</h1>
      <p className="mt-1 text-sm text-muted-foreground">灵犀每日定时签到与通知配置。</p>
      <div className="mt-6">
        <LingxiSettingsPanel />
      </div>
    </div>
  );
}
