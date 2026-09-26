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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import * as api from "@/lib/api";
import { useZcodeStore } from "@/stores/zcode";

interface ZcodeAddAccountModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** 本机导入依赖凭据解密与本机数据目录，仅桌面 Windows 可用。 */
const canImportLocal = api.isDesktop() && navigator.userAgent.includes("Windows");

/**
 * 添加 ZCode 账号弹窗（与 Qoder 同构，无区域选择）：
 * 导入本机账号（读取 ~/.zcode/v2/credentials.json，仅桌面 Windows）/ 手动录入 token。
 */
export function ZcodeAddAccountModal({ open, onOpenChange }: ZcodeAddAccountModalProps) {
  const refreshAccounts = useZcodeStore((s) => s.refreshAccounts);

  const [importing, setImporting] = useState(false);
  const [manualName, setManualName] = useState("");
  const [manualToken, setManualToken] = useState("");
  const [adding, setAdding] = useState(false);

  async function handleImportLocal() {
    if (importing) return;
    setImporting(true);
    try {
      const accounts = await api.zcodeImportLocal();
      await refreshAccounts();
      if (accounts.length === 0) {
        toast.info("未发现可导入的账号", {
          description: "请确认本机 ~/.zcode/v2/credentials.json 中已登录账号",
        });
      } else {
        toast.success(`已导入 ${accounts.length} 个本机账号`);
        onOpenChange(false);
      }
    } catch (e) {
      toast.error("导入失败", { description: api.asError(e) });
    } finally {
      setImporting(false);
    }
  }

  async function handleAddManual() {
    const name = manualName.trim();
    const token = manualToken.trim();
    if (!name || !token) {
      toast.error("请填写名称与 token");
      return;
    }
    if (adding) return;
    setAdding(true);
    try {
      const account = await api.zcodeAddAccount(name, token);
      toast.success("账号已添加", { description: account.name });
      await refreshAccounts();
      setManualName("");
      setManualToken("");
      onOpenChange(false);
    } catch (e) {
      toast.error("添加失败", { description: api.asError(e) });
    } finally {
      setAdding(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-[520px]">
        <DialogHeader>
          <DialogTitle>添加 ZCode 账号</DialogTitle>
          <DialogDescription>从本机凭据导入，或手动录入 token。</DialogDescription>
        </DialogHeader>
        <Tabs defaultValue={canImportLocal ? "local" : "manual"}>
          <TabsList className="w-full">
            <TabsTrigger value="local" className="flex-1">
              导入本机账号
            </TabsTrigger>
            <TabsTrigger value="manual" className="flex-1">
              手动录入
            </TabsTrigger>
          </TabsList>

          {/* 入口一：导入本机账号（仅桌面 Windows：依赖凭据解密与本机数据目录） */}
          <TabsContent value="local" className="space-y-3 pt-2">
            <p className="text-sm leading-6 text-muted-foreground">
              读取本机 ~/.zcode/v2/credentials.json 中已登录的账号并导入（凭据保持密文快照）。
            </p>
            {canImportLocal ? (
              <Button className="w-full" onClick={() => void handleImportLocal()} disabled={importing}>
                {importing ? <Loader2 className="animate-spin" /> : <Download />}
                {importing ? "导入中…" : "扫描并导入本机账号"}
              </Button>
            ) : (
              <p className="rounded-lg border border-dashed px-3 py-6 text-center text-sm text-muted-foreground">
                本机导入仅支持桌面版（Windows）。
              </p>
            )}
          </TabsContent>

          {/* 入口二：手动录入 token */}
          <TabsContent value="manual" className="space-y-3 pt-2">
            <div className="space-y-2">
              <Label htmlFor="zcode-manual-name">名称</Label>
              <Input
                id="zcode-manual-name"
                value={manualName}
                onChange={(e) => setManualName(e.target.value)}
                placeholder="账号显示名称"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="zcode-manual-token">Token</Label>
              <Input
                id="zcode-manual-token"
                value={manualToken}
                onChange={(e) => setManualToken(e.target.value)}
                placeholder="粘贴 API Key / zcodejwttoken"
              />
            </div>
            <Button className="w-full" onClick={() => void handleAddManual()} disabled={adding}>
              {adding ? <Loader2 className="animate-spin" /> : <Plus />}
              {adding ? "添加中…" : "添加账号"}
            </Button>
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}
