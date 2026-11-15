<script lang="ts">
  import { onMount } from "svelte";
  import { pathsGet, openDataPath, javaStatus, type JavaStatus, type AppPathsInfo } from "$lib/api/app";
  import { launchStart, launchCancel, launchStatus } from "$lib/api/launch";
  import { accountsList, type AccountInfo } from "$lib/api/auth";
  import { clientStatus, type ClientStatus } from "$lib/api/client";
  import { onLaunchLog, onLaunchExit, type LaunchExitPayload } from "$lib/events";
  import { settings, loadSettings } from "$lib/stores/settings.svelte";
  import { view } from "$lib/stores/view.svelte";
  import { spawnRipple } from "$lib/utils/ripple";
  import ClientPicker from "$lib/components/ClientPicker.svelte";

  let { active = false } = $props();

  let paths = $state<AppPathsInfo | null>(null);
  let playerName = $state("Player");
  /** 当前生效的 Java（启动按钮下方小字展示，null = 还没查到） */
  let java = $state<JavaStatus | null>(null);
  let msaAccount = $state<AccountInfo | null>(null);
  /** idle: 可启动 / preparing: 下载安装中 / running: 游戏运行中 */
  let phase = $state<"idle" | "preparing" | "running">("idle");
  /** mods 里的 AirPlus 客户端（null = 还没查过） */
  let client = $state<ClientStatus | null>(null);
  /** 版本选择弹窗 */
  let pickerOpen = $state(false);
  /** 弹窗是「启动触发的缺客户端」还是「主动切换版本」：前者下载完要接着启动 */
  let startAfterPick = $state(false);
  /** 日志岛默认折叠，启动时自动展开 */
  let logOpen = $state(false);
  /** 日志用自增 id 做稳定 key，避免按索引 key 时整表重建 */
  let logs = $state<{ id: number; text: string }[]>([]);
  let logSeq = 0;
  const MAX_LOG_LINES = 150;
  /** 单行长度上限：游戏/模组偶尔会喷出几十万字符的长行，配上 word-break 会卡死布局 */
  const MAX_LINE_LEN = 400;

  const launchLabel = $derived(
    phase === "idle" ? "启动游戏" : phase === "preparing" ? "取消准备" : "结束游戏",
  );

  const SOURCE_LABELS: Record<string, string> = {
    manual: "手动指定",
    bundled: "内置",
    system: "系统",
    download: "自动下载",
  };

  /** 启动按钮旁的小字：「Java：内置 Temurin 8 / 1.8.0_402」 */
  const javaLabel = $derived.by(() => {
    if (java === null) return "Java：检测中…";
    if (!java.path) return "Java：未就绪（启动时自动准备）";
    const who = SOURCE_LABELS[java.source] ?? java.source;
    const ver = java.vendor ? `${java.vendor} ${java.version ?? ""}` : (java.version ?? "");
    return `Java：${who}${ver ? ` · ${ver.trim()}` : ""}`;
  });

  /** 打开游戏日志目录（latest.txt 与轮转归档所在文件夹） */
  function openLogsDir() {
    if (!paths) return;
    openDataPath(paths.logsDir).catch((e) => pushLogs(`打开日志目录失败: ${e}`));
  }

  function pushLogs(text: string) {
    // 后端日志按 100ms 合批发送，一次可能含多行
    for (const raw of text.split("\n")) {
      const line = raw.length > MAX_LINE_LEN ? `${raw.slice(0, MAX_LINE_LEN)}…` : raw;
      logs.push({ id: logSeq++, text: line });
    }
    if (logs.length > MAX_LOG_LINES) {
      logs.splice(0, logs.length - MAX_LOG_LINES);
    }
  }

  onMount(() => {
    // 监听是长期注册，只做一次；页面现在常驻，不会因为切页重复注册
    const unlisteners: Promise<() => void>[] = [
      onLaunchLog((line) => {
        pushLogs(line);
        if (line.includes("游戏已启动")) phase = "running";
      }),
      onLaunchExit((payload: LaunchExitPayload) => {
        phase = "idle";
        // 游戏崩溃（非 0 退出码）时引导用户去看日志文件
        if (payload.exitCode !== null && payload.exitCode !== 0) {
          pushLogs(`游戏进程异常退出（exit code ${payload.exitCode}），点击上方「打开日志」查看 latest.txt`);
        }
      }),
    ];
    return () => unlisteners.forEach((p) => p.then((fn) => fn()));
  });

  /** 每次切回首页时同步一次（其他页可能改了当前账号） */
  $effect(() => {
    if (!active) return;
    void syncStatus();
  });

  async function syncStatus() {
    pathsGet().then((p) => (paths = p));
    javaStatus()
      .then((j) => (java = j))
      .catch(() => (java = { source: "none", path: null, version: null, vendor: null }));
    clientStatus()
      .then((c) => (client = c))
      .catch(() => {
        // 查不到就当未安装：点启动时会再查一次，真失败也有后端自动安装兜底
        client = { installed: false, fileName: null, tag: null };
      });
    try {
      const s = await launchStatus();
      phase = s.running ? "running" : "idle";
      // 必须等设置真正加载完，否则 activeAccountId 还是默认的 null
      await loadSettings();
      const id = settings.activeAccountId;
      if (!id) {
        msaAccount = null;
        return;
      }
      const list = await accountsList();
      msaAccount = list.find((a) => a.id === id) ?? null;
    } catch {
      /* 状态同步失败不影响界面可用性 */
    }
  }

  async function onLaunchButton() {
    if (phase === "preparing" || phase === "running") {
      launchCancel();
      return;
    }

    // 启动前必须确认 mods 里有客户端：没有的话进游戏就是原版 Forge 界面。
    // 查不到（后端异常）就交给 launch 内部的自动安装兜底，不阻断用户。
    try {
      const st = await clientStatus();
      client = st;
      if (!st.installed) {
        startAfterPick = true;
        pickerOpen = true;
        return;
      }
    } catch {
      /* 忽略，交给后端兜底 */
    }

    await doLaunch();
  }

  async function doLaunch() {
    logs = [];
    logOpen = true;
    phase = "preparing";
    try {
      if (msaAccount) {
        await launchStart({ playerName: "", accountId: msaAccount.id });
      } else {
        const name = playerName.trim();
        if (!name) {
          phase = "idle";
          return;
        }
        await launchStart({ playerName: name, accountId: null });
      }
    } catch (e) {
      pushLogs(`启动失败: ${e}`);
      phase = "idle";
    }
  }

  /** 打开版本选择弹窗：startAfterPick=false 表示只是切换版本，下完不自动启动 */
  function openVersionPicker() {
    startAfterPick = false;
    pickerOpen = true;
  }

  /** 弹窗里下好并激活了客户端 */
  async function onClientReady() {
    clientStatus()
      .then((c) => (client = c))
      .catch(() => {});
    if (startAfterPick) {
      startAfterPick = false;
      await doLaunch();
    }
  }

  /* ---- hero 视差微倾（克制档：±1.5deg + 每帧 lerp） ---- */
  let heroEl = $state<HTMLElement | null>(null);
  let curX = 0;
  let curY = 0;
  let targetX = 0;
  let targetY = 0;
  let rafId = 0;

  function onHeroMove(e: PointerEvent) {
    if (!heroEl || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const r = heroEl.getBoundingClientRect();
    targetX = -((e.clientY - r.top) / r.height - 0.5) * 3;
    targetY = ((e.clientX - r.left) / r.width - 0.5) * 3;
    if (rafId === 0) rafId = requestAnimationFrame(parallaxTick);
  }

  function onHeroLeave() {
    targetX = 0;
    targetY = 0;
    if (rafId === 0 && heroEl) rafId = requestAnimationFrame(parallaxTick);
  }

  function parallaxTick() {
    rafId = 0;
    curX += (targetX - curX) * 0.12;
    curY += (targetY - curY) * 0.12;
    if (heroEl) {
      heroEl.style.transform = `perspective(900px) rotateX(${curX.toFixed(2)}deg) rotateY(${curY.toFixed(2)}deg)`;
    }
    if (Math.abs(targetX - curX) > 0.01 || Math.abs(targetY - curY) > 0.01) {
      rafId = requestAnimationFrame(parallaxTick);
    }
  }
</script>

<div class="page">
  <section
    class="hero card"
    role="presentation"
    bind:this={heroEl}
    onpointermove={onHeroMove}
    onpointerleave={onHeroLeave}
  >
    <div class="hero-left">
      <div class="titles">
        <h1>AirPlus</h1>
        <p class="sub">基于 LiquidBounce Legacy 的 Minecraft 1.8.9 Forge 客户端</p>
      </div>
      <div class="launch">
        {#if phase === "idle"}
          <input
            class="name"
            type="text"
            placeholder="玩家名"
            bind:value={playerName}
            maxlength="16"
          />
        {/if}
        <button
          class="launch-btn"
          class:danger={phase === "running"}
          class:breathing={phase === "idle"}
          onclick={onLaunchButton}
          onpointerdown={spawnRipple}
        >
          {launchLabel}
        </button>
        {#if phase === "preparing"}
          <span class="indeterminate"></span>
        {/if}
        <span class="java-hint">{javaLabel}</span>
      </div>
    </div>

    <div class="hero-right">
      <button
        class="pill"
        onclick={openVersionPicker}
        title="切换客户端版本（会替换掉 mods 里现有的客户端）"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path
            d="M12 3v10m0 0 4-4m-4 4-4-4M4 17v3h16v-3"
            fill="none"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
        <span class="pval">
          客户端版本 ·
          {client === null
            ? "检测中…"
            : client.installed
              ? `AirPlus ${client.tag ?? "已安装"}`
              : "未安装"}
        </span>
        <span class="chip">{client?.installed ? "切换版本" : "选择版本"}</span>
      </button>

      <button class="pill" onclick={() => (view.current = "accounts")} title="管理登录账号">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path
            d="M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8Zm-8 9a8 8 0 0 1 16 0"
            fill="none"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
        <span class="pval">
          账号 · {msaAccount ? `${msaAccount.name}（微软）` : "离线模式"}
        </span>
      </button>
    </div>
  </section>

  <section class="card logbox">
    <div class="log-head">
      <button
        class="log-toggle"
        onclick={() => (logOpen = !logOpen)}
        aria-expanded={logOpen}
      >
        <span class="title">启动日志</span>
        <span class="chev" class:open={logOpen} aria-hidden="true">
          <svg viewBox="0 0 12 12">
            <path d="M2.5 4.5 6 8l3.5-3.5" fill="none" stroke="currentColor" stroke-width="1.4" />
          </svg>
        </span>
      </button>
      <button class="log-btn" onclick={openLogsDir} disabled={!paths} title="打开游戏日志目录（latest.txt 与历史归档）">
        打开日志
      </button>
    </div>
    <div class="log-body" class:open={logOpen}>
      <div class="logs">
        {#each logs as line (line.id)}
          <div class="line">{line.text}</div>
        {/each}
        {#if logs.length === 0}
          <div class="empty">点击「启动游戏」开始，首次运行会先让你选一个客户端版本</div>
        {/if}
      </div>
      {#if paths}
        <div class="dir">
          <span class="dir-label">数据目录</span>
          <code>{paths.root}</code>
        </div>
      {/if}
    </div>
  </section>
</div>

<ClientPicker
  open={pickerOpen}
  startAfterPick={startAfterPick}
  onclose={() => {
    pickerOpen = false;
    startAfterPick = false;
  }}
  onready={onClientReady}
/>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  /* ---- hero 岛 ---- */
  .hero {
    display: flex;
    align-items: center;
    gap: 24px;
    min-height: 200px;
    padding: 26px 28px;
    will-change: transform;
  }
  .hero-left {
    flex: 1.2;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 22px;
  }
  .titles h1 {
    margin: 0 0 6px;
    font-size: 26px;
    letter-spacing: 0.5px;
  }
  .sub {
    margin: 0;
    color: var(--text-muted);
    font-size: 13px;
  }
  .launch {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .java-hint {
    flex-basis: 100%;
    font-size: 11.5px;
    color: var(--text-muted);
  }
  .name {
    background: var(--window);
    border: 1px solid var(--line);
    border-radius: var(--radius-full);
    color: var(--text);
    padding: 10px 16px;
    font-size: 13px;
    width: 148px;
    outline: none;
    transition:
      border-color var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out);
  }
  .name:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .launch-btn {
    position: relative;
    overflow: hidden;
    height: 46px;
    padding: 0 32px;
    border-radius: var(--radius-full);
    background: var(--accent);
    color: var(--accent-contrast);
    font-size: 15px;
    font-weight: 600;
    letter-spacing: 0.3px;
    transition:
      transform var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out),
      background var(--dur-fast) var(--ease-out);
  }
  .launch-btn:hover {
    transform: scale(1.03);
    box-shadow: var(--shadow-accent);
  }
  .launch-btn:active {
    transform: scale(0.97);
  }
  .launch-btn.danger {
    background: var(--danger);
    color: #fff;
  }
  .launch-btn.breathing {
    animation: breathe 4s ease-in-out infinite;
  }
  @keyframes breathe {
    0%,
    100% {
      box-shadow: 0 0 0 0 transparent;
    }
    50% {
      box-shadow: var(--shadow-accent);
    }
  }
  /* preparing 时贴按钮下沿的 indeterminate 细条 */
  .indeterminate {
    position: absolute;
    left: 32px;
    right: 32px;
    bottom: -8px;
    height: 2px;
    border-radius: var(--radius-full);
    overflow: hidden;
    background: var(--accent-soft);
  }
  .indeterminate::after {
    content: "";
    position: absolute;
    inset: 0;
    width: 40%;
    border-radius: var(--radius-full);
    background: var(--accent);
    animation: indeterminate 1.1s var(--ease-out) infinite;
  }
  @keyframes indeterminate {
    from {
      transform: translateX(-110%);
    }
    to {
      transform: translateX(310%);
    }
  }

  .hero-right {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
  }
  .pill {
    position: relative;
    overflow: hidden;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-radius: var(--radius-full);
    background: var(--accent-soft);
    color: var(--text);
    font-size: 13px;
    transition:
      background var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out);
  }
  .pill:hover {
    background: var(--accent-strong);
    transform: translateY(-1px);
  }
  .pill svg {
    width: 15px;
    height: 15px;
    flex: none;
    color: var(--accent);
  }
  .pval {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* 版本 pill 右侧的「切换版本」提示：点整个 pill 都会打开弹窗 */
  .chip {
    flex: none;
    margin-left: 2px;
    padding: 2px 9px;
    border-radius: var(--radius-full);
    background: var(--accent);
    color: var(--accent-contrast);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.2px;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .pill:hover .chip {
    background: var(--accent-strong);
    color: var(--text);
  }

  /* ---- 日志折叠岛 ---- */
  .logbox {
    display: flex;
    flex-direction: column;
    padding: 0 18px;
  }
  .log-head {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 44px;
    flex: none;
  }
  .log-toggle {
    position: relative;
    display: flex;
    align-items: center;
    flex: 1;
    height: 100%;
    justify-content: space-between;
  }
  .title {
    font-size: 12px;
    color: var(--text-muted);
    letter-spacing: 0.3px;
  }
  .log-btn {
    position: relative;
    z-index: 1;
    padding: 3px 10px;
    border-radius: var(--radius-full);
    border: 1px solid var(--line);
    background: var(--window);
    color: var(--text-muted);
    font-size: 11px;
    transition:
      color var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .log-btn:hover {
    color: var(--accent);
    border-color: var(--accent);
  }
  .chev {
    display: flex;
    color: var(--text-muted);
    transition: transform var(--dur) var(--ease-out), color var(--dur-fast);
  }
  .chev.open {
    transform: rotate(180deg);
    color: var(--accent);
  }
  .chev svg {
    width: 12px;
    height: 12px;
  }
  .log-body {
    max-height: 0;
    opacity: 0;
    overflow: hidden;
    transition:
      max-height var(--dur-slow) var(--ease-out),
      opacity var(--dur) var(--ease-out);
  }
  .log-body.open {
    max-height: 300px;
    opacity: 1;
  }
  .logs {
    font-family: "Cascadia Mono", Consolas, monospace;
    font-size: 12px;
    color: var(--text-muted);
    max-height: 190px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-bottom: 10px;
    user-select: text;
    -webkit-user-select: text;
  }
  .logs .line {
    white-space: pre-wrap;
    word-break: break-all;
    animation: line-in 150ms var(--ease-out);
  }
  @keyframes line-in {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  .empty {
    opacity: 0.6;
  }
  .dir {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 8px 0 14px;
    border-top: 1px solid var(--line);
    font-size: 12px;
  }
  .dir-label {
    flex: none;
    color: var(--text-muted);
  }
  code {
    font-size: 12px;
    color: var(--text-muted);
    user-select: text;
    -webkit-user-select: text;
    word-break: break-all;
  }

  @media (prefers-reduced-motion: reduce) {
    .launch-btn.breathing {
      animation: none;
    }
    .logs .line {
      animation: none;
    }
    .indeterminate::after {
      animation: none;
    }
  }
</style>
