import { useState } from "react";
import { toast } from "sonner";
import { BrowserRouter, HashRouter, Navigate, NavLink, Outlet, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { ArrowUp, Loader2, MessagesSquare, Rocket, Settings, Sparkles, User } from "lucide-react";

import { cn } from "@/lib/utils";
import * as api from "@/lib/api";
import AccountsPage from "@/pages/AccountsPage";
import CreditStatsPage from "@/pages/CreditStatsPage";
import TokenStatsPage from "@/pages/TokenStatsPage";
import SettingsPage from "@/pages/SettingsPage";
import TraeAccountsPage from "@/pages/TraeAccountsPage";
import TraeCreditsPage from "@/pages/TraeCreditsPage";
import { StatusDot, AppIconMark } from "@/components/product-marks";
import { CompanionDemoDialog } from "@/components/companion-demo-dialog";
import { DemoAction } from "@/components/demo-action";
import { UpdateInstallDialog } from "@/components/update-install-dialog";
import { Badge } from "@/components/ui/badge";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
  AlertDialogDescription, AlertDialogFooter, AlertDialogHeader,
  AlertDialogTitle, AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Toaster } from "@/components/ui/sonner";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import companionTrayIcon from "@/assets/agent-companion-tray.png";
import { demoModeEnabled, pagesDemoHostingEnabled } from "@/lib/demo-mode";
import { changeCompanionEnabled, useCompanionEnabled } from "@/lib/use-companion-enabled";
import { useCreditAutoRefresh } from "@/lib/use-credit-auto-refresh";
import { useRotateDeferredNotice } from "@/lib/use-rotate-deferred-notice";
import { useUpdateState } from "@/lib/use-update-state";
import { useWorkbuddyStatusRefresh } from "@/lib/use-workbuddy-status-refresh";
import { useAccountsStore } from "@/stores/accounts";

function CompanionFooter() {
  const { enabled, busy } = useCompanionEnabled();

  async function onToggle() {
    if (enabled === null) return;
    try {
      const confirmed = await changeCompanionEnabled(!enabled);
      toast.success(confirmed ? "已启用 Agent Companion 悬浮窗" : "已关闭 Agent Companion 悬浮窗");
    } catch (cause) {
      toast.error("悬浮窗设置失败", { description: api.asError(cause) });
    }
  }

  async function openSettings() {
    try {
      await api.openCompanionSettings();
    } catch (cause) {
      toast.error("打开悬浮窗设置失败", { description: api.asError(cause) });
    }
  }

  return (
    <div className="flex min-w-0 items-center gap-1 text-sidebar-foreground">
      <AlertDialog>
        <Tooltip>
          <TooltipTrigger asChild>
            <AlertDialogTrigger asChild>
              <Button type="button" variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg" aria-label="会话悬浮窗" disabled={enabled === null || busy}>
                <img src={companionTrayIcon} alt="" className={cn("size-6 object-contain transition-all", !enabled && "grayscale opacity-55")} />
              </Button>
            </AlertDialogTrigger>
          </TooltipTrigger>
          <TooltipContent side="top">会话悬浮窗</TooltipContent>
        </Tooltip>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{enabled ? "关闭会话悬浮窗？" : "开启会话悬浮窗？"}</AlertDialogTitle>
            <AlertDialogDescription>
              {enabled
                ? "关闭后悬浮栏将隐藏，并停止 wb-switch 中的会话监听。"
                : "开启后会显示悬浮栏，并开始监听会话状态。"}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction onClick={() => void onToggle()}>{enabled ? "确认关闭" : "确认开启"}</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button type="button" variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg" aria-label="悬浮窗设置" disabled={!enabled} onClick={() => void openSettings()}>
            <Settings className="size-4" aria-hidden="true" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">悬浮窗设置</TooltipContent>
      </Tooltip>
    </div>
  );
}

