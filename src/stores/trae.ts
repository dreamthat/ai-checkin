import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";

import * as api from "@/lib/api";
import type { TraeAccount, TraeCheckinLog, TraeCheckinResult, TraeSettings } from "@/lib/types";

/** 单独刷新签到日志（事件与签到流程复用；失败静默保留旧数据）。 */
async function reloadLogs() {
  try {
    const logs = await api.traeGetLogs();
    useTraeStore.setState({ logs });
  } catch {
    /* 静默 */
  }
}

/**
 * TRAE 子系统状态（trae-mate 合并）：账号 / 日志 / 设置 / 调度。
 * 数据只在桌面端或 webui 服务端模式取数；演示模式侧栏隐藏、路由兜底跳转，不会进入本 store。
 */
interface TraeState {
  accounts: TraeAccount[];
  logs: TraeCheckinLog[];
  settings: TraeSettings | null;
  /** 下次定时签到时间（ISO 字符串；未启用为 null）。 */
  nextRunAt: string | null;
  loading: boolean;
  error: string | null;
  fetchAll: () => Promise<void>;
  /** 单账号签到：成功后顺带刷新总积分并同步列表与日志。 */
  checkinOne: (id: string) => Promise<TraeCheckinResult>;
  /** 一键签到全部启用账号。 */
  checkinAll: () => Promise<void>;
  saveSettings: (partial: Partial<TraeSettings>) => Promise<TraeSettings>;
  startScheduler: () => Promise<void>;
  stopScheduler: () => Promise<void>;
  clearLogs: () => Promise<void>;
  /** 仅刷新账号列表（事件回调复用；失败静默保留旧列表）。 */
  refreshAccounts: () => Promise<void>;
}

export const useTraeStore = create<TraeState>((set, get) => ({
  accounts: [],
  logs: [],
  settings: null,
  nextRunAt: null,
  loading: false,
  error: null,

  async fetchAll() {
    set({ loading: true, error: null });
    try {
      const [accounts, logs, settings, nextRunAt] = await Promise.all([
        api.traeGetAccounts(),
        api.traeGetLogs(),
        api.traeGetSettings(),
        api.traeGetNextRunTime(),
      ]);
      set({ accounts, logs, settings, nextRunAt, loading: false });
    } catch (e) {
      set({ error: api.asError(e), loading: false });
    }
  },

  async checkinOne(id) {
    const result = await api.traeCheckinAccount(id);
    // 成功后顺带刷新总积分（后端同时回写账号 points）；查询失败不影响签到回执
    if (result.success) {
      try {
        await api.traeGetAccountPoints(id);
      } catch {
        /* 总积分查询失败不影响签到结果 */
      }
    }
    await get().refreshAccounts();
    await reloadLogs();
    return result;
  },

  async checkinAll() {
    await api.traeCheckinAll();
    await get().refreshAccounts();
    await reloadLogs();
  },

  async saveSettings(partial) {
    const settings = await api.traeSaveSettings(partial);
    set({ settings });
    try {
      set({ nextRunAt: await api.traeGetNextRunTime() });
    } catch {
      /* 下次执行时间读取失败不打扰保存结果 */
    }
    return settings;
  },

  async startScheduler() {
    await api.traeStartScheduler();
    try {
      set({ nextRunAt: await api.traeGetNextRunTime() });
    } catch {
      /* 忽略 */
    }
  },

  async stopScheduler() {
    await api.traeStopScheduler();
    set({ nextRunAt: null });
  },

  async clearLogs() {
    await api.traeClearLogs();
    set({ logs: await api.traeGetLogs() });
  },

  async refreshAccounts() {
    try {
      set({ accounts: await api.traeGetAccounts() });
    } catch {
      /* 静默：保留最后一次成功列表 */
    }
  },
}));

let eventsInstalled = false;

/**
 * 监听桌面端 TRAE 事件（页面挂载时调用一次，重复调用无副作用）。
 * - `trae-checkin-start` / `trae-checkin-progress`：进度事件，界面按签到回执更新，这里不动作；
 * - `trae-checkin-done`：整批签到结束，触发 fetchAll() 同步账号与日志；
 * - `trae-login-imported`：新实例登录导入完成，成功刷新列表、失败提示原因。
 * 仅桌面宿主有事件通道（webui 无事件桥，演示模式不订阅）。
 */
export async function initTraeEvents(): Promise<void> {
  if (eventsInstalled || !api.isDesktop()) return;
  eventsInstalled = true;
  const noop = () => {};
  await Promise.all([
    listen("trae-checkin-start", noop),
    listen("trae-checkin-progress", noop),
    listen("trae-checkin-done", () => {
      void useTraeStore.getState().fetchAll();
    }),
    listen<{ success: boolean; name?: string; userId?: string; error?: string }>(
      "trae-login-imported",
      (e) => {
        const payload = e.payload;
        if (payload?.success) {
          toast.success("登录账号已导入", {
            description: payload.name || payload.userId || undefined,
          });
          void useTraeStore.getState().fetchAll();
        } else {
          toast.error("登录导入失败", { description: payload?.error || "未检测到登录" });
        }
      },
    ),
  ]);
}
