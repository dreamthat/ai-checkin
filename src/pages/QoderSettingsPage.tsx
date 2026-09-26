// Qoder 平台独立设置页（仿 TraeSettingsPage）：
// 自动领取开关 / 间隔分钟 / 新账号默认区域 / 下次执行时间 / 清空领取日志。
// 调度由后端按「启用自动领取」开关自动启停，无手动启动/停止命令。

import { useEffect } from "react";
import { toast } from "sonner";
import { Loader2, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import type { CreditRegion } from "@/lib/types";
import { useQoderStore } from "@/stores/qoder";

function formatNextRun(iso: string): string {
  return new Date(iso).toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function QoderSettingsPanel() {
  const settings = useQoderStore((s) => s.settings);
  const nextRunAt = useQoderStore((s) => s.nextRunAt);
  const fetchAll = useQoderStore((s) => s.fetchAll);
  const saveSettings = useQoderStore((s) => s.saveSettings);
  const clearLogs = useQoderStore((s) => s.clearLogs);

  // 直达设置页时（未经过账号页）补拉一次数据
  useEffect(() => {
    if (!settings) void fetchAll();
  }, [settings, fetchAll]);

  /** 立即保存单项设置（后端保存后会按 autoClaimEnabled 重启定时轮）。 */
  async function patch(partial: Partial<NonNullable<typeof settings>>) {
    if (!settings) return;
    try {
      await saveSettings(partial);
    } catch (e) {
      toast.error("保存设置失败", { description: api.asError(e) });
    }
  }

  async function handleClearLogs() {
    try {
      await clearLogs();
      toast.success("领取日志已清空");
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
      {/* 自动领取 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">自动领取</h3>
        <div className="divide-y rounded-xl border border-border">
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">启用自动领取</p>
              <p className="mt-0.5 text-xs text-muted-foreground">
                按下方间隔为启用账号定时领取活动 Credits
              </p>
            </div>
            <Switch
              checked={settings.autoClaimEnabled}
              onCheckedChange={(checked) => void patch({ autoClaimEnabled: checked })}
            />
          </div>
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">领取间隔（分钟）</p>
              <p className="mt-0.5 text-xs text-muted-foreground">两轮领取之间的间隔（最短 30）</p>
            </div>
            <Input
              type="number"
              min={30}
              className="w-24"
              defaultValue={settings.intervalMin}
              onBlur={(e) => {
                const value = Number(e.target.value);
                if (Number.isFinite(value) && value >= 30 && value !== settings.intervalMin) {
                  void patch({ intervalMin: Math.floor(value) });
                }
              }}
            />
          </div>
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">新账号默认区域</p>
              <p className="mt-0.5 text-xs text-muted-foreground">
                手动录入账号时的缺省区域（国际版需本机 Qoder 客户端）
              </p>
            </div>
            <Select
              value={settings.region}
              onValueChange={(v) => void patch({ region: v as CreditRegion })}
            >
              <SelectTrigger className="w-[180px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="intl">国际版（intl）</SelectItem>
                <SelectItem value="cn">国内版（cn）</SelectItem>
              </SelectContent>
            </Select>
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
                : settings.autoClaimEnabled
                  ? "调度启动中，稍后自动显示"
                  : "未启用（开启「自动领取」后自动调度）"}
            </p>
          </div>
        </div>
        <p className="text-xs leading-5 text-muted-foreground">
          调度随「启用自动领取」开关自动启动与停止，无需手动操作；间隔调整后会自动重启下一轮计划。
        </p>
      </section>

      {/* 维护 */}
      <section className="space-y-3">
        <h3 className="text-sm font-semibold">维护</h3>
        <div className="flex items-center justify-between gap-3 rounded-xl border border-border px-4 py-3">
          <div className="min-w-0">
            <p className="text-sm font-medium">清空领取日志</p>
            <p className="mt-0.5 text-xs text-muted-foreground">删除全部历史领取记录（不影响账号）</p>
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

export default function QoderSettingsPage() {
  return (
    <div className="mx-auto w-full max-w-[820px] px-8 py-8">
      <h1 className="text-2xl font-semibold tracking-[-0.02em]">设置</h1>
      <p className="mt-1 text-sm text-muted-foreground">Qoder 活动领取调度与账号默认配置。</p>
      <div className="mt-6">
        <QoderSettingsPanel />
      </div>
    </div>
  );
}
