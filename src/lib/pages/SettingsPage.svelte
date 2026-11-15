<script lang="ts">
  import { settings, saveSettings, loadSettings, applyWindowEffect } from "$lib/stores/settings.svelte";
  import { theme, setTheme } from "$lib/stores/theme.svelte";
  import {
    pathsGet,
    openDataPath,
    openExternal,
    dataDirSet,
    dataDirReset,
    appRestart,
    javaDetect,
    systemTotalMemoryMb,
    type AppPathsInfo,
    type DetectResult,
  } from "$lib/api/app";
  import { manifestGet, type Manifest } from "$lib/api/manifest";
  import { open } from "@tauri-apps/plugin-dialog";

  let { active = false } = $props();

  let saving = $state(false);
  let savedAt = $state("");
  let paths = $state<AppPathsInfo | null>(null);

  // 数据目录迁移
  let dirBusy = $state(false);
  let dirError = $state("");
  let dirNotice = $state("");
  let dirNeedsRestart = $state(false);
  let restarting = $state(false);

  /** 致敬名单：点击用系统浏览器打开 bilibili 主页 */
  const TRIBUTES = [
    {
      role: "启动器开发者",
      links: [
        { label: "B站 · 516308333", url: "https://space.bilibili.com/516308333" },
        { label: "B站 · 3706927473232492", url: "https://space.bilibili.com/3706927473232492" },
      ],
    },
    {
      role: "AirPlus 主要开发者",
      links: [
        { label: "B站 · 3706927473232492", url: "https://space.bilibili.com/3706927473232492" },
      ],
    },
  ];

  // 下载清单：下过哪些客户端版本、装过哪些模组（含 sha1）
  let manifest = $state<Manifest | null>(null);
  // 打开目录/文件失败时的提示（以前静默失败，看着像按钮没反应）
  let openError = $state("");

  // 镜像输入框草稿（自定义源用）
  let mirrorDraft = $state("");
  // 下载源选择：官方 / BMCLAPI / 自定义
  let mirrorKind = $state<MirrorKind>("official");
  // JVM 参数草稿（一行一个）
  let jvmDraft = $state("");

  // Java 运行时检测：null = 还没检测过；undefined 区分不出，改用独立 loaded 标记
  let javaBusy = $state(false);
  let javaDetectRes = $state<DetectResult | null>(null);
  let javaScanned = $state(false);
  /** 物理内存总量 (MB)，保存内存时校验用；null = 后端拿不到 */
  let totalMemoryMb = $state<number | null>(null);
  let ramWarning = $state("");
  /** 最近一次复制成功的 sha1（按钮上短暂显示「已复制」） */
  let copiedSha = $state<string | null>(null);

  let initialized = $state(false);
  $effect(() => {
    if (!active || initialized) return;
    initialized = true;
    pathsGet().then((p) => (paths = p));
    void refreshManifest();
    systemTotalMemoryMb().then((n) => (totalMemoryMb = n));
    // 等设置真正加载完再回填草稿，否则会读到默认值
    void loadSettings().then(() => {
      mirrorDraft = settings.mirror ?? "";
      mirrorKind = detectMirror(settings.mirror);
      jvmDraft = settings.extraJvmArgs.join("\n");
    });
    // 打开设置页时顺手查一次系统 Java，让状态一眼可见
    void detectJava();
  });

  async function refreshManifest() {
    try {
      manifest = await manifestGet();
    } catch (e) {
      openError = `读取下载记录失败：${String(e)}`;
    }
  }

  /**
   * 用系统默认程序打开路径（走后端 opener，不走前端插件 ACL）。
   * 失败必须弹出来，不然表现就是「点了没反应」。
   */
  async function openTarget(path: string) {
    openError = "";
    try {
      await openDataPath(path);
    } catch (e) {
      openError = `打开失败：${String(e)}`;
    }
  }

  /** 致敬链接：用系统浏览器打开（后端开，避免前端 ACL 限制） */
  async function openLink(url: string) {
    openError = "";
    try {
      await openExternal(url);
    } catch (e) {
      openError = `打开链接失败：${String(e)}`;
    }
  }

  /** 选择新数据目录：后端负责迁移现有数据 */
  async function changeDataDir() {
    dirError = "";
    dirNotice = "";
    const picked = await open({
      directory: true,
      multiple: false,
      title: "选择数据目录（客户端、模组、JRE 都会保存到这里）",
    });
    if (typeof picked !== "string" || !picked) return;
    dirBusy = true;
    try {
      const next = await dataDirSet(picked);
      dirNeedsRestart = true;
      dirNotice = `已迁移到 ${next}，重启启动器后生效。`;
    } catch (e) {
      dirError = String(e);
    } finally {
      dirBusy = false;
    }
  }

  /** 恢复默认目录（旧目录里的数据不删，只是切回去） */
  async function restoreDefaultDir() {
    dirError = "";
    dirNotice = "";
    dirBusy = true;
    try {
      const next = await dataDirReset();
      dirNeedsRestart = true;
      dirNotice = `已恢复默认目录 ${next}，重启启动器后生效（旧目录数据未删除）。`;
    } catch (e) {
      dirError = String(e);
    } finally {
      dirBusy = false;
    }
  }

  async function restartNow() {
    restarting = true;
    try {
      await appRestart();
    } catch (e) {
      restarting = false;
      dirError = `重启失败，请手动关闭后重新打开：${String(e)}`;
    }
  }

  function sizeText(bytes: number): string {
    if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    return `${bytes} B`;
  }

  function shortSha1(sha1: string | null): string {
    return sha1 ? `${sha1.slice(0, 8)}…` : "无";
  }

  function dateText(iso: string): string {
    // 后端存 UTC，这里转本地时间显示
    const d = new Date(iso);
    return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
  }

  const EFFECTS = [
    { value: "acrylic", label: "亚克力（半透明毛玻璃）" },
    { value: "mica", label: "云母（Win11 系统材质）" },
    { value: "none", label: "不透明（兼容性最好）" },
  ];

  type MirrorKind = "official" | "bmclapi" | "custom";
  const BMCLAPI = "https://bmclapi2.bangbang93.com";
  const MIRRORS: { value: MirrorKind; label: string; url: string | null }[] = [
    { value: "official", label: "官方源（Mojang 直连）", url: null },
    { value: "bmclapi", label: "BMCLAPI 镜像（国内加速）", url: BMCLAPI },
    { value: "custom", label: "自定义地址…", url: null },
  ];

  /** 从已保存的地址反推属于哪一档 */
  function detectMirror(url: string | null): MirrorKind {
    if (!url) return "official";
    return url.replace(/\/+$/, "") === BMCLAPI ? "bmclapi" : "custom";
  }

  function onEffectChange(e: Event) {
    const value = (e.currentTarget as HTMLSelectElement).value as typeof settings.windowEffect;
    settings.windowEffect = value;
    applyWindowEffect(value); // 立即预览，保存时落盘
  }

  /** 检测系统里的所有 Java（JAVA_HOME / PATH / 注册表 / 常见安装目录） */
  async function detectJava() {
    if (javaBusy) return;
    javaBusy = true;
    try {
      javaDetectRes = await javaDetect();
      javaScanned = true;
    } catch (e) {
      openError = `检测系统 Java 失败：${String(e)}`;
    } finally {
      javaBusy = false;
    }
  }

  /** 手动指定 Java：文件对话框选 java.exe，作为最高优先级 */
  async function browseJava() {
    const picked = await open({
      multiple: false,
      title: "选择 java.exe（留空取消）",
      filters: [{ name: "Java 可执行文件", extensions: ["exe"] }],
    });
    if (typeof picked !== "string" || !picked) return;
    settings.javaPath = picked;
  }

  /** 内存设置保存前校验：超过物理内存会在启动游戏时 OOM */
  function validateRam(): boolean {
    ramWarning = "";
    if (totalMemoryMb !== null && settings.ramMb > totalMemoryMb) {
      ramWarning = `设置的内存 ${settings.ramMb} MB 超过物理内存 ${totalMemoryMb} MB，游戏启动时会内存不足。`;
      return false;
    }
    return true;
  }

  async function save() {
    saving = true;
    try {
      if (!validateRam()) {
        saving = false;
        return;
      }
      settings.mirror = selectedMirror();
      settings.extraJvmArgs = jvmDraft
        .split("\n")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      await saveSettings();
      savedAt = new Date().toLocaleTimeString();
    } finally {
      saving = false;
    }
  }

  /** 复制完整 sha1（清单里的 8 位缩写对不上账时用） */
  async function copySha1(sha: string | null) {
    if (!sha) return;
    try {
      await navigator.clipboard.writeText(sha);
      copiedSha = sha;
      setTimeout(() => {
        if (copiedSha === sha) copiedSha = null;
      }, 1500);
    } catch {
      /* 剪贴板权限被拒时静默，用户仍可手选 */
    }
  }

  /** 当前选择对应的镜像地址（官方源 = null） */
  function selectedMirror(): string | null {
    if (mirrorKind === "custom") {
      const url = mirrorDraft.trim().replace(/\/+$/, "");
      return url === "" ? null : url;
    }
    return MIRRORS.find((m) => m.value === mirrorKind)?.url ?? null;
  }
