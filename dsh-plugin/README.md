# @zihaovistonwang/stata-ai-skill

**The Stata AI Skill is the DeepSeek Harness (DSH) plugin for running Stata** — it brings the native Stata AI Skill service into DSH: the service starts with DSH and stops on shutdown.

**Stata AI Skill 是 DeepSeek Harness (DSH) 运行 Stata 的插件** —— 它把 Stata AI Skill 原生服务接入 DSH:服务随 DSH 一起启动、随 DSH 关闭。

> This plugin is the one with which DeepSeek Harness (DSH) runs Stata: as a Cordis bundle plugin of DSH, it hosts the Stata AI Skill native background service (HTTP `127.0.0.1:19522`) inside the DSH lifecycle. It does not bundle Stata itself and does not replace the skill — it only guarantees the service is online whenever DSH is up.
>
> 本插件就是 DeepSeek Harness (DSH) 用来运行 Stata 的插件:作为 DSH 的 Cordis bundle 插件,它把 Stata AI Skill 的原生后台服务(HTTP `127.0.0.1:19522`)托管进 DSH 的生命周期。它本身不包含 Stata,也不替代技能本身——它只保证 DSH 在运行时服务始终在线。

## What it does / 它做什么

- **Starts with DSH / 随 DSH 启动**: when the DSH process starts, the plugin automatically launches the Stata AI Skill native service in `serve` mode (default port 19522). DSH 进程启动时,自动以 `serve` 模式拉起 Stata AI Skill 原生服务(默认端口 19522)。
- **Platform-aware / 平台自适应**(precompiled binaries for every platform are bundled in the package / 安装包内已内置各平台预编译二进制):
  - macOS Apple Silicon → `bin/macos-arm64/stata-ai-skill`
  - macOS legacy fallback → `bin/macos/stata-ai-skill`
  - Windows x64 → `bin/windows/stata-ai-skill.exe`
  - Windows ARM64 → `bin/windows-arm64/stata-ai-skill.exe`
  - Intel Mac is not supported (consistent with the skill itself); the plugin logs an error instead of starting. Intel Mac 不支持(与 skill 本身一致),会记录错误日志而不是启动。
- **Reuses an existing service / 复用已有服务**: if a Stata AI Skill service is already running on the port (e.g. started manually), the plugin reuses it instead of starting a second one. 如果端口上已经有 Stata AI Skill 服务在运行(例如手动启动的),插件不会重复启动,直接复用。
- **Restarts on crash / 崩溃自动重启**: if the service exits unexpectedly, the plugin restarts it with exponential backoff (5 attempts max by default). 服务意外退出时按指数退避自动重启(默认最多 5 次)。
- **Stops with DSH / 随 DSH 关闭**: on DSH shutdown, the plugin first sends HTTP `/shutdown` for a graceful stop (safely closing the embedded Stata session), then force-kills the process if it does not exit in time. DSH 退出时先发 HTTP `/shutdown` 优雅关闭(安全关闭嵌入的 Stata 会话),超时后强制结束进程。

The plugin only manages the service lifecycle; how you run Stata stays unchanged — the `stata-ai-skill` skill (SKILL.md) in DSH still talks to `http://127.0.0.1:19522` over HTTP, and the plugin guarantees the service is online.

插件只管理服务生命周期;运行 Stata 的用法不变——DSH 里的 `stata-ai-skill` 技能(SKILL.md)仍然通过 `http://127.0.0.1:19522` 的 HTTP 接口工作,插件保证它在线。

## Prerequisites / 前提条件

