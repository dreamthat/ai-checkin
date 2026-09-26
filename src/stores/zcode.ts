import { create } from "zustand";

import * as api from "@/lib/api";
import type { CreditLogEntry, ZcodeAccount, ZcodeClaimResult, ZcodeSettings } from "@/lib/types";

/** 单独刷新领取日志（领取流程复用；失败静默保留旧数据）。 */
async function reloadLogs() {
  try {
    const logs = await api.zcodeGetLogs();
    useZcodeStore.setState({ logs });
  } catch {
    /* 静默 */
  }
}

/**
 * ZCode 领取子系统状态：与 stores/qoder.ts 同构（claim 措辞）。
 * 调度由后端按「启用自动领取」开关自动启停，无独立的启动/停止命令（区别于 TRAE）。
 */
interface ZcodeStore {
  accounts: ZcodeAccount[];
  logs: CreditLogEntry[];
  settings: ZcodeSettings | null;
  /** 下次定时领取时间（ISO 字符串；未启用为 null）。 */
  nextRunAt: string | null;
  loading: boolean;
  error: string | null;
  fetchAll: () => Promise<void>;
  /** 单账号领取：完成后同步列表与日志。 */
  claimOne: (id: string) => Promise<ZcodeClaimResult>;
  /** 一键领取全部启用账号。 */
  claimAll: () => Promise<void>;
  saveSettings: (partial: Partial<ZcodeSettings>) => Promise<ZcodeSettings>;
  clearLogs: () => Promise<void>;
  /** 仅刷新账号列表（领取回调复用；失败静默保留旧列表）。 */
  refreshAccounts: () => Promise<void>;
}

export const useZcodeStore = create<ZcodeStore>((set, get) => ({
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
        api.zcodeGetAccounts(),
        api.zcodeGetLogs(),
        api.zcodeGetSettings(),
        api.zcodeGetNextRunTime(),
      ]);
      set({ accounts, logs, settings, nextRunAt, loading: false });
    } catch (e) {
      set({ error: api.asError(e), loading: false });
    }
  },

  async claimOne(id) {
    const result = await api.zcodeClaimAccount(id);
    await get().refreshAccounts();
    await reloadLogs();
    return result;
  },

  async claimAll() {
    await api.zcodeClaimAll();
    await get().refreshAccounts();
    await reloadLogs();
  },

  async saveSettings(partial) {
    const settings = await api.zcodeSaveSettings(partial);
    set({ settings });
    try {
      set({ nextRunAt: await api.zcodeGetNextRunTime() });
    } catch {
      /* 下次执行时间读取失败不打扰保存结果 */
    }
    return settings;
  },

  async clearLogs() {
    await api.zcodeClearLogs();
    set({ logs: await api.zcodeGetLogs() });
  },

  async refreshAccounts() {
    try {
      set({ accounts: await api.zcodeGetAccounts() });
    } catch {
      /* 静默：保留最后一次成功列表 */
    }
  },
}));
