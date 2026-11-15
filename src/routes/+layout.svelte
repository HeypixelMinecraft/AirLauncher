<script lang="ts">
  import "../app.css";
  import { onMount } from "svelte";
  import { view, type ViewId } from "$lib/stores/view.svelte";
  import { loadSettings } from "$lib/stores/settings.svelte";
  import { initTheme } from "$lib/stores/theme.svelte";

  import TitleBar from "$lib/components/TitleBar.svelte";
  import TopNav from "$lib/components/TopNav.svelte";
  import TaskPanel from "$lib/components/TaskPanel.svelte";
  import HomePage from "$lib/pages/HomePage.svelte";
  import VersionsPage from "$lib/pages/VersionsPage.svelte";
  import ModsPage from "$lib/pages/ModsPage.svelte";
  import AccountsPage from "$lib/pages/AccountsPage.svelte";
  import SettingsPage from "$lib/pages/SettingsPage.svelte";
  import type { Component } from "svelte";
  import { initDownloadEvents } from "$lib/stores/downloads.svelte";

  /*
   * 页面常驻，不再用 {#key view.current} 整页销毁重建。
   * 之前每次切页都要卸载再挂载 HomePage，等于重新 listen 两个事件 + 清空日志状态，
   * 既丢数据又制造 IPC 抖动；现在只切换可见性，各页面自己处理「重新进入时刷新」。
   */
  const pageList: { id: ViewId; component: Component<{ active: boolean }> }[] = [
    { id: "home", component: HomePage },
    { id: "versions", component: VersionsPage },
    { id: "mods", component: ModsPage },
    { id: "accounts", component: AccountsPage },
    { id: "settings", component: SettingsPage },
  ];

  let mainEl = $state<HTMLElement | null>(null);

  /*
   * 切页 stagger：pane 常驻，无法靠 {#if} 重新挂载触发动画，
   * 于是在 view.current 变化时对该 pane 的 .page 直接子元素重放动画类。
   */
  $effect(() => {
    const id = view.current;
    const pane = mainEl?.querySelector<HTMLElement>(`.pane[data-id="${id}"]`);
    if (!pane) return;
    pane.classList.remove("stagger");
    // 强制 reflow，让移除-添加跨帧生效
    void pane.offsetWidth;
    pane.classList.add("stagger");
  });

  onMount(() => {
    initTheme();
    void loadSettings();
    initDownloadEvents();
  });
</script>

<div class="shell">
  <TitleBar />
  <TopNav />
  <main class="main" bind:this={mainEl}>
    {#each pageList as p (p.id)}
      {@const Page = p.component}
      <div class="pane" class:active={view.current === p.id} data-id={p.id}>
        <Page active={view.current === p.id} />
      </div>
    {/each}
    <TaskPanel />
  </main>
</div>

<style>
  /*
   * 亚克力可见层：半透明压暗 + 两层极淡薄荷氛围光。
   * 这里不能有不透明的 linear-gradient 底色，否则 DWM 模糊会被完全遮住。
   */
  .shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background-color: var(--overlay);
    background-image:
      radial-gradient(900px 480px at 10% -6%, var(--glow-a), transparent 60%),
      radial-gradient(800px 460px at 106% 30%, var(--glow-b), transparent 55%);
  }
  .main {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  /*
   * 用 visibility 而不是 hidden/display:none：display:none 会把滚动容器的
   * scrollTop 重置为 0，切回去就跳回顶部。visibility:hidden 保留布局与滚动位置。
   * 底部留白给悬浮任务 pill。
   */
  .pane {
    position: absolute;
    inset: 0;
    overflow-y: auto;
    padding: 22px 26px 76px;
    visibility: hidden;
    pointer-events: none;
  }
  .pane.active {
    visibility: visible;
    pointer-events: auto;
  }

  /* 页面切换入场：各页 .page 的直接子岛依次浮现（stagger 由 JS 动态挂/摘） */
  :global(.pane.stagger .page > *) {
    animation: island-in var(--dur) var(--ease-out) both;
  }
  :global(.pane.stagger .page > *:nth-child(2)) {
    animation-delay: 40ms;
  }
  :global(.pane.stagger .page > *:nth-child(3)) {
    animation-delay: 80ms;
  }
  :global(.pane.stagger .page > *:nth-child(4)) {
    animation-delay: 120ms;
  }
  :global(.pane.stagger .page > *:nth-child(5)) {
    animation-delay: 160ms;
  }
  :global(.pane.stagger .page > *:nth-child(6)) {
    animation-delay: 200ms;
  }
  :global(.pane.stagger .page > *:nth-child(n + 7)) {
    animation-delay: 240ms;
  }
  @keyframes island-in {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    :global(.pane.stagger .page > *) {
      animation: none;
    }
  }
</style>
