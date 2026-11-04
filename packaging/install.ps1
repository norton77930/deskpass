<#
.SYNOPSIS
  Installs Deskpass for the current user (no administrator rights needed).

.DESCRIPTION
  1. Checks for Node.js 22 or newer; if it is missing or too old, changes nothing and exits.
  2. Copies the program files to %LOCALAPPDATA%\Programs\Deskpass.
  3. If claude is on PATH, registers the MCP server "deskpass" with user scope; otherwise prints the line to run yourself.
     Either way it prints the MCP server definition for this install (command node, args = the installed dist\index.js)
     as a JSON "mcpServers" block and copies it to the clipboard, for CodeBuddy, Cursor, Claude Desktop, Codex CLI and
     any other MCP client (README.md lists where each one takes it). A clipboard that cannot be written only warns.
  4. Creates the Start menu shortcut "Deskpass Settings" (runs deskpass-settings.vbs through wscript, so no console window stays open).

  Messages are shown in English or Traditional Chinese. The environment variable DESKPASS_LANG ("en" or "zh-TW")
  decides; any other value is ignored. Without it, a Windows display language of zh-TW, zh-HK, zh-MO or any
  zh-Hant gives Traditional Chinese, and everything else gives English. The shortcut is named "Deskpass Settings"
  in English and "Deskpass 設定" in Traditional Chinese.

  Running it again is an upgrade: program files are overwritten, %LOCALAPPDATA%\deskpass (settings, grants, logs) is left alone.
  If an older helper is still running (your AI client is open), its exe is locked: it is renamed to
  deskpass-helper.old-<time>.exe, the new one is put in place, and the old copy is removed on the next install.
  Restart your AI client after an upgrade.

.PARAMETER SkipShellIntegration
  For tests only: does not register the MCP server and does not create the shortcut, it only copies the program files.
  Together with LOCALAPPDATA pointed at a temporary folder, this tests copying and upgrading without touching user settings.
  Do not use it for a normal install.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\install.ps1
