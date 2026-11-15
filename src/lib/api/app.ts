/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 */
import { invoke } from "@tauri-apps/api/core";

export interface AppPathsInfo {
  root: string;
  /** 默认数据目录（「恢复默认」用） */
  defaultRoot: string;
  instancesDir: string;
  instanceDir: string;
  modsDir: string;
  jreDir: string;
  /** 游戏日志目录（latest.txt 与轮转归档） */
  logsDir: string;
  cacheDir: string;
  settingsFile: string;
  accountsFile: string;
  /** 下载清单（客户端版本 + 模组 + sha1） */
  manifestFile: string;
}

export interface Settings {
  /** 已废弃：窗口为 opaque + CSS 渐变，保留字段仅为后端 serde 兼容 */
  windowEffect: "acrylic" | "mica" | "none";
  ramMb: number;
  extraJvmArgs: string[];
  downloadConcurrency: number;
  githubPat: string | null;
  mirror: string | null;
  activeAccountId: string | null;
  /** 手动指定的 Java 路径（最高优先级，设置页「浏览…」选择） */
  javaPath: string | null;
  /** 上次解析出的 Java 来源："manual" | "bundled" | "system" | "download" */
  javaSource: string | null;
  /** 上次找到的系统 Java 路径（javaSource = "system" 时有效） */
  systemJavaPath: string | null;
}

export function pathsGet(): Promise<AppPathsInfo> {
  return invoke("paths_get");
}

/**
 * 用系统默认程序打开数据目录内的路径。
 * 走后端 opener（Rust 侧不受 ACL 限制），前端直接调插件会报 Not allowed to open path。
 */
export function openDataPath(path: string): Promise<void> {
  return invoke("open_path", { path });
}

/** 用系统浏览器打开外部链接（bilibili 等） */
export function openExternal(url: string): Promise<void> {
  return invoke("open_url", { url });
}

/** 切换数据目录：后端会搬走现有数据，返回新目录（重启后生效） */
export function dataDirSet(path: string): Promise<string> {
  return invoke("data_dir_set", { path });
}

/** 恢复默认数据目录（不搬数据），返回默认目录（重启后生效） */
export function dataDirReset(): Promise<string> {
  return invoke("data_dir_reset");
}

export function appRestart(): Promise<void> {
  return invoke("app_restart");
}

export function settingsGet(): Promise<Settings> {
  return invoke("settings_get");
}

export function settingsSet(settings: Settings): Promise<void> {
  return invoke("settings_set", { settings });
}

/** 一个可用的 Java：路径 + `java -version` 解析出的版本号与发行商 */
export interface JavaInfo {
  path: string;
  /** 完整版本号，如 "1.8.0_402" */
  version: string;
  /** 发行商简称，如 "Temurin" / "Zulu" */
  vendor: string | null;
}

/** 手动检测结果：Java 8 与其它版本（11/17/21…）分开报告 */
export interface DetectResult {
  java8: JavaInfo | null;
  others: JavaInfo[];
}

/** 当前生效的 Java（主页/设置页展示） */
export interface JavaStatus {
  /** "manual" | "bundled" | "system" | "download" | "none" */
  source: string;
  path: string | null;
  version: string | null;
  vendor: string | null;
}

/** 手动检测系统里的所有 Java（Java 8 与其它版本分开报告，刷新缓存） */
export function javaDetect(): Promise<DetectResult> {
  return invoke("java_detect");
}

/** 当前生效的 Java（不触发下载） */
export function javaStatus(): Promise<JavaStatus> {
  return invoke("java_status");
}

/** 物理内存总量 (MB)；拿不到返回 null */
export function systemTotalMemoryMb(): Promise<number | null> {
  return invoke("system_total_memory_mb");
}
