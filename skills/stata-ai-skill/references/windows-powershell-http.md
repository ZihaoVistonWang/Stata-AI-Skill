# Windows PowerShell HTTP Requests

Read this reference when sending Stata AI Skill HTTP requests from Windows
PowerShell.

`curl.exe -d` with inline JSON is often mangled because PowerShell intercepts
the double quotes. Write the request JSON, not the do-file, to a temporary file
and send it with `--data-binary @file`.

PowerShell 5.1's `Out-File -Encoding utf8` writes a UTF-8 BOM that breaks JSON
parsing. Use `WriteAllText` with a BOM-free UTF-8 encoding:

```powershell
$body = '{"code":"display 2+2"}'
[System.IO.File]::WriteAllText("$env:TEMP\stata_body.json", $body, [System.Text.UTF8Encoding]::new($false))
curl.exe -s -X POST http://127.0.0.1:19522/execute `
  -H "Content-Type: application/json" `
  --data-binary "@$env:TEMP\stata_body.json"
```

`Invoke-RestMethod` in PowerShell 5.1 can have similar encoding issues; prefer
the file approach above.

For multiline Stata code, put a literal `\n` in the JSON string. The JSON
parser converts it to a newline:

```powershell
$body = '{"code":"sysuse auto, clear\nsummarize price mpg"}'
[System.IO.File]::WriteAllText("$env:TEMP\stata_body.json", $body, [System.Text.UTF8Encoding]::new($false))
curl.exe -s -X POST http://127.0.0.1:19522/execute `
  -H "Content-Type: application/json" `
  --data-binary "@$env:TEMP\stata_body.json"
```

