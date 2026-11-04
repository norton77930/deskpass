<#
.SYNOPSIS
  Uninstalls Deskpass for the current user.

.DESCRIPTION
  Removes the Claude Code MCP registration "deskpass" (user scope, when claude is on PATH), the Start menu
  shortcut "Deskpass Settings" (or "Deskpass 設定", whichever language created it) and the program files in
  %LOCALAPPDATA%\Programs\Deskpass.
  %LOCALAPPDATA%\deskpass (settings, grants, logs) is kept unless -RemoveData is given.
  The folder of the old name, %LOCALAPPDATA%\computer-use-mcp, is never touched.
  Before deleting, every path is checked to be exactly the expected one; if not, it stops.

  Messages are shown in English or Traditional Chinese by the same rule as install.ps1: the environment variable
  DESKPASS_LANG ("en" or "zh-TW") wins and any other value is ignored; otherwise a Windows display language of
  zh-TW, zh-HK, zh-MO or any zh-Hant gives Traditional Chinese, and everything else gives English.

.PARAMETER RemoveData
  Also deletes %LOCALAPPDATA%\deskpass (settings, grants and logs are gone).

.PARAMETER SkipShellIntegration
  For tests only: does not remove the MCP registration or the shortcut, it only handles the program files and folders.
  Do not use it for a normal uninstall.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\Deskpass\uninstall.ps1"
  powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\Deskpass\uninstall.ps1" -RemoveData
