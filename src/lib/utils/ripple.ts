/*
 * AirLauncher
 * A Tauri-based launcher for the AirPlus Minecraft client.
 * https://github.com/lmx0721/AirPlus
 *
 * 点击涟漪：在目标按钮内插入一枚从指针位置扩散的圆点，动画结束自行移除。
 * 父容器需要 position:relative + overflow:hidden。
 */
export function spawnRipple(e: PointerEvent): void {
  const host = e.currentTarget as HTMLElement | null;
  if (!host) return;
  if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;

  const rect = host.getBoundingClientRect();
  const size = Math.max(rect.width, rect.height) * 2;
  const span = document.createElement("span");
  span.className = "ripple";
  span.style.width = `${size}px`;
  span.style.height = `${size}px`;
  span.style.left = `${e.clientX - rect.left - size / 2}px`;
  span.style.top = `${e.clientY - rect.top - size / 2}px`;
  host.appendChild(span);
  span.addEventListener("animationend", () => span.remove(), { once: true });
}
