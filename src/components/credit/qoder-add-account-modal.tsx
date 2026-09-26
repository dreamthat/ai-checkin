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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import * as api from "@/lib/api";
import type { CreditRegion } from "@/lib/types";
import { useQoderStore } from "@/stores/qoder";

interface QoderAddAccountModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** 本机导入依赖 DPAPI 与本机客户端数据目录，仅桌面 Windows 可用。 */
const canImportLocal = api.isDesktop() && navigator.userAgent.includes("Windows");

/**
 * 添加 Qoder 账号弹窗：
 * 导入本机账号（读取 Qoder 客户端 auth.v1.dat 与 IDE 数据目录的 token，仅桌面 Windows）
 * / 手动录入 token（国际版需本机安装 Qoder 客户端才有每日积分活动）。
 */
export function QoderAddAccountModal({ open, onOpenChange }: QoderAddAccountModalProps) {
  const refreshAccounts = useQoderStore((s) => s.refreshAccounts);
  const defaultRegion = useQoderStore((s) => s.settings?.region ?? "intl");

  const [importing, setImporting] = useState(false);
  const [manualName, setManualName] = useState("");
  const [manualToken, setManualToken] = useState("");
  const [manualRegion, setManualRegion] = useState<CreditRegion>(defaultRegion);
  const [adding, setAdding] = useState(false);

  async function handleImportLocal() {
    if (importing) return;
    setImporting(true);
    try {
      const accounts = await api.qoderImportLocal();
      await refreshAccounts();
      if (accounts.length === 0) {
        toast.info("未发现可导入的账号", {
          description: "请确认本机已登录 Qoder 客户端，或 IDE 中存在 dt-*/pt-* token",
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
      const account = await api.qoderAddAccount(name, token, manualRegion);
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
          <DialogTitle>添加 Qoder 账号</DialogTitle>
          <DialogDescription>从本机客户端导入，或手动录入 token。</DialogDescription>
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

          {/* 入口一：导入本机账号（仅桌面 Windows：依赖 DPAPI 与本机客户端数据） */}
          <TabsContent value="local" className="space-y-3 pt-2">
            <p className="text-sm leading-6 text-muted-foreground">
              读取本机 Qoder 客户端登录凭据与 IDE 数据目录中的 token（dt-* / pt-*），批量导入。
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
              <Label htmlFor="qoder-manual-name">名称</Label>
              <Input
                id="qoder-manual-name"
                value={manualName}
                onChange={(e) => setManualName(e.target.value)}
                placeholder="账号显示名称"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="qoder-manual-token">Token</Label>
              <Input
                id="qoder-manual-token"
                value={manualToken}
                onChange={(e) => setManualToken(e.target.value)}
                placeholder="粘贴 dt-* / pt-* token"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="qoder-manual-region">区域</Label>
              <Select value={manualRegion} onValueChange={(v) => setManualRegion(v as CreditRegion)}>
                <SelectTrigger id="qoder-manual-region" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="intl">国际版（intl）</SelectItem>
                  <SelectItem value="cn">国内版（cn）</SelectItem>
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                国际版的每日积分活动需要本机安装并登录 Qoder 客户端（设备风控身份）。
              </p>
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
