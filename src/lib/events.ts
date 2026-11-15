/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 */
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface DownloadProgress {
  taskId: string;
  name: string;
  downloaded: number;
  total: number;
  speedBps: number;
  state: "running" | "done" | "error" | "cancelled";
  error: string | null;
}

export interface ForgeStage {
  taskId: string;
  stage: string;
  message: string;
}

export interface AuthStage {
  /** waiting | code | msa | xbl | minecraft | done | error */
  stage: string;
  message: string;
  /** 设备码流程：用户在浏览器要输入的短码 */
  userCode?: string;
  /** 设备码流程：授权页地址 */
  verificationUri?: string;
}

export interface LaunchStatus {
  running: boolean;
  pid: number | null;
}

/**
 * 后端每 100ms 把各任务的最新进度汇总成一个数组发一次（而非每个任务一条事件），
 * 并发数再高事件频率也恒定为 10/s。
 */
export function onDownloadProgress(cb: (batch: DownloadProgress[]) => void): Promise<UnlistenFn> {
  return listen<DownloadProgress[]>("download://progress", (e) => cb(e.payload));
}

export function onForgeStage(cb: (stage: string) => void): Promise<UnlistenFn> {
  return listen<string>("forge://stage", (e) => cb(e.payload));
}

export function onAuthStage(cb: (s: AuthStage) => void): Promise<UnlistenFn> {
  return listen<AuthStage>("auth://stage", (e) => cb(e.payload));
}

export function onLaunchLog(cb: (line: string) => void): Promise<UnlistenFn> {
  return listen<string>("launch://log", (e) => cb(e.payload));
}

/** launch://exit 载荷：error = 启动流程失败原因；exitCode = 游戏进程退出码 */
export interface LaunchExitPayload {
  error: string | null;
  exitCode: number | null;
}

/**
 * 正常结束 {error: null, exitCode: 0}；游戏崩溃时 exitCode 非 0，
 * 前端据此引导用户去看日志。
 */
export function onLaunchExit(cb: (payload: LaunchExitPayload) => void): Promise<UnlistenFn> {
  return listen<LaunchExitPayload>("launch://exit", (e) => cb(e.payload));
}
