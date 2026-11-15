/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 *
 * 主题状态与持久化（localStorage，不动 Rust Settings schema）。
 * 首帧的主题由 app.html 的内联脚本提前写入 data-theme，避免闪错。
 */
export type ThemeId = "dark" | "light";

const STORAGE_KEY = "airlauncher.theme";

export const theme = $state<{ current: ThemeId }>({ current: "dark" });

/** 同步 app.html 内联脚本已设置的主题到运行时状态 */
export function initTheme(): void {
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(STORAGE_KEY);
  } catch {
    /* 存储不可用时用默认深色 */
  }
  theme.current = saved === "light" ? "light" : "dark";
  document.documentElement.dataset.theme = theme.current;
}

export function setTheme(t: ThemeId): void {
  if (theme.current === t) return;
  theme.current = t;
  try {
    localStorage.setItem(STORAGE_KEY, t);
  } catch {
    /* 记忆失败不影响本次切换 */
  }
  const root = document.documentElement;
  root.classList.add("theme-anim");
  root.dataset.theme = t;
  window.setTimeout(() => root.classList.remove("theme-anim"), 550);
}
