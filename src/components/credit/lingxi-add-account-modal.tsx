import { useState } from "react";
import { toast } from "sonner";
import { Download, Loader2, Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import * as api from "@/lib/api";
import { useLingxiStore } from "@/stores/lingxi";

interface LingxiAddAccountModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * 添加灵犀账号弹窗：手动录入 checkinUrl 与 Cookie（浏览器 F12 抓包获取），
 * 或从本机 WPS 灵犀客户端一键导入当前登录态（仅 Windows，后端自会校验并报错）。
 */
export function LingxiAddAccountModal({ open, onOpenChange }: LingxiAddAccountModalProps) {
  const accounts = useLingxiStore((s) => s.accounts);
  const refreshAccounts = useLingxiStore((s) => s.refreshAccounts);

  const [manualName, setManualName] = useState("");
  const [manualUrl, setManualUrl] = useState("");
  const [manualCookie, setManualCookie] = useState("");
  const [importUrl, setImportUrl] = useState("");
  const [importing, setImporting] = useState(false);
  const [adding, setAdding] = useState(false);

  async function handleImportLocal() {
    // 导入地址:优先用导入区块的输入,留空则复用上方已填的签到 URL;
    // 两者都空时后端按灵犀主域(lingxi.wps.cn)匹配 Cookie,签到地址可后补
    if (importing) return;
    setImporting(true);
    try {
      const checkinUrl = (importUrl || manualUrl).trim();
      const account = await api.lingxiImportLocal(checkinUrl);
      const existed = accounts.some((a) => a.id === account.id);
      toast.success(existed ? "该登录态已导入过" : "导入成功", {
        description: existed
          ? account.name
          : checkinUrl
            ? `已添加账号：${account.name}`
            : `已添加账号：${account.name}（未填签到地址，签到前请在账号卡片中补填）`,
      });
      await refreshAccounts();
      setImportUrl("");
      onOpenChange(false);
    } catch (e) {
      // 后端消息(客户端运行中锁文件 / 未找到 Cookie / v20 不支持等)直接透出
      toast.error("导入失败", { description: api.asError(e) });
    } finally {
      setImporting(false);
    }
  }

  async function handleAddManual() {
    const name = manualName.trim();
    const checkinUrl = manualUrl.trim();
    const cookie = manualCookie.trim();
    if (!checkinUrl || !cookie) {
      toast.error("请填写 checkinUrl 与 Cookie");
      return;
    }
    if (adding) return;
    setAdding(true);
    try {
      const account = await api.lingxiAddAccount(name, checkinUrl, cookie);
      toast.success("账号已添加", { description: account.name });
      await refreshAccounts();
      setManualName("");
      setManualUrl("");
      setManualCookie("");
      onOpenChange(false);
    } catch (e) {
      toast.error("添加失败", { description: api.asError(e) });
    } finally {
      setAdding(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-[540px]">
        <DialogHeader>
          <DialogTitle>添加灵犀账号</DialogTitle>
          <DialogDescription>每个账号各自录入一份签到接口地址与 Cookie。</DialogDescription>
        </DialogHeader>
        <div className="space-y-3 pt-1">
          <div className="space-y-2 rounded-lg border bg-muted/20 p-3">
            <div className="space-y-1">
              <Label htmlFor="lingxi-import-url">从本机灵犀导入</Label>
              <p className="text-xs text-muted-foreground">
                解密本机 WPS 灵犀客户端的登录 Cookie（需本机已安装并登录灵犀；仅
                Windows，其他平台请手动录入）。多账号：在灵犀客户端切换登录后再次导入即可添加。
              </p>
            </div>
            <Input
              id="lingxi-import-url"
              value={importUrl}
              onChange={(e) => setImportUrl(e.target.value)}
              placeholder="签到接口地址（留空则复用上方已填的 URL）"
            />
            <Button
              variant="secondary"
              className="w-full"
              onClick={() => void handleImportLocal()}
              disabled={importing}
            >
              {importing ? <Loader2 className="animate-spin" /> : <Download />}
              {importing ? "导入中…" : "导入本机登录态"}
            </Button>
          </div>
          <div className="space-y-2">
            <Label htmlFor="lingxi-manual-name">名称</Label>
            <Input
              id="lingxi-manual-name"
              value={manualName}
              onChange={(e) => setManualName(e.target.value)}
              placeholder="账号显示名称（可留空，默认「灵犀账号」）"
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="lingxi-manual-url">签到接口地址（checkinUrl）</Label>
            <Input
              id="lingxi-manual-url"
              value={manualUrl}
              onChange={(e) => setManualUrl(e.target.value)}
              placeholder="POST https://…"
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="lingxi-manual-cookie">Cookie</Label>
            <Input
              id="lingxi-manual-cookie"
              value={manualCookie}
              onChange={(e) => setManualCookie(e.target.value)}
              placeholder="粘贴完整 Cookie 请求头"
            />
          </div>
          <div className="rounded-lg border border-dashed bg-muted/30 px-3 py-3 text-xs leading-5 text-muted-foreground">
            <p className="font-medium text-foreground">抓包获取方式（浏览器 F12）：</p>
            <ol className="mt-1 list-decimal space-y-0.5 pl-4">
              <li>浏览器登录灵犀后按 F12 打开开发者工具，切到「网络 / Network」；</li>
              <li>在灵犀页面上手动完成一次签到，找到该次签到请求（POST）；</li>
              <li>右键该请求 →「复制 → Copy as cURL」，从中提取 URL 与 Cookie 头分别粘贴到上方。</li>
            </ol>
            <p className="mt-1">Cookie 过期后需重新抓包，并在账号卡片「编辑」中更新。</p>
          </div>
          <Button className="w-full" onClick={() => void handleAddManual()} disabled={adding}>
            {adding ? <Loader2 className="animate-spin" /> : <Plus />}
            {adding ? "添加中…" : "添加账号"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
