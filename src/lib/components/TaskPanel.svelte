<script lang="ts">
  import { downloads, formatBytes } from "$lib/stores/downloads.svelte";
  import { downloadCancel } from "$lib/api/github";

  const hidden = $derived(downloads.tasks.length === 0);
</script>

<footer class="panel" class:hidden aria-hidden={hidden}>
  {#each downloads.tasks as task (task.taskId)}
    <div class="task">
      <div class="row">
        <span class="name" title={task.name}>{task.name}</span>
        {#if task.state === "running"}
          <span class="meta">
            {formatBytes(task.downloaded)}
            {#if task.total > 0}/ {formatBytes(task.total)}{/if}
            · {formatBytes(task.speedBps)}/s
          </span>
          <button class="cancel" onclick={() => downloadCancel(task.taskId)}>取消</button>
        {:else if task.state === "done"}
          <span class="meta ok">完成</span>
        {:else if task.state === "cancelled"}
          <span class="meta">已取消</span>
        {:else}
          <span class="meta err" title={task.error ?? ""}>失败</span>
        {/if}
      </div>
      <div class="bar">
        <div
          class="fill"
          class:running={task.state === "running"}
          class:err={task.state === "error"}
          class:ok={task.state === "done"}
          style="transform: scaleX({task.total > 0
            ? Math.min(task.downloaded / task.total, 1)
            : task.state === 'done'
              ? 1
              : 0})"
        ></div>
      </div>
    </div>
  {/each}
</footer>

<style>
  /* 悬浮毛玻璃 pill：固定在 main 底部居中，无任务时整体淡出 */
  .panel {
    position: absolute;
    left: 50%;
    bottom: 14px;
    z-index: 10;
    width: max-content;
    max-width: 720px;
    max-height: 132px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 16px;
    border-radius: var(--radius-card);
    background: var(--window-strong);
    backdrop-filter: blur(14px);
    -webkit-backdrop-filter: blur(14px);
    border: 1px solid var(--line);
    box-shadow: var(--shadow-2);
    font-size: 12px;
    transform: translateX(-50%);
    opacity: 1;
    transition:
      opacity var(--dur) var(--ease-out),
      transform var(--dur) var(--ease-out);
  }
  .panel.hidden {
    opacity: 0;
    transform: translateX(-50%) translateY(8px);
    pointer-events: none;
  }
  .task {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 280px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .name {
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--text-muted);
    white-space: nowrap;
  }
  .meta.ok {
    color: var(--ok);
  }
  .meta.err {
    color: var(--danger);
  }
  .cancel {
    margin-left: auto;
    font-size: 11px;
    color: var(--text-muted);
    padding: 1px 8px;
    border-radius: var(--radius-full);
    border: 1px solid var(--line);
    transition:
      color var(--dur-fast),
      border-color var(--dur-fast);
  }
  .cancel:hover {
    color: var(--danger);
    border-color: var(--danger);
  }
  .bar {
    height: 4px;
    border-radius: var(--radius-full);
    background: var(--accent-soft);
    overflow: hidden;
  }
  /*
   * 用 transform 而不是 width：width 变化会触发布局，且 100ms 一次的进度更新
   * 配上过渡永远追不上。scaleX 只走合成层。
   */
  .fill {
    position: relative;
    overflow: hidden;
    height: 100%;
    width: 100%;
    transform-origin: 0 50%;
    transform: scaleX(0);
    will-change: transform;
    background: var(--accent);
  }
  /* 下载中的流光只在 running 时挂 */
  .fill.running::after {
    content: "";
    position: absolute;
    inset: 0;
    width: 24%;
    background: linear-gradient(90deg, transparent, rgba(255, 255, 255, 0.35), transparent);
    animation: sweep 1.2s linear infinite;
  }
  :root[data-theme="light"] .fill.running::after {
    background: linear-gradient(90deg, transparent, rgba(255, 255, 255, 0.25), transparent);
  }
  .fill.ok {
    background: var(--ok);
  }
  .fill.err {
    background: var(--danger);
  }
  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(420%);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .fill.running::after {
      animation: none;
    }
  }
</style>
