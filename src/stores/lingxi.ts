import { create } from "zustand";

import * as api from "@/lib/api";
import type { CreditLogEntry, LingxiAccount, LingxiCheckinResult, LingxiSettings } from "@/lib/types";

/** 单独刷新签到日志（签到流程复用；失败静默保留旧数据）。 */
async function reloadLogs() {
  try {
    const logs = await api.lingxiGetLogs();
    useLingxiStore.setState({ logs });
  } catch {
    /* 静默 */
  }
}

/**
 * 灵犀签到子系统状态：账号 / 日志 / 设置 / 调度。
 * 数据只在桌面端或 webui 服务端模式取数；调度由后端按有效签到时间点自动启停
 * （时间点列表为空即不调度），无独立的启动/停止命令（区别于 TRAE）。
 */
interface LingxiStore {
  accounts: LingxiAccount[];
  logs: CreditLogEntry[];
  settings: LingxiSettings | null;
  /** 下次定时签到时间（ISO 字符串；无有效时间点为 null）。 */
  nextRunAt: string | null;
  loading: boolean;
  error: string | null;
  fetchAll: () => Promise<void>;
  /** 单账号签到：完成后同步列表与日志。 */
  checkinOne: (id: string) => Promise<LingxiCheckinResult>;
  /** 一键签到全部启用账号。 */
  checkinAll: () => Promise<void>;
  saveSettings: (partial: Partial<LingxiSettings>) => Promise<LingxiSettings>;
  clearLogs: () => Promise<void>;
  /** 仅刷新账号列表（签到回调复用；失败静默保留旧列表）。 */
  refreshAccounts: () => Promise<void>;
}

export const useLingxiStore = create<LingxiStore>((set, get) => ({
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
        api.lingxiGetAccounts(),
        api.lingxiGetLogs(),
        api.lingxiGetSettings(),
        api.lingxiGetNextRunTime(),
      ]);
      set({ accounts, logs, settings, nextRunAt, loading: false });
    } catch (e) {
      set({ error: api.asError(e), loading: false });
    }
  },

  async checkinOne(id) {
    const result = await api.lingxiCheckinAccount(id);
    await get().refreshAccounts();
    await reloadLogs();
    return result;
  },

  async checkinAll() {
    await api.lingxiCheckinAll();
    await get().refreshAccounts();
    await reloadLogs();
  },

  async saveSettings(partial) {
    const settings = await api.lingxiSaveSettings(partial);
    set({ settings });
    try {
      set({ nextRunAt: await api.lingxiGetNextRunTime() });
    } catch {
      /* 下次执行时间读取失败不打扰保存结果 */
    }
    return settings;
  },

  async clearLogs() {
    await api.lingxiClearLogs();
    set({ logs: await api.lingxiGetLogs() });
  },

  async refreshAccounts() {
    try {
      set({ accounts: await api.lingxiGetAccounts() });
    } catch {
      /* 静默：保留最后一次成功列表 */
    }
  },
}));
