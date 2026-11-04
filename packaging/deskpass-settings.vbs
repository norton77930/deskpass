' Deskpass settings launcher (ticket 119): what the Start-menu shortcut runs through wscript.exe.
' Runs `node <this folder>\dist\index.js settings` with its console window hidden (window style 0)
' and returns at once; the settings server opens the browser and stops itself once no tab has used
' it for a while. Plain ASCII on purpose: wscript reads a BOM-less script in the ANSI code page.
Option Explicit
Dim shell, fso, here, command
Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
here = fso.GetParentFolderName(WScript.ScriptFullName)
command = "node """ & here & "\dist\index.js"" settings"
On Error Resume Next
shell.Run command, 0, False
If Err.Number <> 0 Then
  MsgBox "Deskpass could not start the settings page: node was not found (" & Err.Description & ")." & vbCrLf & _
    "Install Node.js 22 or newer, then try again.", vbExclamation, "Deskpass"
End If