/**
 * 演示模式的悬浮窗入口：与桌面正式版同形，但点击打开的是只读演示浮层。
 * 设置入口沿用演示模式的禁用约定（`DemoAction`），不触发任何本机命令。
 */
function CompanionDemoFooter() {
  const [open, setOpen] = useState(false);

  return (
    <div className="flex min-w-0 items-center gap-1 text-sidebar-foreground">
      <Tooltip>
        <TooltipTrigger asChild>
          <Button type="button" variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg" aria-label="会话悬浮窗" onClick={() => setOpen(true)}>
            <img src={companionTrayIcon} alt="" className="size-6 object-contain" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">会话悬浮窗</TooltipContent>
      </Tooltip>
      <DemoAction>
        <Button type="button" variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg" aria-label="悬浮窗设置">
          <Settings className="size-4" aria-hidden="true" />
        </Button>
      </DemoAction>
      <CompanionDemoDialog open={open} onOpenChange={setOpen} />
    </div>
  );
}

function UpdateCenter({ running }: { running: boolean | undefined }) {
  const version = useAccountsStore((s) => s.status?.version);
  const snapshot = useUpdateState();
  const [dialogOpen, setDialogOpen] = useState(false);
  const showCompanion = api.isDesktop() && !demoModeEnabled;
  // 演示模式只提供只读演示浮层，不渲染正式版的悬浮窗开关。
  const showCompanionDemo = demoModeEnabled;

  // 阶段由 Rust 更新服务经 `update-state` 推送（托盘同源），前端不再轮询检查。
  // 已知目标版本时，检查中 / 失败也要保留入口，与托盘「升级到 vX / 点击重试」对齐。
  const hasKnownTarget = Boolean(snapshot.latest);
  const hasUpdate =
    snapshot.phase === "available" ||
    snapshot.phase === "downloading" ||
    snapshot.phase === "readyToRestart" ||
    (hasKnownTarget && (snapshot.phase === "error" || snapshot.phase === "checking"));
  const updateHint =
    snapshot.phase === "downloading"
      ? snapshot.percent === null
        ? "正在下载更新…"
        : `正在下载更新 ${snapshot.percent}%`
      : snapshot.phase === "readyToRestart"
        ? "重启以完成升级"
        : snapshot.phase === "error"
          ? "更新失败，点击重试"
          : snapshot.phase === "checking"
            ? "正在检查…"
            : "更新";

  return (
    <>
      <section className="mt-auto border-t border-sidebar-border px-2 pt-3 text-xs">
        <div className="flex items-center gap-2 text-[13px] text-sidebar-foreground">
          {showCompanion ? <CompanionFooter /> : showCompanionDemo ? <CompanionDemoFooter /> : (
            <>
              <StatusDot on={Boolean(running)} />
              <span className="min-w-0 flex-1 truncate">WorkBuddy</span>
            </>
          )}
          <div className="ml-auto flex shrink-0 items-center gap-1.5">
            <span className="text-sidebar-foreground/50">v{version || "?"}</span>
            {hasUpdate && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    type="button"
                    size="icon"
                    className="size-5 rounded-full p-0"
                    aria-label={updateHint}
                    onClick={() => setDialogOpen(true)}
                  >
                    {snapshot.phase === "downloading" || snapshot.phase === "checking" ? (
                      <Loader2 className="size-3 animate-spin" strokeWidth={2.5} aria-hidden="true" />
                    ) : (
                      <ArrowUp className="size-3" strokeWidth={2.5} aria-hidden="true" />
                    )}
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="top">{updateHint}</TooltipContent>
              </Tooltip>
            )}
          </div>
        </div>
      </section>
      <UpdateInstallDialog open={dialogOpen} onOpenChange={setDialogOpen} />
    </>
  );
}

/**
 * 顶部平台标签页定义:一级平台域切换(Qoder / 灵犀为预留位,接入后补路由与页面即可)。
 * traeOnly:仅 Windows 或 webui 可用(与 TRAE 侧栏项可见性一致);comingSoon:置灰预留。
 */