#>
[CmdletBinding()]
param(
  [switch]$SkipShellIntegration
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ---- Messages: one table per language, same keys in both. {0}, {1}, ... are filled in by Msg. -----------
$Messages = @{
  "en" = @{
    shortcutName      = "Deskpass Settings"
    shortcutDesc      = "Open the Deskpass settings page"
    fileMissing       = "Cannot find {0}. Extract the whole zip, then run install.ps1 from the extracted folder."
    nodeMissing       = "Cannot find node. Deskpass needs Node.js 22 or newer: install it from https://nodejs.org/, reopen PowerShell, and run this again. Nothing was changed."
    nodeTooOld        = "Node is too old: {0} ({1}). Deskpass needs Node.js 22 or newer; upgrade it and run this again. Nothing was changed."
    nodeFound         = "Node {0} ({1})"
    localAppDataBad   = "LOCALAPPDATA is not set to a full path ({0}), so there is nowhere to install. Nothing was changed."
    runFromProgramDir = "Run install.ps1 from the extracted zip folder, not from the install folder {0}."
    reinstalling      = "Reinstalling Deskpass {0} (overwriting the existing one): {1}"
    upgrading         = "Upgrading Deskpass {0} -> {1}: {2}"
    installing        = "Installing Deskpass {0}: {1}"
    removingModules   = "Removing the old node_modules (the program now includes its dependencies)..."
    modulesInUse      = "node_modules is still in use and was not removed ({0}). This does not affect use; it will be cleaned up on the next install."
    cannotOverwrite   = "Cannot overwrite {0}: {1}. Close your AI client (Claude Code, CodeBuddy, Cursor, ...) and the Deskpass settings page, then run this again."
    helperRenamed     = "The old helper is still running; it was renamed to {0} (it will be deleted on the next install)."
    cannotReplace     = "Cannot replace {0}: {1}. Close your AI client (Claude Code, CodeBuddy, Cursor, ...), then run this again."
    filesCopied       = "Program files copied. Settings and logs are in {0}; install and upgrade never touch them."
    skipIntegration   = "(-SkipShellIntegration: the MCP server was not registered and no shortcut was created)"
    mcpRegistered     = "Registered the MCP server 'deskpass' in Claude Code (user scope)."
    mcpAddFailed      = "claude mcp add failed (exit code {0}): {1}"
    runThisLine       = "Run this line yourself:"
    mcpNoClaude       = "claude (Claude Code) was not found on PATH, so Deskpass was not registered with Claude Code. If you use another AI client, ignore this; Claude Code users can run this line once Claude Code is installed:"
    definitionIntro   = "MCP server definition for this install (for CodeBuddy, Cursor, Claude Desktop, Codex CLI and other MCP clients):"
    definitionCopied  = "Copied the definition above to the clipboard. Other AI clients (CodeBuddy, Cursor, Claude Desktop, Codex CLI) take it as described in README.md."
    definitionNotCopied = "Could not copy to the clipboard ({0}). Copy the definition above yourself; README.md says where each AI client takes it."
    shortcutCreated   = "Created the Start menu shortcut '{0}'."
    shortcutFailed    = "Cannot create the Start menu shortcut: {0}"
    shortcutFallback  = "You can run this in PowerShell instead: node `"{0}`" settings"
    doneReinstall     = "Reinstall complete. Restart your AI client (Claude Code, CodeBuddy, Cursor, ...) so it uses the new Deskpass."
    doneUpgrade       = "Upgrade complete. Restart your AI client (Claude Code, CodeBuddy, Cursor, ...) so it uses the new Deskpass."
    doneInstall       = "Install complete. Restart your AI client (Claude Code, CodeBuddy, Cursor, ...) and Deskpass is ready to use."
    uninstallHint     = "To uninstall: powershell -ExecutionPolicy Bypass -File `"{0}`""
  }
  "zh-TW" = @{
    shortcutName      = "Deskpass 設定"
    shortcutDesc      = "開啟 Deskpass 設定網頁"
    fileMissing       = "找不到 {0}。請把 zip 完整解壓縮,再從解壓縮出來的資料夾執行 install.ps1。"
    nodeMissing       = "找不到 node。Deskpass 需要 Node.js 22 以上:請到 https://nodejs.org/ 安裝,重開 PowerShell 後再執行一次。沒有做任何變更。"
    nodeTooOld        = "Node 版本太舊:{0}({1})。Deskpass 需要 Node.js 22 以上,請升級後再執行一次。沒有做任何變更。"
    nodeFound         = "Node {0}({1})"
    localAppDataBad   = "LOCALAPPDATA 沒有設定成完整路徑({0}),不知道要裝到哪裡。沒有做任何變更。"
    runFromProgramDir = "install.ps1 要從解壓縮出來的 zip 資料夾執行,不是從安裝資料夾 {0}。"
    reinstalling      = "重新安裝 Deskpass {0}(覆蓋現有的):{1}"
    upgrading         = "升級 Deskpass {0} → {1}:{2}"
    installing        = "安裝 Deskpass {0}:{1}"
    removingModules   = "移除舊版的 node_modules(現在的程式已包含相依套件)…"
    modulesInUse      = "node_modules 還在使用中,沒有移除({0})。不影響使用,下次安裝時再清。"
    cannotOverwrite   = "無法覆寫 {0}:{1}。請關閉你的 AI client(Claude Code、CodeBuddy、Cursor 等)和 Deskpass 設定網頁後再執行一次。"
    helperRenamed     = "舊版 helper 還在執行,已改名為 {0}(下次安裝時刪除)。"
    cannotReplace     = "無法替換 {0}:{1}。請關閉你的 AI client(Claude Code、CodeBuddy、Cursor 等)後再執行一次。"
    filesCopied       = "程式檔已複製。設定與日誌在 {0},安裝與升級都不會動它。"
    skipIntegration   = "(-SkipShellIntegration:沒有註冊 MCP server,也沒有建立捷徑)"
    mcpRegistered     = "已在 Claude Code 註冊 MCP server「deskpass」(user scope)。"
    mcpAddFailed      = "claude mcp add 失敗(結束代碼 {0}):{1}"
    runThisLine       = "請自己執行這一行:"
    mcpNoClaude       = "PATH 上找不到 claude(Claude Code),所以沒有在 Claude Code 註冊 Deskpass。如果你用的是別的 AI client,不用理會;Claude Code 的使用者裝好後可以執行這一行:"
    definitionIntro   = "這台電腦上的 MCP server 定義(給 CodeBuddy、Cursor、Claude Desktop、Codex CLI 等 MCP client):"
    definitionCopied  = "上面的定義已複製到剪貼簿。其他 AI client(CodeBuddy、Cursor、Claude Desktop、Codex CLI)照 README.md 的說明貼上即可。"
    definitionNotCopied = "無法複製到剪貼簿({0})。請自己複製上面的定義;各 AI client 要貼在哪裡,README.md 有說明。"
    shortcutCreated   = "已建立開始功能表捷徑「{0}」。"
    shortcutFailed    = "無法建立開始功能表捷徑:{0}"
    shortcutFallback  = "可以改在 PowerShell 執行:node `"{0}`" settings"
    doneReinstall     = "重新安裝完成。請重新啟動你的 AI client(Claude Code、CodeBuddy、Cursor 等),讓它改用新版的 Deskpass。"
    doneUpgrade       = "升級完成。請重新啟動你的 AI client(Claude Code、CodeBuddy、Cursor 等),讓它改用新版的 Deskpass。"
    doneInstall       = "安裝完成。請重新啟動你的 AI client(Claude Code、CodeBuddy、Cursor 等),就能使用 Deskpass。"
    uninstallHint     = "解除安裝:powershell -ExecutionPolicy Bypass -File `"{0}`""
  }
}

# The language rule below mirrors src/ui-language.ts (decideUiLanguage and windowsUiLanguages) exactly; change them together.
# DESKPASS_LANG is used when it is exactly "en" or "zh-TW" (case-sensitive) and any other value is ignored. Otherwise the
# Windows UI languages decide, preferred first: HKCU\Control Panel\Desktop\PreferredUILanguages (REG_MULTI_SZ, present once
# the user picked a display language), else HKCU\Control Panel\Desktop\MuiCached\MachinePreferredUILanguages. Only the first
# entry counts: zh-TW, zh-HK, zh-MO and zh-Hant / zh-Hant-* give Traditional Chinese; anything else, missing or unreadable gives English.
function Read-RegistryList([string]$key, [string]$name) {
  try {
    $item = Get-ItemProperty -LiteralPath $key -Name $name -ErrorAction SilentlyContinue
    if ($null -eq $item) { return @() }
    return @($item.$name | ForEach-Object { "$_".Trim() } | Where-Object { $_.Length -gt 0 })
  } catch {
    return @()
  }
}

# Whether a BCP 47 tag names Traditional Chinese: language zh, then a four-letter script decides (Hant yes, any other no);
# without a script, a region of TW, HK or MO means yes.
function Test-TraditionalChinese([string]$tag) {
  $parts = @($tag.Trim().Replace("_", "-").ToLowerInvariant().Split("-"))
  if ($parts[0] -ne "zh") { return $false }
  $rest = @($parts | Select-Object -Skip 1)
  $scriptTag = @($rest | Where-Object { $_.Length -eq 4 } | Select-Object -First 1)
  if ($scriptTag.Count -gt 0) { return ($scriptTag[0] -eq "hant") }
  return (@($rest | Where-Object { $_ -eq "tw" -or $_ -eq "hk" -or $_ -eq "mo" }).Count -gt 0)
}

function Get-DeskpassLanguage {
  if ($env:DESKPASS_LANG -ceq "en" -or $env:DESKPASS_LANG -ceq "zh-TW") { return $env:DESKPASS_LANG }
  $languages = @(Read-RegistryList "HKCU:\Control Panel\Desktop" "PreferredUILanguages")
  if ($languages.Count -eq 0) {
    $languages = @(Read-RegistryList "HKCU:\Control Panel\Desktop\MuiCached" "MachinePreferredUILanguages")
  }
  if ($languages.Count -gt 0 -and (Test-TraditionalChinese $languages[0])) { return "zh-TW" }
  return "en"
}
$Lang = Get-DeskpassLanguage

# Looks up a message and fills in {0}, {1}, ... A key missing from the table shows as [missing:key] instead of falling back.
function Msg([string]$key) {
  $table = $Messages[$Lang]
  if (-not $table.ContainsKey($key)) { return "[missing:$key]" }
  return ([string]$table[$key]) -f $args
}

function Fail([string]$message) {
  Write-Host $message -ForegroundColor Red
  exit 1
}

# When a native program writes to stderr, Windows PowerShell 5.1 in Stop mode throws it as an error; only the exit code matters here.
# Each stderr line becomes an ErrorRecord; only its message is kept, otherwise Out-String would print the red At/CategoryInfo format (ticket 136).
function Invoke-Native([scriptblock]$command) {
  $previous = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $output = & $command 2>&1 | ForEach-Object { if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { $_ } } | Out-String
    return @{ Code = $LASTEXITCODE; Output = $output.Trim() }
  } finally {
    $ErrorActionPreference = $previous
  }
}