- [DeepSeek Harness](https://github.com/deepseek-ai/dsh) installed (the `dsh` command available). 已安装 [DeepSeek Harness](https://github.com/deepseek-ai/dsh)(`dsh` 命令可用)。
- A locally installed and licensed Stata (Apple Silicon macOS or Windows; Intel Mac is not supported). 本机已安装并授权 Stata(Apple Silicon macOS 或 Windows;Intel Mac 不支持)。
- The [stata-ai-skill skill](https://github.com/ZihaoVistonWang/Stata-AI-Skill) installed (the SKILL.md skill directory). 本机已安装 [stata-ai-skill 技能](https://github.com/ZihaoVistonWang/Stata-AI-Skill)(SKILL.md 技能目录)。

## Installation / 安装

Run this from any directory (omitting the version installs the latest):

在任意目录执行(不写版本号即安装最新版):

```bash
dsh plugin --profile web add @zihaovistonwang/stata-ai-skill
```

`dsh plugin` installs the package into `~/.dsh/profiles/web/node_modules` and automatically adds `@zihaovistonwang/stata-ai-skill` to the `dsh.profile.bundles` layer (the package declares `dsh.bundle.patch`). Then restart DSH Web:

`dsh plugin` 会把包安装进 `~/.dsh/profiles/web/node_modules`,并自动把 `@zihaovistonwang/stata-ai-skill` 加入 `dsh.profile.bundles` 层(因为包声明了 `dsh.bundle.patch`)。然后重启 DSH Web:

```bash
dsh web
```

Once the plugin is active, the service starts automatically with DSH. Verify:

插件生效后,服务会随 DSH 自动启动。验证:

```bash
curl -s http://127.0.0.1:19522/status
```

A JSON response means success. 返回 JSON 即成功。

### Update / 更新

```bash
dsh plugin --profile web update @zihaovistonwang/stata-ai-skill
```

### Uninstall / 卸载

```bash
dsh plugin --profile web remove @zihaovistonwang/stata-ai-skill
```

## Configuration / 配置

You can override the row config in the profile's user layer `~/.dsh/profiles/web/cordis.patch.yml` (the whole config is replaced, so restate every field you want to keep):

安装后可在 profile 的用户层 `~/.dsh/profiles/web/cordis.patch.yml` 中覆盖行配置(整块替换,需要补全要保留的字段):

```yaml
- id: stata-ai-skill
  name: '@zihaovistonwang/stata-ai-skill'
  config:
    port: 19522            # service listening port / 服务监听端口
    binPath: null          # explicit executable path (overrides platform detection) / 显式指定可执行文件路径(覆盖内置平台探测)
    startTimeoutMs: 30000  # max time to wait for /status after startup / 启动后等待 /status 上线的最长时间
    maxRestarts: 5         # max consecutive restarts after unexpected exit / 意外退出后的最大连续重启次数
    restartBackoffMs: 1000 # restart backoff base (doubles each attempt) / 重启退避基数(每次翻倍)
```

`binPath` can also be overridden with the `STATA_AI_SKILL_BIN` environment variable.

`binPath` 也可用环境变量 `STATA_AI_SKILL_BIN` 覆盖。

## Source & Development / 源码与开发

- Repository / 仓库: [ZihaoVistonWang/Stata-AI-Skill](https://github.com/ZihaoVistonWang/Stata-AI-Skill) (the plugin lives in the `dsh-plugin/` directory / 插件位于 `dsh-plugin/` 目录)
- Package layout / 包结构:
  - `lib/index.js` — Cordis host plugin (service lifecycle management) / Cordis host 插件(服务生命周期管理)
  - `cordis.patch.yml` — bundle patch row / bundle patch 行
  - `bin/<platform>/` — precompiled native service binaries (macos / macos-arm64 / windows / windows-arm64) / 预编译原生服务二进制(macos / macos-arm64 / windows / windows-arm64)
  - `stata/aiskill/` — Stata-side files (aiskill.ado etc., used to run `aiskill setup` in GUI Stata to complete configuration) / Stata 侧文件(aiskill.ado 等,供 GUI Stata 里执行 `aiskill setup` 完成配置)
  - `scripts/discover_stata_windows.bat` — Windows Stata discovery script / Windows Stata 发现脚本
- Local development install / 本地开发安装: `dsh plugin --profile web add link:./dsh-plugin`
- Smoke tests (need Node 18+; they use dedicated ports so a running service is not affected; directory: `test/`) / 冒烟测试(需要 Node 18+,用独立端口,不影响在跑的服务;目录:`test/`):
  - `node test/smoke-spawn.mjs` — verifies the plugin starts the service, `/status` comes online, and the disposer shuts it down (port 19523) / 验证插件拉起服务、`/status` 上线、disposer 优雅关闭(端口 19523)
  - `node test/smoke-reuse.mjs` — verifies the plugin reuses an existing service on the port without starting a second one (port 19522; start a service manually first) / 验证端口上已有服务时插件复用而不重复启动(端口 19522,需先手动起一个服务)
  - `node test/smoke-e2e.mjs` — end-to-end: start the service → run a real Stata command → dispose shuts it down (port 19524) / 端到端:拉起服务 → 跑真实 Stata 命令 → dispose 关闭(端口 19524)
  - `node test/smoke-crash-restart.mjs` — hard-kills the service process and verifies the plugin restarts it as a new process (port 19525) / 强行 kill 服务进程,验证插件自动重启为新进程(端口 19525)

## License / 许可

MIT
