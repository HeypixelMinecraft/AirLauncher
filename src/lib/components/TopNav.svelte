<script lang="ts">
  import { onMount, tick } from "svelte";
  import { view, type ViewId } from "$lib/stores/view.svelte";
  import { spawnRipple } from "$lib/utils/ripple";

  const items: { id: ViewId; label: string; path: string }[] = [
    {
      id: "home",
      label: "主页",
      path: "M3 10.5 12 3l9 7.5M5.5 9.5V20h13V9.5",
    },
    {
      id: "versions",
      label: "版本",
      path: "M12 3v10m0 0 4-4m-4 4-4-4M4 17v3h16v-3",
    },
    {
      id: "mods",
      label: "模组",
      path: "M12 2.5 21 7v10l-9 4.5L3 17V7l9-4.5Zm0 0V12m9-5-9 5m-9-5 9 5",
    },
    {
      id: "accounts",
      label: "账号",
      path: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8Zm-8 9a8 8 0 0 1 16 0",
    },
    {
      id: "settings",
      label: "设置",
      path: "M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Zm8-3.5c0 .6-.06 1.17-.18 1.72l2.02 1.58-2 3.46-2.39-.96A8.2 8.2 0 0 1 14.4 19.2l-.36 2.55h-4l-.36-2.55a8.2 8.2 0 0 1-3.05-1.4l-2.4.96-2-3.46 2.03-1.58A8.6 8.6 0 0 1 4.06 12c0-.6.06-1.17.18-1.72L2.22 8.7l2-3.46 2.39.96A8.2 8.2 0 0 1 9.68 4.8l.36-2.55h4l.36 2.55a8.2 8.2 0 0 1 3.05 1.4l2.4-.96 2 3.46-2.03 1.58c.12.55.18 1.12.18 1.72Z",
    },
  ];

  let navEl = $state<HTMLElement | null>(null);
  let indX = $state(0);
  let indW = $state(0);

  /** 量当前 tab 的 offsetLeft/offsetWidth，驱动下滑线 */
  function measure() {
    const el = navEl?.querySelector<HTMLElement>(`[data-tab="${view.current}"]`);
    if (!el) return;
    indX = el.offsetLeft;
    indW = el.offsetWidth;
  }

  $effect(() => {
    // 依赖 view.current：切页后等 DOM 更新完再量
    view.current;
    void tick().then(measure);
  });

  onMount(() => {
    // 字体加载完成后 tab 宽度会变，rAF 再补量一次
    measure();
    const raf = requestAnimationFrame(measure);
    window.addEventListener("resize", measure);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("resize", measure);
    };
  });
</script>

<nav class="topnav" bind:this={navEl}>
  <div class="drag-fill" data-tauri-drag-region></div>

  {#each items as item (item.id)}
    <button
      class="tab"
      class:active={view.current === item.id}
      data-tab={item.id}
      onclick={() => (view.current = item.id)}
      onpointerdown={spawnRipple}
    >
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path
          d={item.path}
          fill="none"
          stroke="currentColor"
          stroke-width="1.6"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      </svg>
      <span>{item.label}</span>
    </button>
  {/each}

  <span class="indicator" style="width: {indW}px; transform: translateX({indX}px)"></span>
</nav>

<style>
  .topnav {
    position: relative;
    flex: none;
    display: flex;
    align-items: center;
    gap: 2px;
    height: 52px;
    padding: 0 18px;
    background: transparent;
  }
  /* 铺满整行只负责窗口拖动命中，tab 的 z-index 更高 */
  .drag-fill {
    position: absolute;
    inset: 0;
    z-index: 0;
  }
  .tab {
    position: relative;
    overflow: hidden;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 4px 16px;
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    font-size: 13.5px;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .tab svg {
    width: 16px;
    height: 16px;
    flex: none;
  }
  .tab:hover {
    color: var(--text);
  }
  .tab.active {
    color: var(--accent);
  }
  .indicator {
    position: absolute;
    bottom: 6px;
    left: 0;
    height: 3px;
    border-radius: var(--radius-full);
    background: var(--accent);
    pointer-events: none;
    z-index: 2;
    transition:
      transform var(--dur) var(--ease-spring),
      width var(--dur) var(--ease-spring);
  }
  @media (prefers-reduced-motion: reduce) {
    .indicator {
      transition: none;
    }
  }
</style>