# ---- Files of the install package (relative to the folder this script is in) ---------------------------
$sourceDir = $PSScriptRoot
$required = @(
  "dist\index.js", "dist\deskpass-helper.exe", "dist\helper-build.json",
  "package.json", "uninstall.ps1", "deskpass-settings.vbs", "README.md", "VERSION"
)
foreach ($file in $required) {
  if (-not (Test-Path -LiteralPath (Join-Path $sourceDir $file))) {
    Fail (Msg "fileMissing" $file)
  }
}
$version = (Get-Content -LiteralPath (Join-Path $sourceDir "VERSION") -Raw).Trim()

# ---- Precondition: Node 22 or newer (checked before anything is changed) -------------------------------
$nodeCommand = Get-Command node -ErrorAction SilentlyContinue
if (-not $nodeCommand) {
  Fail (Msg "nodeMissing")
}
$nodeVersion = (Invoke-Native { node -v }).Output   # for example v24.13.0
$nodeMajor = 0
if ($nodeVersion -match '^v?(\d+)\.') { $nodeMajor = [int]$Matches[1] }
if ($nodeMajor -lt 22) {
  Fail (Msg "nodeTooOld" $nodeVersion $nodeCommand.Source)
}
Write-Host (Msg "nodeFound" $nodeVersion $nodeCommand.Source)

