import { create } from "zustand";

import * as api from "@/lib/api";
import type { CreditLogEntry, QoderAccount, QoderCheckinResult, QoderSettings } from "@/lib/types";

/** 单独刷新领取日志（领取流程复用；失败静默保留旧数据）。 */
async function reloadLogs() {
  try {
    const logs = await api.qoderGetLogs();
    useQoderStore.setState({ logs });
  } catch {
    /* 静默 */
  }
}

/**
 * Qoder 领取子系统状态：账号 / 日志 / 设置 / 调度。
 * 数据只在桌面端或 webui 服务端模式取数；调度由后端按「启用自动领取」开关自动启停，
 * 无独立的启动/停止命令（区别于 TRAE）。
 */
interface QoderStore {
  accounts: QoderAccount[];
  logs: CreditLogEntry[];
  settings: QoderSettings | null;
  /** 下次定时领取时间（ISO 字符串；未启用为 null）。 */
  nextRunAt: string | null;
  loading: boolean;
  error: string | null;
  fetchAll: () => Promise<void>;
  /** 单账号领取：完成后同步列表与日志。 */
  checkinOne: (id: string) => Promise<QoderCheckinResult>;
  /** 一键领取全部启用账号。 */
  checkinAll: () => Promise<void>;
  saveSettings: (partial: Partial<QoderSettings>) => Promise<QoderSettings>;
  clearLogs: () => Promise<void>;
  /** 仅刷新账号列表（领取回调复用；失败静默保留旧列表）。 */
  refreshAccounts: () => Promise<void>;
}

export const useQoderStore = create<QoderStore>((set, get) => ({
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
        api.qoderGetAccounts(),
        api.qoderGetLogs(),
        api.qoderGetSettings(),
        api.qoderGetNextRunTime(),
      ]);
      set({ accounts, logs, settings, nextRunAt, loading: false });
    } catch (e) {
      set({ error: api.asError(e), loading: false });
    }
  },

  async checkinOne(id) {
    const result = await api.qoderCheckinAccount(id);
    await get().refreshAccounts();
    await reloadLogs();
    return result;
  },

  async checkinAll() {
    await api.qoderCheckinAll();
    await get().refreshAccounts();
    await reloadLogs();
  },

  async saveSettings(partial) {
    const settings = await api.qoderSaveSettings(partial);
    set({ settings });
    try {
      set({ nextRunAt: await api.qoderGetNextRunTime() });
    } catch {
      /* 下次执行时间读取失败不打扰保存结果 */
    }
    return settings;
  },

  async clearLogs() {
    await api.qoderClearLogs();
    set({ logs: await api.qoderGetLogs() });
  },

  async refreshAccounts() {
    try {
      set({ accounts: await api.qoderGetAccounts() });
    } catch {
      /* 静默：保留最后一次成功列表 */
    }
  },
}));