type PlatformTab = "workbuddy" | "trae" | "qoder" | "lingxi";

const PLATFORM_TABS: {
  platform: PlatformTab;
  label: string;
  path: string;
  /** 仅 Windows 或 webui 可用(与 TRAE 侧栏项可见性一致) */
  traeOnly?: boolean;
  /** 置灰预留位 */
  comingSoon?: boolean;
}[] = [
  { platform: "workbuddy", label: "WorkBuddy", path: "/" },
  { platform: "trae", label: "TRAE", path: "/trae", traeOnly: true },
  { platform: "qoder", label: "Qoder", path: "/qoder", comingSoon: true },
  { platform: "lingxi", label: "灵犀", path: "/lingxi", comingSoon: true },
];

function PlatformTabs({ current, traeEnabled }: { current: PlatformTab; traeEnabled: boolean }) {
  const navigate = useNavigate();
  return (
    <div
      className="mb-3 grid grid-cols-2 gap-1 rounded-lg bg-foreground/[0.04] p-1"
      role="tablist"
      aria-label="平台切换"
    >
      {PLATFORM_TABS.map((tab) => {
        const disabled = tab.comingSoon === true || (tab.traeOnly === true && !traeEnabled);
        const active = current === tab.platform;
        return (
          <button
            key={tab.platform}
            type="button"
            role="tab"
            aria-selected={active}
            disabled={disabled}
            title={tab.comingSoon ? "即将上线" : undefined}
            onClick={() => navigate(tab.path)}
            className={cn(
              "rounded-md px-2 py-1.5 text-[13px] outline-none transition-colors focus-visible:ring-2 focus-visible:ring-sidebar-ring/50",
              active
                ? "bg-background font-medium text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground",
              disabled && "cursor-not-allowed opacity-40 hover:text-muted-foreground",
            )}
          >
            {tab.label}
          </button>
        );
      })}
    </div>
  );
}

