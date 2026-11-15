<script lang="ts">
  import {
    listReleases,
    downloadAsset,
    downloadCancel,
    type GhRelease,
    type GhAsset,
  } from "$lib/api/github";
  import { downloads, formatBytes } from "$lib/stores/downloads.svelte";
  import { marked } from "marked";
  import DOMPurify from "dompurify";

  let releases = $state<GhRelease[]>([]);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let selectedTag = $state<string | null>(null);

  /**
   * 点击「下载」后本地先标记为下载中：
   * 后端要等测速/首包才有进度事件，直接依赖事件会让按钮在几百毫秒内还是「下载」，
   * 用户以为没点到就会连点好几次，触发多个重复任务。
   */
  let starting = $state<Record<string, boolean>>({});

  let { active = false } = $props();
  let lastLoaded = 0;
  /** GitHub 未认证接口只有 60 次/小时，切页回来不要重复拉 */
  const CACHE_MS = 5 * 60 * 1000;

  const jarAssetOf = (r: GhRelease): GhAsset | undefined =>
    r.assets.find((a) => a.name.endsWith(".jar") && !a.name.includes("sources"));

  const taskOf = (assetName: string) =>
    downloads.tasks.find((t) => t.name === assetName);

  const renderedBody = $derived.by(() => {
    const r = releases.find((x) => x.tag_name === selectedTag);
    if (!r?.body) return "";
    return DOMPurify.sanitize(marked.parse(r.body, { async: false }) as string);
  });

  const formatDate = (iso: string | null) =>
    iso ? new Date(iso).toLocaleDateString("zh-CN") : "";

  async function load() {
    loading = true;
    loadError = null;
    try {
      releases = await listReleases();
      lastLoaded = Date.now();
      if (releases.length > 0) selectedTag = releases[0].tag_name;
    } catch (e) {
      loadError = String(e);
    } finally {
      loading = false;
    }
  }

  async function install(asset: GhAsset, tag: string) {
    if (starting[asset.name]) return;
    starting[asset.name] = true;
    try {
      await downloadAsset(asset, tag);
    } catch {
      // 启动任务失败：立刻放开按钮，具体错误由下方任务条/日志承担
      starting[asset.name] = false;
      return;
    }
    // 任务已在后端排队，等进度事件接管即可；这里兜底清理，避免标记残留
    window.setTimeout(() => {
      starting[asset.name] = false;
    }, 3000);
  }

  $effect(() => {
    if (!active) return;
    if (releases.length > 0 && Date.now() - lastLoaded < CACHE_MS) return;
    void load();
  });
</script>

<div class="page">
  <header class="page-head">
    <h2>版本</h2>
    <button class="btn" onclick={load} disabled={loading}>刷新</button>
  </header>
  {#if loading}
    <p class="muted">正在加载版本列表…</p>
  {:else if loadError}
    <div class="error card">
      <p>加载版本列表失败：{loadError}</p>
      <button class="btn" onclick={load}>重试</button>
    </div>
  {:else}
    <div class="split">
      <aside class="list">
        {#each releases as r (r.tag_name)}
          <button
            class="rel island clickable"
            class:active={selectedTag === r.tag_name}
            onclick={() => (selectedTag = r.tag_name)}
          >
            <span class="tag">
              {r.tag_name}
              {#if r.prerelease}<em class="pre">预发布</em>{/if}
            </span>
            <span class="date">{formatDate(r.published_at)}</span>
          </button>
        {/each}
      </aside>

      <section class="detail island">
        {#each releases.filter((r) => r.tag_name === selectedTag) as r (r.tag_name)}
          <h2>{r.name ?? r.tag_name}</h2>

          <div class="assets">
            {#each r.assets as asset (asset.id)}
              {@const task = taskOf(asset.name)}
              <div class="asset">
                <span class="aname" title={asset.name}>{asset.name}</span>
                <span class="asize">{formatBytes(asset.size)}</span>
                {#if task?.state === "running"}
                  <span class="dlmeta">
                    <span class="spin"></span>
                    {#if task.total > 0}
                      {formatBytes(task.downloaded)} / {formatBytes(task.total)}
                      {#if task.speedBps > 0}· {formatBytes(task.speedBps)}/s{/if}
                    {:else}
                      准备中…
                    {/if}
                  </span>
                  <button class="btn ghost" onclick={() => downloadCancel(task.taskId)}>取消</button>
                {:else if task?.state === "error"}
                  <span class="dlmeta err" title={task.error ?? ""}>
                    下载失败{task.error ? `：${task.error}` : ""}
                  </span>
                  <button class="btn" onclick={() => install(asset, r.tag_name)}>重试</button>
                {:else if starting[asset.name]}
                  <span class="dlmeta"><span class="spin"></span>准备中…</span>
                {:else}
                  <button class="btn" onclick={() => install(asset, r.tag_name)}>下载</button>
                {/if}
              </div>
            {/each}
          </div>

          <div class="changelog">
            <h3>更新日志</h3>
            {@html renderedBody}
          </div>
        {/each}
      </section>
    </div>
  {/if}
</div>

<style>
  .page {
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .page-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .page-head h2 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
  }
  .muted {
    color: var(--text-muted);
  }
  .error {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    align-items: flex-start;
  }
  .split {
    flex: 1;
    display: flex;
    gap: 14px;
    min-height: 0;
  }
  .list {
    width: 200px;
    flex: none;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 2px 2px 2px 0;
  }
  .rel {
    display: flex;
    flex-direction: column;
    gap: 2px;
    text-align: left;
    padding: 10px 12px;
    color: var(--text);
  }
  .rel.active {
    border-color: var(--accent-strong);
    background: var(--accent-soft);
  }
  .tag {
    font-size: 13px;
    font-weight: 600;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .pre {
    font-style: normal;
    font-size: 10px;
    color: var(--accent);
    border: 1px solid var(--accent-strong);
    border-radius: var(--radius-full);
    padding: 0 6px;
  }
  .date {
    font-size: 11px;
    color: var(--text-muted);
  }
  .detail {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 20px 22px;
  }
  .detail h2 {
    margin: 0 0 14px;
    font-size: 18px;
  }
  h3 {
    margin: 18px 0 8px;
    font-size: 14px;
    color: var(--text-muted);
  }
  .assets {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .asset {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    border-radius: var(--radius);
    background: var(--window);
    border: 1px solid var(--line);
  }
  .aname {
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .asize {
    font-size: 11px;
    color: var(--text-muted);
    flex: none;
  }
  .dlmeta {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--accent);
  }
  .dlmeta.err {
    color: var(--danger);
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* 下载中的小转圈，点击后立刻可见 */
  .spin {
    width: 10px;
    height: 10px;
    flex: none;
    border-radius: 50%;
    border: 1.5px solid var(--accent-soft);
    border-top-color: var(--accent);
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation: none;
    }
  }
  .asset .btn {
    margin-left: auto;
    padding: 4px 14px;
  }
  .asset .btn.ghost {
    margin-left: 0;
    color: var(--text-muted);
  }
  .changelog {
    user-select: text;
    -webkit-user-select: text;
    font-size: 13px;
    line-height: 1.6;
  }
  .changelog :global(code) {
    background: var(--window);
    padding: 1px 5px;
    border-radius: var(--radius-sm);
    font-size: 12px;
  }
  .changelog :global(pre) {
    background: var(--window);
    padding: 10px 12px;
    border-radius: var(--radius);
    overflow-x: auto;
  }
  .changelog :global(a) {
    color: var(--accent);
  }
  .changelog :global(img) {
    max-width: 100%;
  }
</style>