</script>

<div class="page">
  <h2>设置</h2>

  <section class="card block tribute">
    <span class="label">致敬</span>
    {#each TRIBUTES as t (t.role)}
      <div class="tribute-row">
        <span class="role">{t.role}</span>
        <div class="links">
          {#each t.links as link, i (link.url + i)}
            <button class="dev-link" onclick={() => openLink(link.url)} title={link.url}>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M4 5h16v14H4zM4 9h16M8 13h5"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                />
              </svg>
              {link.label}
            </button>
          {/each}
        </div>
      </div>
    {/each}
    <p class="hint">点击名字可跳转到对应的 B 站主页。</p>
  </section>

  <section class="card block">
    <span class="label">外观</span>
    <div class="segmented">
      <span class="thumb" class:light={theme.current === "light"}></span>
      <button class:on={theme.current === "dark"} onclick={() => setTheme("dark")}>深色</button>
      <button class:on={theme.current === "light"} onclick={() => setTheme("light")}>浅色</button>
    </div>
    <p class="hint">主题跟随启动器，不影响游戏内客户端。</p>
  </section>

  <section class="card block">
    <span class="label">窗口背景</span>
    <select class="input" value={settings.windowEffect} onchange={onEffectChange}>
      {#each EFFECTS as e (e.value)}
        <option value={e.value}>{e.label}</option>
      {/each}
    </select>
    <p class="hint">若拖动窗口时明显掉帧，可切到「云母」或「不透明」。</p>
  </section>

  <section class="card block">
    <span class="label">游戏内存 (MB)</span>
    <input
      class="input"
      type="number"
      min="1024"
      max="16384"
      step="512"
      bind:value={settings.ramMb}
      onchange={validateRam}
    />
    {#if ramWarning}
      <p class="error">{ramWarning}</p>
    {:else if totalMemoryMb !== null}
      <p class="hint">本机物理内存 {totalMemoryMb} MB。</p>
    {/if}
  </section>

  <section class="card block">
    <div class="row">
      <span class="label">Java 运行时</span>
      <div class="row-actions">
        <button class="btn small" onclick={browseJava}>浏览…</button>
        <button class="btn small" onclick={detectJava} disabled={javaBusy}>
          {javaBusy ? "检测中…" : "检测系统 Java"}
        </button>
        {#if paths}
          <button class="btn small" onclick={() => openTarget(paths!.jreDir)}>打开 JRE 目录</button>
        {/if}
      </div>
    </div>

    {#if settings.javaPath}
      <code class="dir">手动指定：{settings.javaPath}</code>
      <div class="row-actions">
        <button class="btn small" onclick={() => (settings.javaPath = null)}>
          清除手动指定
        </button>
      </div>
      <p class="notice">手动指定的路径优先于内置 JRE 与系统 Java（记得保存设置）。</p>
    {/if}

    {#if !javaScanned}
      <p class="hint">正在本机查找可用的 Java…</p>
    {:else if javaDetectRes?.java8}
      <p class="notice">
        已找到系统 Java 8：{javaDetectRes.java8.vendor
          ? `Java ${javaDetectRes.java8.version} · ${javaDetectRes.java8.vendor}`
          : `Java ${javaDetectRes.java8.version}`}
      </p>
      <code class="dir">{javaDetectRes.java8.path}</code>
      <p class="hint">启动时优先使用（内置 JRE 缺失时不再联网下载）。</p>
    {:else if javaDetectRes && javaDetectRes.others.length > 0}
      <p class="notice-warn">
        检测到 {javaDetectRes.others
          .slice(0, 3)
          .map(
            (o) => `Java ${o.version}${o.vendor ? ` (${o.vendor})` : ""}`,
          )
          .join("、")}，但 Forge 1.8.9 需要 Java 8 —— 启动时会自动下载 Temurin 8 JRE，属正常现象。
      </p>
    {:else}
      <p class="hint">
        未检测到系统 Java 8。首次启动游戏时会自动下载 Temurin 8 JRE（约 40 MB）到数据目录。
      </p>
    {/if}
    <p class="hint">
      只认 Java 8 —— Forge 1.8.9 无法在 Java 9 及以上版本运行。
      查找范围：JAVA_HOME、PATH、注册表与常见安装目录；结果会缓存到 settings.json。
    </p>
  </section>

  <section class="card block">
    <span class="label">下载源</span>
    <select class="input" bind:value={mirrorKind}>
      {#each MIRRORS as m (m.value)}
        <option value={m.value}>{m.label}</option>
      {/each}
    </select>
    {#if mirrorKind === "custom"}
      <input
        class="input"
        type="text"
        placeholder="https://your-mirror.example.com"
        bind:value={mirrorDraft}
      />
      <p class="hint">留空则回退到官方源。地址需兼容 Mojang 的目录结构（/maven、/assets 子路径）。</p>
    {/if}
    <p class="hint">
      影响版本清单、依赖库、游戏资源与 Forge 的下载地址；国内网络建议选 BMCLAPI。
      Modrinth 与 GitHub 走自己的 CDN，不受此项影响。
    </p>
  </section>

  <section class="card block">
    <span class="label">额外 JVM 参数（每行一条）</span>
    <textarea
      class="input area"
      rows="3"
      placeholder={"-XX:+UseG1GC\n-Dsun.java2d.d3d=false"}
      bind:value={jvmDraft}
    ></textarea>
  </section>

  {#if paths}
    <section class="card block">
      <span class="label">数据目录</span>
      <code class="dir">{paths.root}</code>
      <div class="row-actions wrap">
        <button class="btn small" onclick={() => openTarget(paths!.root)}>打开目录</button>
        <button class="btn small" onclick={changeDataDir} disabled={dirBusy}>
          {dirBusy ? "迁移中…" : "更改目录…"}
        </button>
        <button
          class="btn small"
          onclick={restoreDefaultDir}
          disabled={dirBusy || paths.root === paths.defaultRoot}
        >
          恢复默认
        </button>
      </div>
      <p class="hint">
        客户端、模组、JRE 与下载缓存都会保存在这里。更改目录时会把现有数据整体迁移过去，重启启动器后生效。
      </p>
      {#if dirNotice}
        <p class="notice">{dirNotice}</p>
      {/if}
      {#if dirError}
        <p class="error">{dirError}</p>
      {/if}
      {#if dirNeedsRestart}
        <button class="btn small primary" onclick={restartNow} disabled={restarting}>
          {restarting ? "正在重启…" : "立即重启启动器"}
        </button>
      {/if}
    </section>

    <section class="card block">
      <div class="row">
        <span class="label">下载记录</span>
        <div class="row-actions">
          <button class="btn small" onclick={() => openTarget(paths!.manifestFile)}>
            打开清单
          </button>
          <button class="btn small" onclick={refreshManifest}>刷新</button>
        </div>
      </div>
      <p class="hint">
        客户端版本与模组的来源、sha1、安装时间都记在
        <code class="dir mono">launcher-manifest.json</code>，重装或校验时可直接对照。
      </p>

      {#if manifest}
        <div class="group">
          <span class="group-title">AirPlus 客户端（{manifest.clients.length}）</span>
          {#if manifest.clients.length === 0}
            <p class="hint">还没有下载过客户端。</p>
          {:else}
            <ul class="records">
              {#each manifest.clients as c (c.fileName)}
                <li>
                  <div class="line">
                    <span class="name" class:dim={!c.present}>{c.tag ?? "未知版本"}</span>
                    <span class="tag" class:on={c.active}>
                      {c.active ? "生效中" : c.present ? "已下载" : "已清理"}
                    </span>
                  </div>
                  <div class="meta">
                    {c.fileName} · {sizeText(c.size)} · sha1 {shortSha1(c.sha1)}
                    {#if c.sha1}
                      <button class="copy-btn" onclick={() => copySha1(c.sha1!)} title="复制完整 sha1">
                        {copiedSha === c.sha1 ? "已复制" : "复制"}
                      </button>
                    {/if}
                    · {dateText(c.installedAt)}
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        </div>

        <div class="group">
          <span class="group-title">模组（{manifest.mods.length}）</span>
          {#if manifest.mods.length === 0}
            <p class="hint">还没有安装过模组。</p>
          {:else}
            <ul class="records">
              {#each manifest.mods as m (m.fileName)}
                <li>
                  <div class="line">
                    <span class="name" class:dim={!m.present}>{m.fileName}</span>
                    <span class="tag">{m.source}</span>
                  </div>
                  <div class="meta">
                    {sizeText(m.size)} · sha1 {shortSha1(m.sha1)}
                    {#if m.sha1}
                      <button class="copy-btn" onclick={() => copySha1(m.sha1!)} title="复制完整 sha1">
                        {copiedSha === m.sha1 ? "已复制" : "复制"}
                      </button>
                    {/if}
                    {#if m.versionNumber}· {m.versionNumber}{/if}
                    · {dateText(m.installedAt)}
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}
    </section>
  {/if}

  {#if openError}
    <p class="error">{openError}</p>
  {/if}

  <div class="actions">
    <button class="btn primary" onclick={save} disabled={saving}>
      {saving ? "保存中…" : "保存设置"}
    </button>
    {#if savedAt}
      <span class="muted">已保存 {savedAt}</span>
    {/if}
  </div>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 14px;
    max-width: 560px;
  }
  h2 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
  }
  .block {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 16px 18px;
  }
  .label {
    font-size: 13px;
    color: var(--text-muted);
  }
  /* 致敬：角色 + 可点击的 B 站链接 */
  .tribute-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }
  .role {
    font-size: 13px;
    color: var(--text);
  }
  .links {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .dev-link {
    position: relative;
    overflow: hidden;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 12px;
    border-radius: var(--radius-full);
    border: 1px solid var(--line);
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 12px;
    transition:
      background var(--dur-fast) var(--ease-out),
      transform var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .dev-link:hover {
    background: var(--accent-strong);
    border-color: var(--accent);
    transform: translateY(-1px);
  }
  .dev-link svg {
    width: 13px;
    height: 13px;
    flex: none;
  }
  .notice {
    margin: 0;
    font-size: 12px;
    color: var(--ok);
    word-break: break-all;
  }
  .notice-warn {
    margin: 0;
    font-size: 12px;
    color: var(--text);
    word-break: break-all;
  }
  .copy-btn {
    padding: 0 6px;
    margin: 0 2px;
    border: none;
    background: none;
    color: var(--accent);
    font-size: 11px;
    cursor: pointer;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .copy-btn:hover {
    text-decoration: underline;
  }
  /* 主题分段控件：滑块在容器内 spring 位移 */
  .segmented {
    position: relative;
    display: inline-flex;
    align-self: flex-start;
    padding: 3px;
    border-radius: var(--radius-full);
    background: var(--window-strong);
    border: 1px solid var(--line);
  }
  .thumb {
    position: absolute;
    top: 3px;
    bottom: 3px;
    left: 3px;
    width: calc(50% - 3px);
    border-radius: var(--radius-full);
    background: var(--accent-soft);
    transition: transform var(--dur) var(--ease-spring);
  }
  .thumb.light {
    transform: translateX(100%);
  }
  .segmented button {
    position: relative;
    z-index: 1;
    padding: 7px 22px;
    border-radius: var(--radius-full);
    font-size: 13px;
    color: var(--text-muted);
    transition: color var(--dur-fast) var(--ease-out);
  }
  .segmented button.on {
    color: var(--accent);
  }
  .input {
    background: var(--window);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    color: var(--text);
    padding: 9px 12px;
    font-size: 13px;
    outline: none;
    width: 100%;
    box-sizing: border-box;
  }
  .input:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  select.input option {
    background: var(--window-strong);
    color: var(--text);
  }
  .area {
    resize: vertical;
    font-family: "Cascadia Mono", Consolas, monospace;
    font-size: 12px;
  }
  .dir {
    font-family: "Cascadia Mono", Consolas, monospace;
    font-size: 12px;
    color: var(--text-muted);
    user-select: text;
    -webkit-user-select: text;
    word-break: break-all;
  }
  .hint {
    margin: 0;
    font-size: 12px;
    color: var(--text-muted);
  }
  .btn.small {
    padding: 6px 12px;
    font-size: 12px;
    align-self: flex-start;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .muted {
    font-size: 12px;
    color: var(--text-muted);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  .row-actions {
    display: flex;
    gap: 8px;
  }
  .row-actions.wrap {
    flex-wrap: wrap;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: 4px;
    border-top: 1px solid var(--line);
  }
  .group-title {
    font-size: 12px;
    color: var(--text-muted);
    letter-spacing: 0.3px;
  }
  .records {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 220px;
    overflow-y: auto;
  }
  .records li {
    background: var(--window);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    padding: 7px 10px;
  }
  .line {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .name {
    font-size: 12.5px;
    font-weight: 600;
    word-break: break-all;
  }
  .name.dim {
    color: var(--text-muted);
    font-weight: 400;
  }
  .tag {
    flex: none;
    font-size: 11px;
    color: var(--text-muted);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 1px 8px;
  }
  .tag.on {
    color: var(--ok);
    border-color: var(--ok);
  }
  .meta {
    margin-top: 3px;
    font-size: 11.5px;
    color: var(--text-muted);
    font-family: "Cascadia Mono", Consolas, monospace;
  }
  .mono {
    font-size: 11.5px;
  }
  .error {
    margin: 0;
    font-size: 12px;
    color: var(--danger);
    word-break: break-all;
    user-select: text;
    -webkit-user-select: text;
  }
</style>