function Layout() {
  const running = useAccountsStore((s) => s.status?.running);
  const hasUnifiedTitleBar =
    api.isDesktop() && typeof navigator !== "undefined" && navigator.userAgent.includes("Macintosh");
  useCreditAutoRefresh();
  useWorkbuddyStatusRefresh();
  useRotateDeferredNotice();

  /**
   * TRAE 侧栏项可见性：webui（服务端模式）始终显示；桌面端仅 Windows 显示
   * （TRAE 数据目录 / DPAPI / 多开依赖 Windows）；演示模式隐藏。
   */
  const showTraeNav =
    !demoModeEnabled && (!api.isDesktop() || navigator.userAgent.includes("Windows"));

  // 当前平台域由路由推导:/trae* 属 TRAE 域,其余(含 /settings)属 WorkBuddy 域
  const { pathname } = useLocation();
  const platform: PlatformTab = pathname.startsWith("/trae") ? "trae" : "workbuddy";

  return (
    <div className="flex h-screen min-h-0 overflow-hidden bg-background">
      {hasUnifiedTitleBar ? (
        <div
          data-tauri-drag-region
          className="fixed inset-x-0 top-0 z-50 h-8"
          aria-hidden="true"
        />
      ) : null}
      <aside
        className={cn(
          "flex min-h-0 w-[220px] shrink-0 flex-col border-r border-sidebar-border bg-sidebar px-3 pb-4",
          hasUnifiedTitleBar ? "pt-20" : "pt-4",
        )}
      >
        <div className="flex items-center gap-2.5 px-1 pb-5">
          <AppIconMark size={36} className="drop-shadow-sm" />
          <div className="min-w-0">
            <div
              className="truncate text-[15px] leading-5 tracking-[-0.02em] text-sidebar-foreground/90"
              style={{
                fontFamily: '"Bricolage Grotesque Variable", "SF Pro Display", ui-sans-serif, sans-serif',
                fontWeight: 640,
              }}
            >
              WorkBuddy Switch
            </div>
            {demoModeEnabled && (
              <Badge variant="secondary" className="mt-1 h-5 border-0 px-1.5 text-[10px] text-sidebar-foreground/60 shadow-none">
                演示模式
              </Badge>
            )}
          </div>
        </div>
        <PlatformTabs current={platform} traeEnabled={showTraeNav} />
        <nav className="flex min-h-0 flex-1 flex-col gap-0.5" aria-label="主导航">
          {platform === "workbuddy" ? (
            <>
              <NavLink
                to="/"
                end
                className={({ isActive }) =>
                  cn(
                    "flex items-center gap-2.5 rounded-lg px-3 py-2.5 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-sidebar-ring/50",
                    isActive
                      ? "bg-foreground/[0.06] font-medium text-foreground"
                      : "text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground",
                  )
                }
              >
                <User className="size-4" />
                账号管理
              </NavLink>
              <NavLink to="/token-stats" className={({ isActive }) => cn("flex items-center gap-2.5 rounded-lg px-3 py-2.5 text-sm outline-none transition-colors", isActive ? "bg-foreground/[0.06] font-medium text-foreground" : "text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground")}><MessagesSquare className="size-4" />Token 统计</NavLink>
              <NavLink
                to="/credit-stats"
                className={({ isActive }) =>
                  cn(
                    "flex items-center gap-2.5 rounded-lg px-3 py-2.5 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-sidebar-ring/50",
                    isActive
                      ? "bg-foreground/[0.06] font-medium text-foreground"
                      : "text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground",
                  )
                }
              >
                <Sparkles className="size-4" />
                积分统计
              </NavLink>
            </>
          ) : (
            <>
              <NavLink
                to="/trae"
                end
                className={({ isActive }) =>
                  cn(
                    "flex items-center gap-2.5 rounded-lg px-3 py-2.5 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-sidebar-ring/50",
                    isActive
                      ? "bg-foreground/[0.06] font-medium text-foreground"
                      : "text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground",
                  )
                }
              >
                <Rocket className="size-4" />
                账号管理
              </NavLink>
              <NavLink
                to="/trae/credits"
                className={({ isActive }) =>
                  cn(
                    "flex items-center gap-2.5 rounded-lg px-3 py-2.5 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-sidebar-ring/50",
                    isActive
                      ? "bg-foreground/[0.06] font-medium text-foreground"
                      : "text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground",
                  )
                }
              >
                <Sparkles className="size-4" />
                TRAE 积分
              </NavLink>
            </>
          )}
          <NavLink
            to="/settings"
            className={({ isActive }) =>
              cn(
                "flex items-center gap-2.5 rounded-lg px-3 py-2.5 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-sidebar-ring/50",
                isActive
                  ? "bg-foreground/[0.06] font-medium text-foreground"
                  : "text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground",
              )
            }
          >
            <Settings className="size-4" />
            设置
          </NavLink>
        </nav>
        {api.isWebui() && !demoModeEnabled ? null : <UpdateCenter running={running} />}
      </aside>
      <main
        className={cn(
          "min-w-0 flex-1 overflow-y-auto bg-background overscroll-contain",
          hasUnifiedTitleBar && "pt-16 [&>div]:pt-4",
        )}
      >
        <Outlet />
      </main>
    </div>
  );
}

export default function App() {
  const Router = pagesDemoHostingEnabled ? HashRouter : BrowserRouter;

  return (
    <TooltipProvider delayDuration={250}>
      <Router>
        <Routes>
          <Route element={<Layout />}>
            <Route path="/" element={<AccountsPage />} />
            <Route path="/credit-stats" element={<CreditStatsPage />} />
            <Route path="/token-stats" element={<TokenStatsPage />} />
            <Route path="/trae" element={<TraeAccountsPage />} />
            <Route path="/trae/credits" element={<TraeCreditsPage />} />
            <Route path="/settings" element={<SettingsPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Route>
        </Routes>
        <Toaster />
      </Router>
    </TooltipProvider>
  );
}