# ---- Destination ---------------------------------------------------------------------------------------
if (-not $env:LOCALAPPDATA -or -not [System.IO.Path]::IsPathRooted($env:LOCALAPPDATA)) {
  Fail (Msg "localAppDataBad" $env:LOCALAPPDATA)
}
$programDir = Join-Path $env:LOCALAPPDATA "Programs\Deskpass"
$dataDir = Join-Path $env:LOCALAPPDATA "deskpass"
$serverPath = Join-Path $programDir "dist\index.js"
$fullSource = [System.IO.Path]::GetFullPath($sourceDir).TrimEnd('\')
if ($fullSource -ieq [System.IO.Path]::GetFullPath($programDir).TrimEnd('\')) {
  Fail (Msg "runFromProgramDir" $programDir)
}

$upgrade = Test-Path -LiteralPath (Join-Path $programDir "VERSION")
$reinstall = $false
if ($upgrade) {
  $oldVersion = (Get-Content -LiteralPath (Join-Path $programDir "VERSION") -Raw).Trim()
  if ($oldVersion -eq $version) {
    $reinstall = $true
    Write-Host (Msg "reinstalling" $version $programDir)
  } else {
    Write-Host (Msg "upgrading" $oldVersion $version $programDir)
  }
} else {
  Write-Host (Msg "installing" $version $programDir)
}
New-Item -ItemType Directory -Path (Join-Path $programDir "dist") -Force | Out-Null

# ---- Old node_modules: dist\index.js now bundles every dependency, so it is no longer needed ---------
# It is renamed aside first and then deleted: when the folder is locked (an old version is still running) the rename may still work.
# If it cannot be deleted the install is unaffected; it is cleaned up on the next install.
$modulesDir = Join-Path $programDir "node_modules"
if (Test-Path -LiteralPath $modulesDir) {
  Write-Host (Msg "removingModules")
  try {
    Rename-Item -LiteralPath $modulesDir -NewName ("node_modules.old-" + [DateTime]::Now.Ticks)
  } catch {
    Write-Host (Msg "modulesInUse" $_.Exception.Message) -ForegroundColor Yellow
  }
}
Get-ChildItem -LiteralPath $programDir -Directory -Filter "node_modules.old-*" -ErrorAction SilentlyContinue |
  ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }

# ---- dist: files the old version had and the new one does not are deleted; a locked helper is renamed aside ----
$targetDist = Join-Path $programDir "dist"
$sourceDist = Join-Path $sourceDir "dist"
$newNames = @(Get-ChildItem -LiteralPath $sourceDist -File | ForEach-Object { $_.Name })
foreach ($old in @(Get-ChildItem -LiteralPath $targetDist -File)) {
  if ($old.Name -like "deskpass-helper.old-*.exe" -or $newNames -notcontains $old.Name) {
    # An old helper that is still running cannot be deleted; the next install tries again.
    Remove-Item -LiteralPath $old.FullName -Force -ErrorAction SilentlyContinue
  }
}
foreach ($name in $newNames) {
  $from = Join-Path $sourceDist $name
  $to = Join-Path $targetDist $name
  try {
    Copy-Item -LiteralPath $from -Destination $to -Force
  } catch {
    if ($name -ne "deskpass-helper.exe") {
      Fail (Msg "cannotOverwrite" $to $_.Exception.Message)
    }
    # Windows does not let a running exe be overwritten but does allow renaming it: the old one is renamed aside and the new one takes its place.
    $aside = "deskpass-helper.old-" + [DateTime]::Now.Ticks + ".exe"
    try {
      Rename-Item -LiteralPath $to -NewName $aside
      Copy-Item -LiteralPath $from -Destination $to -Force
      Write-Host (Msg "helperRenamed" $aside) -ForegroundColor Yellow
    } catch {
      Fail (Msg "cannotReplace" $to $_.Exception.Message)
    }
  }
}

foreach ($name in @("package.json", "uninstall.ps1", "deskpass-settings.vbs", "README.md", "VERSION")) {
  Copy-Item -LiteralPath (Join-Path $sourceDir $name) -Destination (Join-Path $programDir $name) -Force
}
# A zip from mail or a browser marks every file as downloaded; the shortcut's script would then hit SmartScreen.
Get-ChildItem -LiteralPath $programDir -Recurse -File | Unblock-File
Write-Host (Msg "filesCopied" $dataDir)

$claudeLine = "claude mcp add --scope user --transport stdio deskpass -- node `"$serverPath`""

if ($SkipShellIntegration) {
  Write-Host (Msg "skipIntegration") -ForegroundColor Yellow
  exit 0
}

# ---- MCP registration ----------------------------------------------------------------------------------
Write-Host ""
if (Get-Command claude -ErrorAction SilentlyContinue) {
  # On a reinstall the existing user-scope registration is removed first (remove fails when there is none, which is fine), then added.
  Invoke-Native { claude mcp remove --scope user deskpass } | Out-Null
  $result = Invoke-Native { claude mcp add --scope user --transport stdio deskpass '--' node "$serverPath" }
  if ($result.Code -eq 0) {
    Write-Host (Msg "mcpRegistered") -ForegroundColor Green
  } else {
    Write-Host (Msg "mcpAddFailed" $result.Code $result.Output) -ForegroundColor Yellow
    Write-Host (Msg "runThisLine")
    Write-Host "  $claudeLine"
  }
} else {
  Write-Host (Msg "mcpNoClaude") -ForegroundColor Yellow
  Write-Host "  $claudeLine"
}

# ---- MCP server definition for every other client (printed and copied whether or not claude was found) ----
# The same shape for CodeBuddy, Cursor and Claude Desktop (an "mcpServers" block); README.md says where each one takes it
# and shows the Codex CLI TOML. Backslashes in the path are doubled for JSON. Built by hand rather than with ConvertTo-Json,
# whose layout differs between PowerShell versions.
$serverPathJson = $serverPath.Replace("\", "\\")
$definition = @(
  '{',
  '  "mcpServers": {',
  '    "deskpass": {',
  '      "command": "node",',
  ('      "args": ["{0}"]' -f $serverPathJson),
  '    }',
  '  }',
  '}'
) -join "`r`n"
Write-Host ""
Write-Host (Msg "definitionIntro")
Write-Host $definition
try {
  Set-Clipboard -Value $definition
  Write-Host (Msg "definitionCopied") -ForegroundColor Green
} catch {
  # No clipboard (for example no desktop session) must not fail the install.
  Write-Host (Msg "definitionNotCopied" $_.Exception.Message) -ForegroundColor Yellow
}

# ---- Start menu shortcut -------------------------------------------------------------------------------
# The shortcut runs wscript.exe (built into Windows, no console window), which makes deskpass-settings.vbs start
# node ... settings with a hidden window; the settings server runs in the background and stops itself some time after the tab is closed.
# The shortcut's name follows the language; the one the other language would have made is removed so there is only one.
$startMenuDir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
$shortcutName = Msg "shortcutName"
$shortcutPath = Join-Path $startMenuDir "$shortcutName.lnk"
try {
  New-Item -ItemType Directory -Path $startMenuDir -Force | Out-Null
  $shell = New-Object -ComObject WScript.Shell
  $shortcut = $shell.CreateShortcut($shortcutPath)
  $shortcut.TargetPath = Join-Path $env:SystemRoot "System32\wscript.exe"
  $shortcut.Arguments = "`"$(Join-Path $programDir 'deskpass-settings.vbs')`""
  $shortcut.WorkingDirectory = $programDir
  $shortcut.Description = Msg "shortcutDesc"
  $shortcut.Save()
  foreach ($language in $Messages.Keys) {
    $otherPath = Join-Path $startMenuDir ("$($Messages[$language].shortcutName).lnk")
    if ($otherPath -ine $shortcutPath) {
      Remove-Item -LiteralPath $otherPath -Force -ErrorAction SilentlyContinue
    }
  }
  Write-Host (Msg "shortcutCreated" $shortcutName) -ForegroundColor Green
} catch {
  Write-Host (Msg "shortcutFailed" $_.Exception.Message) -ForegroundColor Yellow
  Write-Host (Msg "shortcutFallback" $serverPath)
}

Write-Host ""
if ($reinstall) {
  Write-Host (Msg "doneReinstall") -ForegroundColor Green
} elseif ($upgrade) {
  Write-Host (Msg "doneUpgrade") -ForegroundColor Green
} else {
  Write-Host (Msg "doneInstall") -ForegroundColor Green
}
Write-Host (Msg "uninstallHint" (Join-Path $programDir 'uninstall.ps1'))
exit 0
