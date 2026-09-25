import { useEffect, useMemo, useState } from "react";
import { Navigate } from "react-router-dom";
import { CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";
import { Loader2, RefreshCw, Search } from "lucide-react";
import { toast } from "sonner";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import * as api from "@/lib/api";
import { demoModeEnabled } from "@/lib/demo-mode";
import type { TraeCreditsDailySnapshot } from "@/lib/types";
import { useTraeStore } from "@/stores/trae";

/** 三线趋势图表配置：总积分 / 获取 / 消耗。 */
const chartConfig = {
  total: { label: "总积分", color: "var(--chart-1)" },
  earned: { label: "获取", color: "var(--chart-2)" },
  consumed: { label: "消耗", color: "var(--chart-3)" },
} satisfies ChartConfig;

/** 趋势图最多展示最近 30 天，避免曲线过密。 */
const TREND_MAX_DAYS = 30;

export default function TraeCreditsPage() {
  const accounts = useTraeStore((s) => s.accounts);
  const refreshAccounts = useTraeStore((s) => s.refreshAccounts);
  const [daily, setDaily] = useState<TraeCreditsDailySnapshot[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshingAll, setRefreshingAll] = useState(false);
  /** 逐账号实时查询的在途 userId 集合（防连点）。 */
  const [fetchingIds, setFetchingIds] = useState<Set<string>>(new Set());

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const snapshots = await api.traeCreditsDailyList();
        if (!cancelled) setDaily(snapshots);
      } catch (e) {
        if (!cancelled) setError(api.asError(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  /** 有 userId 的账号才能查剩余积分（jwt 取 userId，desktop 取 desktopUserId）。 */
  const creditAccounts = useMemo(
    () =>
      accounts
        .map((a) => ({ id: a.id, name: a.name, userId: a.userId ?? a.desktopUserId, points: a.points }))
        .filter((a) => Boolean(a.userId)),
    [accounts],
  );

  const trendData = useMemo(
    () => daily.slice(-TREND_MAX_DAYS),
    [daily],
  );

  /** 刷新全部剩余积分（含自动解冻），完成后同步账号列表与每日快照。 */
  async function handleRefreshAll() {
    if (refreshingAll) return;
    setRefreshingAll(true);
    const toastId = toast.loading("正在刷新全部剩余积分…");
    try {
      const count = await api.traeRefreshAllRemainingCredits();
      toast.success("剩余积分已刷新", { id: toastId, description: `已更新 ${count} 个账号` });
      await Promise.all([refreshAccounts(), reloadDaily()]);
    } catch (e) {
      toast.error("刷新失败", { id: toastId, description: api.asError(e) });
    } finally {
      setRefreshingAll(false);
    }
  }

  async function reloadDaily() {
    try {
      setDaily(await api.traeCreditsDailyList());
    } catch {
      /* 快照刷新失败不影响主流程 */
    }
  }

  /** 逐账号实时查询剩余积分。 */
  async function handleFetchOne(userId: string, name: string) {
    if (fetchingIds.has(userId)) return;
    setFetchingIds((prev) => new Set(prev).add(userId));
    try {
      const credits = await api.traeFetchRemainingCredits(userId);
      toast.success(`${name} 剩余积分：${credits}`);
      await Promise.all([refreshAccounts(), reloadDaily()]);
    } catch (e) {
      toast.error("查询失败", { description: `${name}：${api.asError(e)}` });
    } finally {
      setFetchingIds((prev) => {
        const next = new Set(prev);
        next.delete(userId);
        return next;
      });
    }
  }

  // 演示模式侧栏隐藏：直接访问路由时兜底跳回首页
  if (demoModeEnabled) return <Navigate to="/" replace />;

  return (
    <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
      <header className="mb-6 flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <h1 className="text-[28px] font-semibold tracking-tight">TRAE 积分看板</h1>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            每日积分快照三线趋势（总积分 / 获取 / 消耗），支持逐账号实时查询剩余积分。
          </p>
        </div>
        <Button
          className="shrink-0"
          onClick={() => void handleRefreshAll()}
          disabled={refreshingAll || creditAccounts.length === 0}
        >
          {refreshingAll ? <Loader2 className="animate-spin" /> : <RefreshCw />}
          刷新全部剩余积分
        </Button>
      </header>

      {error && (
        <Alert variant="destructive" className="mb-4">
          <AlertTitle>加载失败</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {/* 三线趋势 */}
      <Card className="mb-6">
        <CardHeader>
          <CardTitle className="text-base">每日积分趋势</CardTitle>
          <CardDescription>最近 {trendData.length} 天快照（来自 credits_daily）</CardDescription>
        </CardHeader>
        <CardContent>
          {loading ? (
            <div className="flex items-center gap-2 py-16 text-sm text-muted-foreground">
              <Loader2 className="animate-spin" />
              加载快照…
            </div>
          ) : trendData.length === 0 ? (
            <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
              暂无每日快照。签到或刷新剩余积分后会自动记录。
            </div>
          ) : (
            <ChartContainer config={chartConfig} className="h-[280px] w-full">
              <LineChart data={trendData} margin={{ left: 4, right: 12 }}>
                <CartesianGrid vertical={false} strokeDasharray="3 3" />
                <XAxis
                  dataKey="date"
                  tickLine={false}
                  axisLine={false}
                  tickMargin={8}
                  minTickGap={24}
                  tickFormatter={(value: string) => value.slice(5)}
                />
                <YAxis tickLine={false} axisLine={false} width={48} />
                <ChartTooltip content={<ChartTooltipContent />} />
                <Line
                  dataKey="total"
                  type="monotone"
                  stroke="var(--color-total)"
                  strokeWidth={2}
                  dot={false}
                />
                <Line
                  dataKey="earned"
                  type="monotone"
                  stroke="var(--color-earned)"
                  strokeWidth={2}
                  dot={false}
                />
                <Line
                  dataKey="consumed"
                  type="monotone"
                  stroke="var(--color-consumed)"
                  strokeWidth={2}
                  dot={false}
                />
              </LineChart>
            </ChartContainer>
          )}
        </CardContent>
      </Card>

      {/* 逐账号实时查询 */}
      <Card>
        <CardHeader>
          <CardTitle className="text-base">账号剩余积分</CardTitle>
          <CardDescription>实时查询各账号剩余积分并写入缓存</CardDescription>
        </CardHeader>
        <CardContent>
          {creditAccounts.length === 0 ? (
            <div className="rounded-xl border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
              暂无账号
            </div>
          ) : (
            <div className="divide-y rounded-xl border border-border">
              {creditAccounts.map((account) => (
                <div key={account.id} className="flex items-center justify-between gap-3 px-4 py-3">
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium">{account.name}</p>
                    <p className="mt-0.5 text-xs text-muted-foreground">
                      总积分 {account.points ?? "—"}
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={fetchingIds.has(account.userId as string)}
                      onClick={() => void handleFetchOne(account.userId as string, account.name)}
                    >
                      {fetchingIds.has(account.userId as string) ? (
                        <Loader2 className="animate-spin" />
                      ) : (
                        <Search />
                      )}
                      实时查询
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
