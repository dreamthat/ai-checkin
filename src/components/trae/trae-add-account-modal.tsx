import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Download, FolderSearch, Loader2, LogIn, Plus } from "lucide-react";

import { Badge } from "@/components/ui/badge";
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
import type { TraeInstanceDirInfo, TraeJwtInfo, TraeAccount } from "@/lib/types";
import { useTraeStore } from "@/stores/trae";

/** 轻量辅助：只同步账号列表到 store（弹窗内避免整页 fetchAll 的 loading 闪烁）。 */
function setTraeAccounts(accounts: TraeAccount[]): void {
  useTraeStore.setState({ accounts });
}

async function refreshTraeAccounts(): Promise<void> {
  await useTraeStore.getState().refreshAccounts();
}

/** JWT 粘贴预览的防抖等待。 */
const JWT_PREVIEW_DEBOUNCE_MS = 400;

function formatExpiry(ms: number): string {
  if (!ms) return "未知";
  const d = new Date(ms);
  const pad = (v: number) => String(v).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}/${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

interface TraeAddAccountModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * 添加 TRAE 账号弹窗（对应 trae-mate AddAccountModal.vue）：
 * 四个入口 —— 导入桌面账号 / 新开登录实例（登录后自动导入）/ 扫描已有多开目录 /
 * JWT 手动录入（粘贴实时预览 userId 与剩余小时）。
 */
export function TraeAddAccountModal({ open, onOpenChange }: TraeAddAccountModalProps) {
  const [importing, setImporting] = useState(false);
  const [openingLogin, setOpeningLogin] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [dirs, setDirs] = useState<TraeInstanceDirInfo[] | null>(null);
  const [importingDir, setImportingDir] = useState<string | null>(null);

  // JWT 手动录入
  const [jwtName, setJwtName] = useState("");
  const [jwt, setJwt] = useState("");
  const [jwtRefreshToken, setJwtRefreshToken] = useState("");
  const [jwtPreview, setJwtPreview] = useState<TraeJwtInfo | null>(null);
  const [jwtPreviewError, setJwtPreviewError] = useState<string | null>(null);
  const [addingJwt, setAddingJwt] = useState(false);
  const previewTimer = useRef<number | undefined>(undefined);

  // JWT 粘贴防抖预览：解析 userId / 剩余小时
  useEffect(() => {
    window.clearTimeout(previewTimer.current);
    if (!jwt.trim()) {
      setJwtPreview(null);
      setJwtPreviewError(null);
      return;
    }
    previewTimer.current = window.setTimeout(() => {
      void api
        .traeJwtParsePreview(jwt.trim())
        .then((info) => {
          setJwtPreview(info);
          setJwtPreviewError(null);
        })
        .catch((e) => {
          setJwtPreview(null);
          setJwtPreviewError(api.asError(e));
        });
    }, JWT_PREVIEW_DEBOUNCE_MS);
    return () => window.clearTimeout(previewTimer.current);
  }, [jwt]);

  async function handleImportDesktop() {
    if (importing) return;
    setImporting(true);
    try {
      const account = await api.traeImportDesktopAccount();
      setTraeAccounts(await api.traeGetAccounts());
      toast.success("已导入当前 TRAE 桌面账号", { description: account.name });
      onOpenChange(false);
    } catch (e) {
      toast.error("导入失败", { description: api.asError(e) });
    } finally {
      setImporting(false);
    }
  }

  async function handleOpenLoginInstance() {
    if (openingLogin) return;
    setOpeningLogin(true);
    try {
      await api.traeOpenNewLoginInstance();
      toast.info("已打开新 TRAE 实例", {
        description: "请在打开的实例中登录；登录完成后会自动导入该账号",
      });
      onOpenChange(false);
    } catch (e) {
      toast.error("打开登录实例失败", { description: api.asError(e) });
    } finally {
      setOpeningLogin(false);
    }
  }

  async function handleScanDirs() {
    if (scanning) return;
    setScanning(true);
    try {
      setDirs(await api.traeScanInstanceDirs());
    } catch (e) {
      toast.error("扫描失败", { description: api.asError(e) });
    } finally {
      setScanning(false);
    }
  }

  async function handleImportDir(dataDir: string) {
    if (importingDir) return;
    setImportingDir(dataDir);
    try {
      const account = await api.traeImportAccountFromDir(dataDir);
      toast.success("已从目录导入账号", { description: account.name });
      await refreshTraeAccounts();
      onOpenChange(false);
    } catch (e) {
      toast.error("导入失败", { description: api.asError(e) });
    } finally {
      setImportingDir(null);
    }
  }

  async function handleAddJwt() {
    const name = jwtName.trim();
    const token = jwt.trim();
    if (!name || !token) {
      toast.error("请填写名称与 JWT");
      return;
    }
    if (addingJwt) return;
    setAddingJwt(true);
    try {
      const account = await api.traeAccountAddJwt(
        name,
        token,
        jwtRefreshToken.trim() || undefined,
      );
      toast.success("JWT 账号已添加", { description: account.name });
      await refreshTraeAccounts();
      setJwtName("");
      setJwt("");
      setJwtRefreshToken("");
      setJwtPreview(null);
      onOpenChange(false);
    } catch (e) {
      toast.error("添加失败", { description: api.asError(e) });
    } finally {
      setAddingJwt(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-[560px]">
        <DialogHeader>
          <DialogTitle>添加 TRAE 账号</DialogTitle>
          <DialogDescription>
            从桌面客户端导入、新开实例登录、扫描已有多开目录，或手动录入 JWT。
          </DialogDescription>
        </DialogHeader>
        <Tabs defaultValue="desktop">
          <TabsList className="w-full">
            <TabsTrigger value="desktop" className="flex-1">
              桌面账号
            </TabsTrigger>
            <TabsTrigger value="login" className="flex-1">
              登录实例
            </TabsTrigger>
            <TabsTrigger value="dirs" className="flex-1">
              扫描目录
            </TabsTrigger>
            <TabsTrigger value="jwt" className="flex-1">
              JWT
            </TabsTrigger>
          </TabsList>

          {/* 入口一：导入当前 TRAE 桌面账号 */}
          <TabsContent value="desktop" className="space-y-3 pt-2">
            <p className="text-sm leading-6 text-muted-foreground">
              读取本机 TRAE 桌面客户端（TRAE SOLO CN）当前登录账号的凭据并导入，导入后可多开独立实例与自动签到。
            </p>
            <Button className="w-full" onClick={() => void handleImportDesktop()} disabled={importing}>
              {importing ? <Loader2 className="animate-spin" /> : <Download />}
              {importing ? "导入中…" : "导入当前桌面账号"}
            </Button>
          </TabsContent>

          {/* 入口二：新开空白实例登录，后端轮询登录完成后自动导入 */}
          <TabsContent value="login" className="space-y-3 pt-2">
            <p className="text-sm leading-6 text-muted-foreground">
              启动一个全新的 TRAE 实例供你登录。登录完成后本工具会自动关闭该实例、导入账号并绑定独立数据目录，无需手动操作。
            </p>
            <Button className="w-full" onClick={() => void handleOpenLoginInstance()} disabled={openingLogin}>
              {openingLogin ? <Loader2 className="animate-spin" /> : <LogIn />}
              {openingLogin ? "正在打开…" : "新开登录实例"}
            </Button>
          </TabsContent>

          {/* 入口三：扫描已有多开/登录临时目录 */}
          <TabsContent value="dirs" className="space-y-3 pt-2">
            <p className="text-sm leading-6 text-muted-foreground">
              扫描 %APPDATA% 下已存在的 TRAE 多开与登录临时目录，选择需要的导入（已绑定的会标注）。
            </p>
            <Button variant="outline" className="w-full" onClick={() => void handleScanDirs()} disabled={scanning}>
              {scanning ? <Loader2 className="animate-spin" /> : <FolderSearch />}
              {scanning ? "扫描中…" : "扫描已有多开目录"}
            </Button>
            {dirs !== null && (
              dirs.length === 0 ? (
                <p className="rounded-lg border border-dashed px-3 py-6 text-center text-sm text-muted-foreground">
                  未发现可导入的目录
                </p>
              ) : (
                <div className="overflow-hidden rounded-lg border">
                  <table className="w-full text-xs">
                    <thead className="bg-muted/50 text-muted-foreground">
                      <tr>
                        <th className="px-2.5 py-2 text-left font-medium">账号</th>
                        <th className="px-2.5 py-2 text-left font-medium">过期</th>
                        <th className="px-2.5 py-2 text-left font-medium">状态</th>
                        <th className="px-2.5 py-2" />
                      </tr>
                    </thead>
                    <tbody>
                      {dirs.map((dir) => (
                        <tr key={dir.dataDir} className="border-t">
                          <td className="max-w-[160px] truncate px-2.5 py-2" title={dir.accountName || dir.userId}>
                            {dir.accountName || dir.userId || "未知账号"}
                            {dir.bound && (
                              <Badge variant="secondary" className="ml-1.5 h-4 rounded px-1 text-[10px]">
                                已绑定
                              </Badge>
                            )}
                            {dir.running && (
                              <Badge variant="outline" className="ml-1 h-4 rounded px-1 text-[10px]">
                                运行中
                              </Badge>
                            )}
                            {!dir.hasSigningKey && (
                              <Badge variant="outline" className="ml-1 h-4 rounded px-1 text-[10px]">
                                无签名密钥
                              </Badge>
                            )}
                          </td>
                          <td className="px-2.5 py-2 text-muted-foreground">
                            {formatExpiry(dir.expiresAt)}
                          </td>
                          <td className="px-2.5 py-2 text-muted-foreground" title={dir.dataDir}>
                            {dir.running ? "运行中" : "未运行"}
                          </td>
                          <td className="px-2.5 py-2 text-right">
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-7 px-2"
                              disabled={dir.bound || importingDir !== null}
                              onClick={() => void handleImportDir(dir.dataDir)}
                            >
                              {importingDir === dir.dataDir && <Loader2 className="size-3 animate-spin" />}
                              {dir.bound ? "已绑定" : "导入"}
                            </Button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )
            )}
          </TabsContent>

          {/* 入口四：JWT 手动录入（粘贴实时预览） */}
          <TabsContent value="jwt" className="space-y-3 pt-2">
            <div className="space-y-2">
              <Label htmlFor="trae-jwt-name">名称</Label>
              <Input
                id="trae-jwt-name"
                value={jwtName}
                onChange={(e) => setJwtName(e.target.value)}
                placeholder="账号显示名称"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="trae-jwt-token">JWT</Label>
              <Input
                id="trae-jwt-token"
                value={jwt}
                onChange={(e) => setJwt(e.target.value)}
                placeholder="粘贴 JWT（支持 Cloud-IDE-JWT 前缀）"
              />
              {jwt.trim() && (jwtPreview || jwtPreviewError) && (
                <p className="text-xs text-muted-foreground">
                  {jwtPreview ? (
                    <>
                      userId：<span className="font-mono">{jwtPreview.userId}</span>
                      {jwtPreview.expHours != null && (
                        <>
                          {" "}· 剩余{" "}
                          <span
                            className={
                              jwtPreview.expHours <= 0
                                ? "text-red-600 dark:text-red-500"
                                : jwtPreview.expHours <= 24
                                  ? "text-amber-600 dark:text-amber-500"
                                  : "text-emerald-600 dark:text-emerald-500"
                            }
                          >
                            {jwtPreview.expHours.toFixed(1)}h
                          </span>
                        </>
                      )}
                    </>
                  ) : (
                    <span className="text-red-600 dark:text-red-500">{jwtPreviewError}</span>
                  )}
                </p>
              )}
            </div>
            <div className="space-y-2">
              <Label htmlFor="trae-jwt-rt">refresh_token（可选）</Label>
              <Input
                id="trae-jwt-rt"
                value={jwtRefreshToken}
                onChange={(e) => setJwtRefreshToken(e.target.value)}
                placeholder="填写后 JWT 过期前可自动刷新"
              />
            </div>
            <Button className="w-full" onClick={() => void handleAddJwt()} disabled={addingJwt}>
              {addingJwt ? <Loader2 className="animate-spin" /> : <Plus />}
              {addingJwt ? "添加中…" : "添加 JWT 账号"}
            </Button>
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}
