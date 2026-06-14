# Stata AI Skill Native Service

Native localhost HTTP service that lets AI agents run Stata without VS Code,
Node.js, or Python on the user side.

> This project is extracted from [ZihaoVistonWang/stata-all-in-one](https://github.com/ZihaoVistonWang/stata-all-in-one) and keeps the AI Skill HTTP workflow while removing the VS Code runtime requirement.

The distributed artifact is a single native executable:

- macOS: `stata-ai-skill` (Mach-O executable)
- Windows: `stata-ai-skill.exe` (PE executable)

Users do not manually install or edit configuration. An AI agent can launch the
service, check status, ask where the Stata app/program is installed only when
needed, write config via CLI, and call the HTTP API.

## Quick Start

```bash
stata-ai-skill serve
curl http://127.0.0.1:19522/status
```

If Stata cannot be found, `/status` returns `needsConfiguration: true`. The
agent should ask the user where the Stata app/program is installed and run:

```bash
stata-ai-skill config set --stata-path "/Applications/StataMP.app"
stata-ai-skill serve
```

Windows example:

```powershell
.\stata-ai-skill.exe config set --stata-path "C:\Program Files\Stata18\StataMP-64.exe"
.\stata-ai-skill.exe serve
```

User-facing wording for agents:

- macOS: "Open Finder > Applications, find the Stata app icon, and tell me its
  name/location. You can also drag the Stata app into Terminal to reveal a path
  like `/Applications/StataNow/StataMP.app`."
- Windows: "Find Stata in the Start menu or under `C:\Program Files\Stata...`.
  The program may be named `StataMP-64.exe`, `StataSE-64.exe`, or similar."

Accepted examples:

- macOS app: `/Applications/StataMP.app`
- macOS nested app: `/Applications/StataNow/StataMP.app`
- macOS library: `/Applications/StataMP.app/Contents/MacOS/libstata-mp.dylib`
- Windows folder: `C:\Program Files\Stata18`
- Windows exe: `C:\Program Files\Stata18\StataMP-64.exe`
- Windows DLL: `C:\Program Files\Stata18\mp-64.dll`

## HTTP API

- `GET /status`
- `POST /execute` with `{ "code": "...", "file": "...", "timeout": 30, "echo": false }`
- `POST /break`
- `POST /shutdown`

Example:

```bash
curl -s -X POST http://127.0.0.1:19522/execute \
  -H "Content-Type: application/json" \
  -d '{"code":"display 2+2"}'
```

## System Directories

The service never creates `.stata-all-in-one/` in the repository or current
working directory.

- macOS config: `~/Library/Application Support/stata-ai-skill/config.toml`
- macOS logs: `~/Library/Logs/stata-ai-skill/`
- macOS graphs: `~/Library/Application Support/stata-ai-skill/graphs/`
- macOS temp: system temp directory under `stata-ai-skill/`
- Windows config: `%APPDATA%\StataAISkill\config.toml`
- Windows logs: `%LOCALAPPDATA%\StataAISkill\Logs\`
- Windows graphs: `%LOCALAPPDATA%\StataAISkill\Graphs\`
- Windows temp: `%TEMP%\StataAISkill\`

## Development

Install Rust on macOS with Homebrew:

```bash
xcode-select --install
brew update
brew install rust
cargo build
```
