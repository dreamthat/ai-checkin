// Qoder / ZCode 领取子系统共用展示辅助（卡片与页面日志表共用）。
// 结果口径与 crates/credit-core/src/models.rs 的 ClaimOutcome 一致。

import { cn } from "@/lib/utils";
import type { QoderQuotaUsage } from "@/lib/types";

function pad(v: number): string {
  return String(v).padStart(2, "0");
}

/** 卡片信息行的日期时间（MM-DD HH:mm）。 */
export function formatCreditTime(ms: number): string {
  const d = new Date(ms);
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 日志表的时间（MM-DD HH:mm:ss）。 */
export function formatCreditLogTime(ms: number): string {
  const d = new Date(ms);
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

type BadgeVariant = "default" | "secondary" | "destructive" | "outline";

/** 领取结果 → 徽标展示态；need-captcha 用橙色「需手动领取」提示。 */
export function claimOutcomeBadge(result: string | null | undefined): {
  text: string;
  variant: BadgeVariant;
  className?: string;
} {
  switch (result) {
    case "checked-in":
      return { text: "已领取", variant: "default" };
    case "already":
      return { text: "今日已领", variant: "default" };
    case "no-activity":
      return { text: "无可领活动", variant: "outline" };
    case "failed":
      return { text: "领取失败", variant: "destructive" };
    case "need-captcha":
      return {
        text: "需手动领取",
        variant: "outline",
        className: "border-amber-500/50 text-amber-600 dark:text-amber-500",
      };
    default:
      return { text: "未领取", variant: "secondary" };
  }
}

/** 成功口径（checked-in / already），今日摘要计数用（与 core ClaimOutcome::is_success 一致）。 */
export function isClaimSuccess(result: string | null | undefined): boolean {
  return result === "checked-in" || result === "already";
}

/** 灵犀签到结果 → 徽标展示态；unknown 用黄色「结果未知」提示（core 会在下个时间点重试）。 */
export function lingxiOutcomeBadge(result: string | null | undefined): {
  text: string;
  variant: BadgeVariant;
  className?: string;
} {
  switch (result) {
    case "success":
      return { text: "签到成功", variant: "default" };
    case "already":
      return { text: "已签到", variant: "secondary" };
    case "unknown":
      return {
        text: "结果未知",
        variant: "outline",
        className: "border-amber-500/50 text-amber-600 dark:text-amber-500",
      };
    case "failed":
      return { text: "签到失败", variant: "destructive" };
    case "skipped":
      return { text: "已跳过", variant: "secondary" };
    default:
      return { text: "未签到", variant: "secondary" };
  }
}

/** 灵犀成功口径（success / already），今日摘要计数用（与 core Outcome::is_success 一致）。 */
export function isLingxiSuccess(result: string | null | undefined): boolean {
  return result === "success" || result === "already";
}

/**
 * Qoder 额度原始 JSON（/api/v2/quota/usage）→ 剩余/总额。
 * 口径同 CreditDaddy normalizeQoderQuota：userQuota / addOnQuota / orgResourcePackage 求和（total > 0 的部分）。
 */
export function summarizeQoderQuota(
  quota: QoderQuotaUsage | null | undefined,
): { remaining: number; total: number } | null {
  if (!quota || typeof quota !== "object") return null;
  const parts = [quota.userQuota, quota.addOnQuota, quota.orgResourcePackage];
  let total = 0;
  let remaining = 0;
  let any = false;
  for (const p of parts) {
    const t = Number(p?.total) || 0;
    if (t > 0) {
      any = true;
      total += t;
      remaining += Number(p?.remaining) || 0;
    }
  }
  if (!any) return null;
  return {
    total: Math.round(total * 100) / 100,
    remaining: Math.round(remaining * 100) / 100,
  };
}

/** 额度文本：remaining/total（可选单位）。 */
export function formatQuotaAmount(value: number, unit?: string): string {
  const rounded = Math.round(value * 100) / 100;
  return unit ? `${rounded} ${unit}` : String(rounded);
}

/** 平台域通用卡片外框样式（与 trae-account-card 同款）。 */
export const creditCardClass = cn(
  "flex min-w-0 flex-col rounded-xl border border-border bg-card p-5 shadow-sm",
);
