# Stata AI Skill Native Service

Native localhost HTTP service that lets AI agents run Stata without VS Code,
Node.js, or Python on the user side.

> This project is extracted from [ZihaoVistonWang/stata-all-in-one](https://github.com/ZihaoVistonWang/stata-all-in-one) and keeps the AI Skill HTTP workflow while removing the VS Code runtime requirement.

The distributed artifact is a single native executable:

- macOS: `stata-ai-skill` (Mach-O executable)
- Windows: `stata-ai-skill.exe` (PE executable)

macOS support is Apple Silicon only. Intel Mac is not supported.

For agent use and skills.sh publishing, package the executable next to the
skill definition:

```text
skills/
  stata-all-in-one-skill/
    SKILL.md
    bin/
      macos/
        stata-ai-skill
      macos-arm64/
        stata-ai-skill
      windows/
        stata-ai-skill.exe
      windows-arm64/
        stata-ai-skill.exe
```

Agents should resolve the executable from
`skills/stata-all-in-one-skill/bin/<platform>/` first. Use `macos-arm64` for
Apple Silicon Macs, `windows` for Windows x64, and `windows-arm64` for Windows
ARM64. Intel Mac is not supported; agents should detect `x86_64` macOS and
report that the skill is not compatible.
`skills/stata-all-in-one-skill/bin/macos/` remains a legacy Apple Silicon
fallback. Cargo's `target/release/` directory is only a development build
output, not the runtime contract.

To build and refresh the packaged binary for the current platform, run:

```bash
cargo run -p xtask -- dist
```

This runs a release build and copies the executable into
`skills/stata-all-in-one-skill/bin/<platform>/`.

## Install With skills.sh

```bash
npx skills add ZihaoVistonWang/Stata-AI-Skill --skill stata-all-in-one-skill
```

Users do not manually install or edit configuration. An AI agent can launch the
service, check status, ask where the Stata app/program is installed only when
needed, write config via CLI, and call the HTTP API.

## Quick Start

```bash
./skills/stata-all-in-one-skill/bin/macos-arm64/stata-ai-skill serve
curl http://127.0.0.1:19522/status
```

If Stata cannot be found, `/status` returns `needsConfiguration: true`. The
agent should ask the user where the Stata app/program is installed and run:

```bash
./skills/stata-all-in-one-skill/bin/macos-arm64/stata-ai-skill config set --stata-path "/Applications/StataMP.app"
./skills/stata-all-in-one-skill/bin/macos-arm64/stata-ai-skill serve
```

Windows example:

```powershell
.\skills\stata-all-in-one-skill\bin\windows\stata-ai-skill.exe config set --stata-path "C:\Program Files\Stata18\StataMP-64.exe"
.\skills\stata-all-in-one-skill\bin\windows\stata-ai-skill.exe serve
```

If Stata is found but the license file is missing, `/status` returns
`needsLicense: true`, `missing: "stata_license"`, and the expected `licensePath`.
Ask the user to confirm that Stata is licensed and that `stata.lic` or
`STATA.lic` exists in the Stata installation directory. The filename is checked
case-insensitively. Examples:

- macOS: `/Applications/StataNow/stata.lic`
- Windows: `C:\Program Files\Stata18\STATA.lic`

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
cargo run -p xtask -- dist
```
