# Java 运行时查找逻辑

AirLauncher 只使用 **Java 8**。本文解释为什么、从哪里找、找不到会怎样。

## 为什么只认 Java 8

本启动器面向 Minecraft 1.8.9 + Forge 11.15.1.2318：

- Forge 1.8.9 的安装流程（`install_profile.json` 版本 JSON）依赖老的
  `minecraftArguments` 与 `launchwrapper`，在 Java 9+ 的模块系统下无法运行；
- 游戏本体 1.8.9 对 Java 9+ 也有兼容问题（类版本、反射调用等）。

所以即使机器上装了 Java 17 / 21，启动器也不会使用它，
而是**自动下载** Temurin 8 JRE——这不是"检测失败"，是预期行为。
设置页的检测会把这些高版本 Java 一并列出，让你知道它们已被发现。

## 查找优先级（从高到低）

1. **手动指定**：设置 → Java 运行时 → 「浏览…」选择的 `javaPath`
   （settings.json，最高优先级；路径失效时跳过并提示）
2. **内置 JRE**：`<数据目录>/jre/bin/java.exe`
3. **系统 Java 8**，依次尝试：
   - `JAVA_HOME` 环境变量
   - `PATH` 里的 `java.exe`
   - 注册表 `JavaSoft\JDK` / `Java Development Kit` / `Java Runtime Environment`
     （HKLM 与 HKCU，含 WOW6432Node；经典 CurrentVersion 布局与新布局都支持）
   - 常见安装目录（`Program Files\Java`、`Eclipse Adoptium`、`Zulu`、
     `Amazon Corretto`、`BellSoft`、`Microsoft`、`Semeru`、`ojdkbuild`）

   每个候选路径都会跑一次 `java -version` 验证主版本号（候选并发验证）。
   只接受主版本号为 8 的（`1.8.0_xxx` → 8）。
4. **自动下载**：Adoptium Temurin 8 JRE (windows x64, 约 40 MB)
   - 优先清华 TUNA 镜像 `mirrors.tuna.tsinghua.edu.cn/Adoptium/8/jre/x64/windows/`，
     失败回退官方 CDN
   - 下载完成后按 Adoptium API 返回的 `binary.package.checksum`（sha256）校验，
     通过的 zip 会缓存复用（`cache/<文件名>.zip.ok` 标记）
   - 解压到 `<数据目录>/jre`（去掉 zip 内层目录，使 java.exe 落在 `jre/bin/`）

## 结果缓存

- **会话内**：内存缓存最近一次系统扫描结果，同一次运行里启动多局不再重扫。
- **跨启动**：结果写入 settings.json（`javaSource` + `systemJavaPath`）。
  下次冷启动先校验缓存路径仍存在，存在则直接用，避免重复
  读注册表 + 多次 `java -version`（约几百毫秒）；路径失效才重新扫描。

## 手动检测（设置页）

「检测系统 Java」按钮触发一次绕过缓存的**全量扫描**，返回：

- Java 8 路径与版本信息（如 `Java 1.8.0_402 · Temurin`）
- 其它版本列表（11/17/21…），提示"需要 Java 8，将自动下载"

版本号与发行商解析自 `java -version` 的 stderr（`1.8.0_402` → 主版本 8；
`17.0.10` → 17；发行商取 "Runtime Environment" 后括号里的名字）。
中文 Windows 下 stderr 可能是 GBK，版本行均为 ASCII 不受影响。
