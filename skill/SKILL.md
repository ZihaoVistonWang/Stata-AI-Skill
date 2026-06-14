---
name: stata-all-in-one-skill
version: 202606130001
description: Run Stata code through the native Stata AI Skill background service at http://127.0.0.1:19522. Use when the user asks to run Stata commands, regressions, summaries, tests, or .do/.dta workflows. No VS Code, Node.js, or Python runtime is required on the user side.
compatibility: Requires macOS or Windows, the native stata-ai-skill executable, and a locally installed/licensed Stata. If automatic Stata discovery fails, ask the user where the Stata app/program is installed and configure it with the executable CLI.
---

# Stata AI Skill

This native service is extracted from
[ZihaoVistonWang/stata-all-in-one](https://github.com/ZihaoVistonWang/stata-all-in-one)
and preserves the AI Skill HTTP workflow without requiring VS Code at runtime.

Use the native localhost service at `http://127.0.0.1:19522` to run Stata.
Do not import internal modules. The stable interface is HTTP.

## Locate The Executable

Agents must resolve the executable from this skill directory before using PATH
or build outputs. Do not require the user to know Cargo's `target/release`
directory.

Expected packaged layout:

```text
stata-all-in-one-skill/
  SKILL.md
  bin/
    macos/
      stata-ai-skill
    windows/
      stata-ai-skill.exe
```

Resolution order:

1. If `STATA_AI_SKILL_BIN` is set, use that exact executable path.
2. macOS: use `<this-skill-directory>/bin/macos/stata-ai-skill`.
3. Windows: use `<this-skill-directory>\bin\windows\stata-ai-skill.exe`.
4. Fallback only if packaged binary is missing: use `stata-ai-skill` from PATH.

For development builds, refresh the packaged executable with:

```bash
cargo run -p xtask -- dist
```

When writing commands below, replace `stata-ai-skill` with the resolved
executable path. Examples:

```bash
# macOS, from the skill directory
./bin/macos/stata-ai-skill serve
```

```powershell
# Windows, from the skill directory
.\bin\windows\stata-ai-skill.exe serve
```

## Agent Workflow

1. Check whether the service is running:

```bash
curl -s --connect-timeout 2 http://127.0.0.1:19522/status 2>/dev/null || echo "OFFLINE"
```

2. If offline, start the native executable:

```bash
stata-ai-skill serve
```

Use the resolved executable path from "Locate The Executable"; the bare command
above is only shorthand.

3. If `/status` returns `needsConfiguration: true`, ask the user where the Stata
app/program is installed. Avoid saying only "Stata path" because some users do
not know what a path is. Then configure it:

```bash
stata-ai-skill config set --stata-path "<USER_PROVIDED_STATA_PATH>"
```

Again, use the resolved executable path. For example:

```bash
./bin/macos/stata-ai-skill config set --stata-path "/Applications/StataNow/StataMP.app"
```

```powershell
.\bin\windows\stata-ai-skill.exe config set --stata-path "C:\Program Files\Stata18"
```

User-facing wording:

- macOS: "Open Finder > Applications, find the Stata app icon, and tell me its
  name/location. You can also drag the Stata app into Terminal to reveal a path
  like `/Applications/StataNow/StataMP.app`."
- Windows: "Find Stata in the Start menu or under `C:\Program Files\Stata...`.
  The program may be named `StataMP-64.exe`, `StataSE-64.exe`, or similar."

Accepted paths include the Stata app/exe, install directory, or shared library:

- macOS: `/Applications/StataMP.app`
- macOS: `/Applications/StataNow/StataMP.app`
- macOS: `/Applications/StataMP.app/Contents/MacOS/libstata-mp.dylib`
- Windows: `C:\Program Files\Stata18`
- Windows: `C:\Program Files\Stata18\StataMP-64.exe`
- Windows: `C:\Program Files\Stata18\mp-64.dll`

4. Recheck `/status`. If `sessionActive: true`, call `/execute`.

If `/status` returns `needsLicense: true` or `missing: "stata_license"`, Stata
was found but the license file was not found. Tell the user:

"Stata is installed, but the service cannot find the Stata license file
`stata.lic` / `STATA.lic`. Please open Stata once to confirm it is licensed, or
check that the license file exists in the Stata installation folder."

Common license locations:

- macOS: `/Applications/StataNow/stata.lic`
- Windows: `C:\Program Files\Stata18\STATA.lic`

## Execute

```bash
curl -s -X POST http://127.0.0.1:19522/execute \
  -H "Content-Type: application/json" \
  -d '{"code":"display 2+2"}'
```

Response:

```json
{
  "success": true,
  "returnCode": 0,
  "output": "4",
  "error": "",
  "graphs": []
}
```

For long commands, set `timeout` in seconds:

```bash
curl -s -X POST http://127.0.0.1:19522/execute \
  -H "Content-Type: application/json" \
  -d '{"code":"bootstrap r(mean), reps(1000): summarize price", "timeout": 300}'
```

## Break And Shutdown

Interrupt the current Stata execution:

```bash
curl -s -X POST http://127.0.0.1:19522/break
```

Close the background service:

```bash
curl -s -X POST http://127.0.0.1:19522/shutdown
```

## Files

The service uses system directories only. It does not create `.stata-all-in-one/`
in the current repository or working directory. Temporary `.do` files are unique
and deleted after execution. Graphs are exported as SVG to the service graph
directory and returned as absolute paths in `graphs`.
