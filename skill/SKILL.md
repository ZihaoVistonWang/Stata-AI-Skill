---
name: stata-all-in-one-skill
version: 202606130001
description: Run Stata code through the native Stata AI Skill background service at http://127.0.0.1:19522. Use when the user asks to run Stata commands, regressions, summaries, tests, or .do/.dta workflows. No VS Code, Node.js, or Python runtime is required on the user side.
compatibility: Requires Apple Silicon macOS or Windows, the native stata-ai-skill executable, and a locally installed/licensed Stata. Intel Mac is not supported. If automatic Stata discovery fails, ask the user where the Stata app/program is installed and configure it with the executable CLI.
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
      stata-ai-skill            (legacy fallback)
    macos-arm64/
      stata-ai-skill            (Apple Silicon)
    windows/
      stata-ai-skill.exe          (x64)
    windows-arm64/
      stata-ai-skill.exe          (ARM64)
```

Resolution order:

1. If `STATA_AI_SKILL_BIN` is set, use that exact executable path.
2. macOS Apple Silicon: use `<this-skill-directory>/bin/macos-arm64/stata-ai-skill`.
3. macOS Intel (`x86_64`): stop and tell the user this skill does not support Intel Mac.
4. macOS Apple Silicon fallback: use `<this-skill-directory>/bin/macos/stata-ai-skill`.
5. Windows x64: use `<this-skill-directory>\bin\windows\stata-ai-skill.exe`.
6. Windows ARM64: use `<this-skill-directory>\bin\windows-arm64\stata-ai-skill.exe`.
7. Fallback only if packaged binary is missing on a supported platform: use `stata-ai-skill` from PATH.

To detect macOS architecture:

```bash
case "$(uname -m)" in
  arm64) exe="./bin/macos-arm64/stata-ai-skill" ;;
  x86_64)
    echo "Stata AI Skill does not support Intel Mac."
    exit 1
    ;;
  *) exe="./bin/macos/stata-ai-skill" ;;
esac
```

To detect Windows architecture from PowerShell:

```powershell
if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
    $exe = ".\bin\windows-arm64\stata-ai-skill.exe"
} else {
    $exe = ".\bin\windows\stata-ai-skill.exe"
}
```

For development builds, refresh the packaged executable with:

```bash
cargo run -p xtask -- dist
```

When writing commands below, replace `stata-ai-skill` with the resolved
executable path. Examples:

```bash
# macOS, from the skill directory
./bin/macos-arm64/stata-ai-skill serve
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

**Important:** After running `config set`, the running service does NOT pick up
the new configuration. You must shut down and restart the service:

```bash
curl -s -X POST http://127.0.0.1:19522/shutdown
# then start again:
./bin/macos/stata-ai-skill serve
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

### PowerShell Curl Notes

In PowerShell, `curl.exe -d` with a JSON body containing double quotes is
often mangled because PowerShell intercepts the quotes before they reach curl.
The double quotes inside `-d '{"code":"..."}'` get stripped or misinterpreted.

**Do NOT use inline JSON with curl.exe in PowerShell.** Instead, always write
the JSON body to a temporary file and use `--data-binary @file`:

```powershell
# Correct approach — write JSON to a temp file first
$body = '{"code":"display 2+2"}'
$body | Out-File -FilePath "$env:TEMP\stata_body.json" -Encoding utf8 -NoNewline
curl.exe -s -X POST http://127.0.0.1:19522/execute `
  -H "Content-Type: application/json" `
  --data-binary "@$env:TEMP\stata_body.json"
```

This also avoids encoding issues with `Invoke-RestMethod` in PowerShell 5.1.

For multi-line Stata code, use a literal `\n` (backslash + n) inside the JSON
string — the JSON parser will convert it to an actual newline:

```powershell
$body = '{"code":"sysuse auto, clear\nsummarize price mpg"}'
$body | Out-File -FilePath "$env:TEMP\stata_body.json" -Encoding utf8 -NoNewline
curl.exe -s -X POST http://127.0.0.1:19522/execute `
  -H "Content-Type: application/json" `
  --data-binary "@$env:TEMP\stata_body.json"
```

On macOS/Linux (bash/zsh), inline JSON works as expected:

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

Response for a timed-out execution returns HTTP 408 with:

```json
{"success":false,"returnCode":-1,"output":"Execution timed out after 3s","error":"Execution timed out after 3s","graphs":[]}
```

### Session Recovery After Timeout

After a timeout kills execution, the Stata session may briefly be in a
recovering state. The **first** execution immediately after a timeout may
return a stale timeout error even though Stata completed. Always verify by
running a trivial command after timeout:

```bash
curl -s -X POST http://127.0.0.1:19522/execute \
  -H "Content-Type: application/json" \
  -d '{"code":"display 123"}'
```

If it returns `success: true`, the session is healthy. If it returns another
timeout error, check `/status` and retry once more. The service itself does
not crash on timeout.

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