#>
[CmdletBinding()]
param(
  [switch]$RemoveData,
  [switch]$SkipShellIntegration
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ---- Messages: one table per language, same keys in both. {0}, {1}, ... are filled in by Msg. -----------
$Messages = @{
  "en" = @{
    shortcutName     = "Deskpass Settings"
    localAppDataBad  = "LOCALAPPDATA is not an existing full path ({0}); nothing is deleted, to be safe."
    unexpectedPath   = "The path to delete, {0}, is not the expected {1}\{2}; stopping, to be safe."
    mcpRemoved       = "Removed the MCP registration 'deskpass' from Claude Code (user scope)."
    mcpNotThere      = "The MCP registration 'deskpass' was not in Claude Code, so there is nothing to remove."
    mcpRemoveNothing = "claude mcp remove did not remove any registration (there may not have been one): {0}"
    mcpNoClaude      = "claude was not found on PATH. If it was ever registered, run this yourself: claude mcp remove --scope user deskpass"
    shortcutRemoved  = "Deleted the Start menu shortcut '{0}'."
    shortcutOtherRemoved = "Deleted the Start menu shortcut made in the other language."
    programRemoved   = "Deleted the program files {0}."
    programLocked    = "Some program files could not be deleted ({0}); most likely your AI client (Claude Code, CodeBuddy, Cursor, ...) is still using Deskpass."
    programNotFound  = "No program files found at {0} (they may already be deleted)."
    dataRemoved      = "Deleted the settings and logs {0}."
    dataLocked       = "Some data could not be deleted ({0})."
    dataKept         = "Kept the settings and logs {0} (add -RemoveData to delete them too)."
    notFullyRemoved  = "Not everything was deleted: {0}. Close your AI client (Claude Code, CodeBuddy, Cursor, ...) and the Deskpass settings page, then run uninstall.ps1 again."
    listSeparator    = ", "
    done             = "Uninstall complete."
  }
  "zh-TW" = @{
    shortcutName     = "Deskpass 設定"
    localAppDataBad  = "LOCALAPPDATA 不是存在的完整路徑({0}),為了安全不刪任何東西。"
    unexpectedPath   = "要刪除的路徑 {0} 不是預期的 {1}\{2},為了安全停止。"
    mcpRemoved       = "已移除 Claude Code 的 MCP 註冊「deskpass」(user scope)。"
    mcpNotThere      = "Claude Code 的 MCP 註冊「deskpass」本來就不在,不用移除。"
    mcpRemoveNothing = "claude mcp remove 沒有移除任何註冊(可能本來就沒有):{0}"
    mcpNoClaude      = "PATH 上找不到 claude。如果曾經註冊過,請自己執行:claude mcp remove --scope user deskpass"
    shortcutRemoved  = "已刪除開始功能表捷徑「{0}」。"
    shortcutOtherRemoved = "已刪除另一種語言建立的開始功能表捷徑。"
    programRemoved   = "已刪除程式檔 {0}。"
    programLocked    = "有些程式檔刪不掉({0}):多半是你的 AI client(Claude Code、CodeBuddy、Cursor 等)還在使用 Deskpass。"
    programNotFound  = "沒有找到程式檔 {0}(可能已經刪除)。"
    dataRemoved      = "已刪除設定與日誌 {0}。"
    dataLocked       = "有些資料刪不掉({0})。"
    dataKept         = "保留設定與日誌 {0}(要一併刪除請加 -RemoveData)。"
    notFullyRemoved  = "還沒刪乾淨:{0}。請關閉你的 AI client(Claude Code、CodeBuddy、Cursor 等)和 Deskpass 設定網頁後,再執行一次 uninstall.ps1。"
    listSeparator    = "、"
    done             = "解除安裝完成。"
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

if (-not $env:LOCALAPPDATA -or -not [System.IO.Path]::IsPathRooted($env:LOCALAPPDATA) -or -not (Test-Path -LiteralPath $env:LOCALAPPDATA)) {
  Fail (Msg "localAppDataBad" $env:LOCALAPPDATA)
}
$localAppData = [System.IO.Path]::GetFullPath($env:LOCALAPPDATA).TrimEnd('\')
$programDir = Join-Path $localAppData "Programs\Deskpass"
$dataDir = Join-Path $localAppData "deskpass"
$serverPath = Join-Path $programDir "dist\index.js"

# Check before deleting: the path must be exactly the expected one, and never the folder of the old name.
function Assert-Expected([string]$path, [string]$expectedParent, [string]$expectedLeaf) {
  $full = [System.IO.Path]::GetFullPath($path).TrimEnd('\')
  $parent = [System.IO.Path]::GetFullPath($expectedParent).TrimEnd('\')
  if ((Split-Path -Parent $full) -ine $parent -or (Split-Path -Leaf $full) -ine $expectedLeaf -or $full -match 'computer-use-mcp') {
    Fail (Msg "unexpectedPath" $full $parent $expectedLeaf)
  }
  return $full
}

# This script may sit inside the folder it deletes: leave it first, otherwise the folder cannot be deleted because it is the current directory.
Set-Location -LiteralPath $localAppData
[System.Environment]::CurrentDirectory = $localAppData

$remaining = @()

# ---- MCP registration and shortcut ---------------------------------------------------------------------
if (-not $SkipShellIntegration) {
  if (Get-Command claude -ErrorAction SilentlyContinue) {
    $result = Invoke-Native { claude mcp remove --scope user deskpass }
    if ($result.Code -eq 0) {
      Write-Host (Msg "mcpRemoved")
    } elseif ($result.Output -match 'No MCP server named') {
      Write-Host (Msg "mcpNotThere") -ForegroundColor Yellow
    } else {
      Write-Host (Msg "mcpRemoveNothing" $result.Output) -ForegroundColor Yellow
    }
  } else {
    Write-Host (Msg "mcpNoClaude") -ForegroundColor Yellow
  }

  $startMenuDir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
  # The shortcut of either language is removed, whichever language installed it.
  foreach ($language in $Messages.Keys) {
    $shortcutName = $Messages[$language].shortcutName
    $shortcutPath = Join-Path $startMenuDir "$shortcutName.lnk"
    if (Test-Path -LiteralPath $shortcutPath) {
      $shortcutPath = Assert-Expected $shortcutPath $startMenuDir "$shortcutName.lnk"
      Remove-Item -LiteralPath $shortcutPath -Force
      # The other language's name is not shown: it would put its script (CJK) into English output.
      if ($language -eq $Lang) { Write-Host (Msg "shortcutRemoved" $shortcutName) } else { Write-Host (Msg "shortcutOtherRemoved") }
    }
  }
}

# ---- Program files -------------------------------------------------------------------------------------
if (Test-Path -LiteralPath $programDir) {
  $programDir = Assert-Expected $programDir (Join-Path $localAppData "Programs") "Deskpass"
  try {
    Remove-Item -LiteralPath $programDir -Recurse -Force -ErrorAction Stop
    Write-Host (Msg "programRemoved" $programDir)
  } catch {
    $remaining += $programDir
    Write-Host (Msg "programLocked" $_.Exception.Message) -ForegroundColor Yellow
  }
} else {
  Write-Host (Msg "programNotFound" $programDir)
}

# ---- Data (deleted only with -RemoveData) --------------------------------------------------------------
if ($RemoveData) {
  if (Test-Path -LiteralPath $dataDir) {
    $dataDir = Assert-Expected $dataDir $localAppData "deskpass"
    try {
      Remove-Item -LiteralPath $dataDir -Recurse -Force -ErrorAction Stop
      Write-Host (Msg "dataRemoved" $dataDir)
    } catch {
      $remaining += $dataDir
      Write-Host (Msg "dataLocked" $_.Exception.Message) -ForegroundColor Yellow
    }
  }
} elseif (Test-Path -LiteralPath $dataDir) {
  Write-Host (Msg "dataKept" $dataDir)
}

Write-Host ""
if ($remaining.Count -gt 0) {
  Write-Host (Msg "notFullyRemoved" ($remaining -join (Msg "listSeparator"))) -ForegroundColor Yellow
  exit 1
}
Write-Host (Msg "done") -ForegroundColor Green
exit 0
