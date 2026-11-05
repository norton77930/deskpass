# Deskpass 使用手冊(繁體中文)

這份手冊給「使用」Deskpass 的人:怎麼安裝、AI 第一次要操作 App 時會發生什麼、怎麼設定與停止、遇到問題怎麼辦。專案簡介見 [README](../README.md)([繁中版](../README.zh-TW.md));要自己建置或修改程式,見 [開發文件](development.md) 與 [CONTRIBUTING](../CONTRIBUTING.md);發現安全問題,請照 [SECURITY](../SECURITY.md) 回報,不要開公開 issue。版本變更見 [CHANGELOG](../CHANGELOG.md)。

目錄:

- [Deskpass 是什麼](#deskpass-是什麼)
- [安裝](#安裝)
- [介面語言](#介面語言)
- [各 MCP client 的設定方式](#各-mcp-client-的設定方式)
- [第一次 AI 要操作 App 時會發生什麼](#第一次-ai-要操作-app-時會發生什麼)
- [四種狀態與預設](#四種狀態與預設)
- [怎麼打開設定網頁](#怎麼打開設定網頁)
- [放寬限制為什麼要按確認視窗](#放寬限制為什麼要按確認視窗)
- [背景操作](#背景操作)
- [緊急停止](#緊急停止)
- [未簽章的 helper 與 SHA-256](#未簽章的-helper-與-sha-256)
- [SmartScreen 或公司資安擋下時怎麼辦](#smartscreen-或公司資安擋下時怎麼辦)
- [解除安裝](#解除安裝)
- [已知限制](#已知限制)
- [從開發版升級](#從開發版升級)

## Deskpass 是什麼

Deskpass 讓 AI(例如 Claude Code)操作你 Windows 電腦上的 App:看視窗、截圖、點擊、打字、按鍵、捲動、拖曳,以及開啟 App。

AI 不是想操作什麼就操作什麼:

- 每個 App 第一次被操作之前,都會先跳出視窗問你,你按允許才會動。
- 終端機、登錄編輯程式、工作管理員這類 App 一律不讓 AI 碰;瀏覽器、郵件這類 App 每次都要問你。
- AI 操作時,那個視窗外面會有一圈邊框,寫著「AI 正在操作 · 按 Ctrl+Alt+Esc 停止」。你隨時按鍵盤上的 **Ctrl+Alt+Esc**,AI 就停下來。AI 把視窗帶到前面操作的時候,單按 Esc 也可以(見「緊急停止」)。
- AI 預設在背景操作 App:不搶走你的畫面和鍵盤,你可以一邊用電腦,一邊讓它在另一個 App 做事(見「背景操作」)。
- 你可以在設定網頁上看到 AI 能操作哪些 App、它做過什麼,也可以把某個 App 封鎖起來。

Deskpass 不會記下你或 AI 輸入的文字、剪貼簿內容或截圖。

## 安裝

你需要:

- Windows 10 2004 以上,或 Windows 11。
- **Node.js 22 以上**。還沒裝或版本太舊,請到 https://nodejs.org/ 下載安裝,裝好後重開 PowerShell。可以用 `node -v` 看版本。
- 一個支援 MCP 的 AI client。Claude Code(`claude` 指令)最簡單,安裝腳本會自動接好;CodeBuddy、Cursor、Claude Desktop、Codex CLI 等其他 client 也能用,見「各 MCP client 的設定方式」。沒有 Claude Code 也能先裝 Deskpass,見下面第 4 步。

步驟:

1. 到 Deskpass 的 GitHub Releases 頁面(https://github.com/norton77930/deskpass/releases)下載 `deskpass-0.1.0.zip`(0.1.0 是版本號,新版的檔名會換成新的版本號)。建議先照「未簽章的 helper 與 SHA-256」核對雜湊值。然後在 zip 上按右鍵 →「全部解壓縮」,解壓縮到任何一個資料夾。
2. 打開解壓縮出來的資料夾,確認裡面有 `install.ps1`、`uninstall.ps1`、`README.md`、`VERSION`、`deskpass-settings.vbs`、`package.json`,以及 `dist` 資料夾(裡面是 `index.js`、`deskpass-helper.exe`、`helper-build.json`)。整個安裝包只有十個左右的檔案,安裝幾秒就完成。
3. 在這個資料夾的網址列(上方顯示路徑的地方)點一下,輸入 `powershell` 按 Enter,會開出一個位置就在這個資料夾的 PowerShell 視窗。在裡面執行:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

   直接打 `.\install.ps1` 時,Windows 可能說「已停用指令碼執行」;上面這一行只對這一次放行,不會改你電腦的設定。不需要系統管理員權限。

4. 看最後幾行訊息(以下是繁中的訊息;英文介面看到的是對應的英文,見「介面語言」):
   - 「已在 Claude Code 註冊 MCP server「deskpass」(user scope)。」表示已經接好 Claude Code。
   - 「PATH 上找不到 claude(Claude Code),所以沒有在 Claude Code 註冊 Deskpass。…」表示電腦上找不到 Claude Code。如果你用的是別的 AI client,不用理會;要用 Claude Code,裝好之後把下一行印出來的指令複製到 PowerShell 執行一次就好,它長這樣(路徑會換成你的使用者資料夾):

     ```powershell
     claude mcp add --scope user --transport stdio deskpass -- node "C:\Users\<你>\AppData\Local\Programs\Deskpass\dist\index.js"
     ```

   - 不論有沒有找到 Claude Code,安裝腳本都會印出這台電腦上的 **MCP server 定義**(一段 JSON),並**複製到剪貼簿**,訊息是「上面的定義已複製到剪貼簿。…」。用其他 AI client 的人,把它貼到該 client 的設定裡,各 client 的位置見「各 MCP client 的設定方式」。剪貼簿寫不進去時只會警告,安裝不受影響,請自己複製畫面上印出的那一段。

5. **重新啟動你的 AI client**(Claude Code、CodeBuddy、Cursor 等),它才會載入 Deskpass。

安裝腳本做了這些事:

- 先檢查 Node.js 是不是 22 以上;不是的話,它說明原因後就結束,電腦上什麼都沒有改。
- 把程式複製到 `%LOCALAPPDATA%\Programs\Deskpass`(也就是 `C:\Users\<你>\AppData\Local\Programs\Deskpass`),並解除這些檔案的「從網路下載」標記。
- 找得到 `claude` 時,在 Claude Code 註冊名為 `deskpass` 的 MCP server(對你所有的專案都有效)。
- 印出並複製 MCP server 定義(給其他 AI client 用)。
- 在開始功能表建立設定網頁的捷徑:繁中介面叫「Deskpass 設定」,英文介面叫「Deskpass Settings」。

你的設定和日誌放在另一個資料夾 `%LOCALAPPDATA%\deskpass`。之後拿到新版的 zip,照同樣步驟再執行一次 `install.ps1` 就是升級:程式會換成新版,設定和日誌不會動。升級後請重新啟動你的 AI client。

解壓縮出來的資料夾在安裝完之後就可以刪掉。

## 介面語言

Deskpass 的介面有兩種語言:繁體中文(`zh-TW`)和英文(`en`)。核准視窗、角落提示與邊框上的字、設定網頁(含日誌頁與確認視窗)、安裝與解除安裝的訊息,以及開始功能表捷徑的名稱,都會用同一種語言。

怎麼決定用哪一種(規則只有一條):

1. 環境變數 `DESKPASS_LANG` 的值**剛好**是 `en` 或 `zh-TW`(大小寫要一樣)時,用它指定的語言。其他值(例如 `EN`、`zh-tw`、`fr`)一律忽略,當作沒設。
2. 沒有設,或設的值被忽略時,看 **Windows 的顯示語言**(設定 → 時間與語言 → 語言;不是「區域格式」)。偏好順序最前面的那一個是 `zh-TW`、`zh-HK`、`zh-MO`,或任何 `zh-Hant`(例如 `zh-Hant-TW`)時,用繁體中文;其他情況,包括簡體中文(`zh-CN`)、日文、讀不到顯示語言,一律用英文。

要固定成某一種語言,例如在繁中的 Windows 上用英文介面,在 PowerShell 執行一次:

```powershell
setx DESKPASS_LANG en
```

要換回繁中就把 `en` 換成 `zh-TW`;要取消固定,在「系統內容 → 進階 → 環境變數」刪掉 `DESKPASS_LANG`。`setx` 只對之後新開的程式有效,所以設定後要重新啟動你的 AI client。從開始功能表打開設定網頁時,如果還是舊的語言,請登出再登入一次 Windows。想只讓安裝腳本用某種語言,可以在同一個 PowerShell 視窗先執行 `$env:DESKPASS_LANG = "en"` 再執行 `install.ps1`。

語言是在程式啟動時決定一次:AI client 啟動 Deskpass 時決定核准視窗、邊框與提示的語言;每次打開設定網頁時決定那一次的網頁語言。執行中不能切換,網頁上也沒有語言選單。

不翻譯的部分:AI 看到的工具描述與錯誤代碼(一律英文)、日誌的欄位名稱、App 的名稱與路徑。

這份手冊提到的按鈕與標題用繁中的字;英文介面的對照如下:

| 繁中 | 英文 |
|---|---|
| 本次允許 / 永遠允許 / 拒絕 | Allow once / Always allow / Deny |
| 允許 / 取消(設定網頁的確認視窗) | Allow / Cancel |
| 繼續 / 不要繼續 | Continue / Don't continue |
| 每次詢問 / 永遠允許 / 高風險 / 封鎖 | Ask every time / Always allow / High risk / Blocked |
| 設定網頁的分頁「App」「日誌」 | Apps, Log |
| 允許背景操作 | Allow background operation |
| 恢復預設… | Restore default... |
| 「Deskpass 設定」捷徑 | "Deskpass Settings" shortcut |

## 各 MCP client 的設定方式

Deskpass 是標準的 stdio MCP server,不限於 Claude Code。任何 MCP client 都能啟動它:指令是 `node`,只有一個參數,就是安裝好的 `dist\index.js` 的完整路徑。

最省事的做法:`install.ps1` 會印出這台電腦上的定義(用的是真正安裝的路徑),並複製到剪貼簿,貼上就好。下面的例子用預設的安裝位置 `%LOCALAPPDATA%\Programs\Deskpass\dist\index.js`,寫成 `C:\\Users\\<you>\\AppData\\Local\\Programs\\Deskpass\\dist\\index.js`;請把 `<you>` 換成你的 Windows 使用者名稱。在 JSON 與 TOML 裡,路徑的每個反斜線要寫兩個。

安裝腳本印出的定義長這樣:

```json
{
  "mcpServers": {
    "deskpass": {
      "command": "node",
      "args": ["C:\\Users\\<you>\\AppData\\Local\\Programs\\Deskpass\\dist\\index.js"]
    }
  }
}
```

| Client | 設定放在哪裡 | 要加什麼 |
|---|---|---|
| Claude Code | `claude` 指令,在任何資料夾執行。`claude` 在 PATH 上時,`install.ps1` 會替你執行。 | `claude mcp add --scope user --transport stdio deskpass -- node "C:\Users\<you>\AppData\Local\Programs\Deskpass\dist\index.js"`,之後 `claude mcp list` 會看到 `deskpass`。 |
| CodeBuddy | CodeBuddy 設定(聊天面板右上角)→ MCP 分頁 → 新增 MCP(JSON 編輯器)。 | 下面那一段。CodeBuddy 需要 `"type": "stdio"`。 |
| Cursor | `%USERPROFILE%\.cursor\mcp.json`(所有專案),或某個專案裡的 `.cursor\mcp.json`。 | 安裝腳本印出的定義,照貼。 |
| Claude Desktop | `%APPDATA%\Claude\claude_desktop_config.json`(設定 → 開發人員 → 編輯設定)。 | 安裝腳本印出的定義,照貼;檔案裡已經有 `mcpServers` 物件時,只把 `"deskpass": { ... }` 這一項加進去。加完要重新啟動 Claude Desktop。 |
| Codex CLI | `%USERPROFILE%\.codex\config.toml` | 下面的 TOML。 |

CodeBuddy:

```json
{
  "mcpServers": {
    "deskpass": {
      "type": "stdio",
      "command": "node",
      "args": ["C:\\Users\\<you>\\AppData\\Local\\Programs\\Deskpass\\dist\\index.js"]
    }
  }
}
```

Codex CLI(`config.toml`):

```toml
[mcp_servers.deskpass]
command = "node"
args = ["C:\\Users\\<you>\\AppData\\Local\\Programs\\Deskpass\\dist\\index.js"]
```

其他任何 stdio MCP client 做法都一樣:指令 `node`、一個參數(`dist\index.js` 的完整路徑)、伺服器名稱隨你取。改完 client 的設定之後,請重新啟動那個 client。

已知限制:不支援 MCP elicitation(client 內的詢問)的 client,會改用 Windows 的核准視窗問你。如果那個 client 的工具呼叫逾時比你回答視窗的時間短,即使你已經允許,呼叫也可能在 client 這邊失敗。這時先在視窗按允許,再請 AI 重試一次那個動作;你的允許會依照你的設定被記住。

實際在 client 裡測過的只有 Claude Code。CodeBuddy、Cursor、Claude Desktop、Codex CLI 的設定方式照各自的文件寫成,並用一個不支援 elicitation 的通用 stdio client 驗證過協定(會改用 Windows 核准視窗);沒有在那些 client 裡實際測過。ARM64 版 Windows 也沒有測過。

## 第一次 AI 要操作 App 時會發生什麼

AI 第一次要操作某個 App(例如記事本)時,畫面上會跳出「Deskpass:核准操作 App」視窗,寫著 App 的名稱和程式的完整路徑,問你要不要允許。視窗有這幾個按鈕:

- **本次允許**:這次的 AI client 工作階段裡可以操作這個 App。關掉 AI client(或它重新啟動 Deskpass)之後,下次會再問你。
- **永遠允許**:記住這個 App,以後都不再問。只有一般的、有數位簽章的 App 才會出現這個按鈕;高風險的 App 和沒有簽章的 App 沒有這個按鈕。之後想收回,到設定網頁按「取消永遠允許」。
- **拒絕**:不讓 AI 操作。這次工作階段裡 AI 再要求同一個 App,Deskpass 直接拒絕,不會一直問你;重新啟動 AI client 之後才會再問。

要注意的地方:

- **60 秒**內沒有回答,視窗自動關閉,算作拒絕(這種不會被記住,下次還會再問)。
- 預設按鈕是「拒絕」:直接按 Enter 或空白鍵就是拒絕,要允許請用滑鼠點。視窗剛出現的 0.7 秒內點允許沒有作用,避免你本來要點別的地方卻誤按。按 Esc 或關閉視窗也是拒絕。
- 這個視窗 AI 看不到、也點不到,只有你能回答。
- 有些 App(例如高風險的瀏覽器、沒有簽章的 App)在 AI client 支援時,可能改成在 AI client 自己的畫面裡問你,選項只有允許和拒絕,允許只算本次。
- AI 要讀剪貼簿時,也會先跳出「Deskpass:讀取剪貼簿」視窗問你,只有「本次允許」與「拒絕」。
- 電腦鎖定螢幕時沒辦法問你,需要問你的操作一律被拒絕。

## 四種狀態與預設

設定網頁上,每個 App 是下面其中一種狀態:

| 狀態 | 意思 |
|---|---|
| 每次詢問 | 預設。AI 每次要操作都先跳視窗問你。 |
| 永遠允許 | 你在詢問視窗按過,以後不問。只有已簽章的 App 能這樣設。 |
| 高風險 | 每次都問,而且不能永遠允許。瀏覽器、郵件等預設在這。 |
| 封鎖 | AI 完全不能操作,不會問你。終端機等預設在這。 |

預設:每個 App 都要先問你。下面這些 App 一開始就有內建的等級(設定網頁稱為「內建」):

- **內建封鎖**
  - 終端機與命令列(能執行任何程式):命令提示字元、PowerShell、Windows Terminal(含商店版)、WSL、Git Bash 等。
  - Windows 管理工具(能修改登錄檔、管理電腦、結束執行中的程式):登錄編輯程式、管理主控台(mmc)、工作管理員。
  - UAC 同意視窗(能給予系統管理員權限)。
  - Windows 認證視窗(會輸入密碼與 PIN)。
  - Deskpass 本身。
- **內建高風險**
  - 瀏覽器(能登入你的帳號、送出表單):Edge、Chrome、Firefox、Brave、Opera、Vivaldi、Internet Explorer。
  - 檔案總管(能開啟、搬移與刪除你的檔案)。
  - Windows 設定與控制台(能變更系統設定)。
  - 密碼管理工具(存有你的密碼):1Password、Bitwarden、KeePass、KeePassXC、Dashlane、LastPass。
  - 郵件(能讀取並寄出你的郵件):Outlook(含新版)、Thunderbird、Windows 郵件。

其他 App 都是一般 App,預設每次詢問。你可以把任何 App 設得比內建更嚴(例如把網銀或公司內部系統設為封鎖),但不能比內建更鬆:內建封鎖的 App 不能解除封鎖,內建高風險的 App 也不能永遠允許。

以系統管理員身分執行的視窗,AI 一律不能操作。

## 怎麼打開設定網頁

- 開始功能表 → **「Deskpass 設定」**(英文介面是「Deskpass Settings」)。
- 或在 PowerShell 執行:

  ```powershell
  node "$env:LOCALAPPDATA\Programs\Deskpass\dist\index.js" settings
  ```

它會用你的預設瀏覽器打開設定網頁。這個網頁只在你自己的電腦上(網址是 `http://127.0.0.1:...`),別台電腦連不進來。打開用的連結只能用一次;已經開著時再點一次捷徑,只會再開一個分頁。所有分頁都關掉約 3 分鐘後,它自己停止,不會一直在背景執行。分頁只是放在背景時照常可用,切回來時會立刻更新。網頁停止後要再用,就再點一次捷徑。

網頁上方有兩個分頁:

- **App**:AI 能操作哪些 App。最上面說明四種狀態。表格的欄位是 App、風險等級(一般/高風險/封鎖,並寫出是預設、內建還是你設定的)、永遠允許(是/否/不適用)、AI 上次操作的時間。
  - 篩選:預設是「AI 操作過」(授權檔裡有永遠允許或你調高等級的 App,加上最近 7 天日誌裡出現過的 App;列表裡只有這些 App);另外有「你設定過」「封鎖」「高風險」,數字只算列出的 App。App 的名稱照開始功能表的名稱顯示。
  - 搜尋框:輸入名稱的一部分就能找到 App。
  - 點表格裡的一個 App,下方會說明「為什麼是這個狀態」,並有這些按鈕:「設為高風險」「設為封鎖」「取消永遠允許」「恢復預設…」。
  - 讓限制變嚴的按鈕(設為高風險、設為封鎖、取消永遠允許)按了**立刻生效**,AI 下一次操作前就會照新的設定。設為高風險或封鎖時,它的永遠允許會一併取消。AI 還沒操作過的 App 不在列表裡:預設每個 App 第一次都會先問你,想擋的 App 在第一次被問時拒絕,再到「AI 操作過」把它設為封鎖。
- **日誌**:AI 做了什麼。可以看今天,或最近 7 天中有紀錄的日子,也可以只看某個 App。同一個 App 連續的操作合併成一組,每筆是一句話,例如哪個時間截圖、點擊、詢問核准,以及結果。你在設定網頁做的變更也會記在這裡。日誌不含輸入的文字或剪貼簿內容。

## 放寬限制為什麼要按確認視窗

AI 可以操作瀏覽器,所以理論上也可能點到設定網頁。因此網頁只能讓限制**變嚴**;要**放寬**(把你設為高風險或封鎖的 App「恢復預設…」)時,網頁不會直接改,而是跳出一個 Windows 視窗「Deskpass 確認」,寫著要把哪個 App 從什麼狀態恢復成什麼狀態。

- 只有你在這個視窗按 **「允許」** 才會改。按「取消」、按 Enter(預設按鈕是取消)、關掉視窗,或 **60 秒**沒有回答,都不會有任何改變。視窗剛出現的 0.7 秒內點「允許」沒有作用。
- 這個視窗 AI 看不到、也點不到,所以 AI 沒辦法替自己解除限制。
- 如果不是你在設定網頁按的,請按「取消」。
- 「恢復預設」只會回到預設或內建等級,不會比內建更鬆。網頁上也沒有「永遠允許」的按鈕,永遠允許只能在核准視窗按。

這個視窗開著時,兩個停止鍵的作用是:

- **Esc**:視窗有焦點時單按 Esc,等於按「取消」,視窗關閉、什麼都不改。Esc 不會讓這個視窗允許任何東西。
- **Ctrl+Alt+Esc**:這個視窗本身不理會它。AI 正在操作時,Ctrl+Alt+Esc 照常停止 AI(見「緊急停止」),但那一下按鍵由 Deskpass 攔下,不會送到這個視窗,視窗仍然開著,沒有答案之前什麼都不會改。要關掉它,按「取消」(或 Esc)。
- AI 正在以前景操作時單按 Esc,會同時停止 AI,並讓這個視窗取消。

## 背景操作

背景操作讓 AI 在另一個 App 做事時,不拿走你的畫面和鍵盤。AI 按按鈕、勾選項目、在欄位填字的時候,你正在用的視窗還是在最前面,游標不會被移動,你可以繼續打字或用滑鼠。

- **預設開啟**,不用設定。想關掉見下面「怎麼關掉」。
- 規則照舊:每個 App 第一次還是要先問你,封鎖的 App 還是不能碰,以系統管理員身分執行的視窗還是不能操作。背景操作不會讓 AI 多碰任何 App。

### 哪些動作在背景,哪些在前景

| | 動作 |
|---|---|
| 預設在背景 | 按按鈕或選單項目、勾選核取方塊、選清單項目或分頁、展開與收合下拉選單、在欄位填入整段文字、左鍵單擊按鈕這類元件 |
| 一律在前景 | 打字、按鍵、貼上文字、捲動、拖曳 |

前景的意思是:AI 把那個視窗帶到最前面,用真的滑鼠和鍵盤操作,和背景操作出現之前一樣。

背景做不到的動作有這些:右鍵、按兩下、按住 Ctrl 或 Shift 的點擊;點在沒有元件可用的地方(例如網頁內容、畫布);把鍵盤焦點放進某個欄位;視窗被最小化。AI 要求打字這類一律在前景的動作用背景時,也是這樣回覆。這時 Deskpass 什麼都不送,回覆 AI「背景操作不可用」,並建議改用前景。

- 由 AI 決定要不要只把那一個動作改用前景重做;之後的動作又先用背景。
- Deskpass 不會自己改用前景,也不會再問你,因為這個 App 你已經核准過。
- 日誌上會看到那筆「背景操作不可用」,和接著的前景動作。

### 你會看到什麼

- **邊框**:只畫在目標視窗看得見的部分,不會畫到你正在用的視窗上。目標整個被擋住時就不畫。邊框上緣寫著「AI 正在操作 · 按 Ctrl+Alt+Esc 停止」。
- **角落提示**:在目標視窗所在螢幕的右下角,寫著「AI 正在操作 ‹App 名稱› · Ctrl+Alt+Esc 停止」。`‹App 名稱›` 是核准視窗上看到的名稱。AI 用前景操作時,寫的是「AI 正在操作 ‹App 名稱› · Esc 停止」。它不會搶走焦點,滑鼠點下去會點到下面的視窗,也不會出現在 AI 的截圖裡。
- 邊框和角落提示一起出現、一起消失:最後一個動作後 10 秒沒有新動作,就都消失。
- 有些 App 收到動作時,會自己跳到最前面。Deskpass 一偵測到,就立刻把前景還給你原本的視窗,並把那個 App 放回原本的疊放位置。多半只佔幾毫秒,通常看不到閃動。

### AI 會等你停手

每個背景動作開始前,AI 先等:你的鍵盤停了 0.3 秒,而且沒有按住滑鼠按鍵,才動作。這是因為 App 跳到最前面的那一瞬間,你正在打的字可能跑進去。

- 最多等 3 秒。你一直不停地打字或按住滑鼠超過 3 秒,AI 就不再等,照樣動作。
- 只算你親手的操作。AI 自己送出的按鍵和點擊不算。
- 等的時候你按停止鍵,AI 就不動作。

### 怎麼停止

- **Ctrl+Alt+Esc**(三個鍵一起按):任何時候都能停止 AI。
- **Esc**(單按):只有在 AI 以前景操作時才算。背景操作時,你在自己的 App 按 Esc(例如關掉自動完成、在 Vim 切換模式)不會停掉 AI。

詳細見下面的「緊急停止」。

### 怎麼關掉

1. 開始功能表 → **「Deskpass 設定」**(也可以照上面「怎麼打開設定網頁」)。
2. 在設定網頁的 **「App」** 分頁(打開時就是這一頁)上方,四種狀態的說明下面,有一個開關 **「允許背景操作」**。
3. 點一下把它關掉。開關下面的字會變成「已關閉:AI 的每個動作都以前景進行,你看得到它在操作;它要求背景時也一樣。」,頁面也會說「已關閉背景操作。AI 的下一個操作起,一律以前景進行。」

- 從 AI 的下一個操作起生效,不用重新啟動 AI client。關掉之後,AI 的每個動作都在前景進行,即使它要求背景也一樣。
- 想再打開,同樣點一下開關。打開時不會跳確認視窗。

### 日誌怎麼看

設定網頁的 **「日誌」** 分頁,AI 的每筆動作旁邊都有標記:**「背景」**表示在背景完成,**「前景」**表示用前景完成。背景做不到的那筆會標「背景操作不可用」。

### 背景操作的已知限制

- 不是每個 App 都做得到。網頁內容、自己畫畫面的 App(例如小畫家的畫布)通常沒有背景能操作的元件,AI 會改用前景。
- AI 的背景點擊不等於真的滑鼠點擊。少數 App 只認真的滑鼠,動作顯示成功但畫面沒有變。AI 會截圖確認,沒變就改用前景。
- 少數 App 的核取方塊,偶爾背景勾選沒有生效。AI 會看結果,沒變可以重做。
- App 跳到最前面到被拉回來之間,你剛好打的 1 個字仍可能進到那個 App。這就是 AI 要先等你停手的原因。
- 偶爾 Deskpass 沒能把前景還給你,動作已經做完,但你的視窗被搶走。AI 會知道,不會重做;你自己點回原來的視窗就好。
- 你的最前面視窗若是螢幕小鍵盤這類不能取得焦點的視窗,動作後視窗的疊放順序可能不是原樣。
- 展開下拉選單時,App 自己畫的彈出視窗可能出現在你的視窗上面。Deskpass 不會移動它。

## 緊急停止

AI 操作時,目標視窗外面會有一圈邊框,上緣的字寫著現在用哪個鍵停止:背景操作寫「AI 正在操作 · 按 Ctrl+Alt+Esc 停止」,前景操作寫「AI 正在操作 · 按 Esc 停止」。螢幕角落的提示也寫同一個鍵(見「背景操作」)。最後一個動作後 10 秒沒有新動作,邊框會消失。

停止鍵有兩個:

- **Ctrl+Alt+Esc**(三個鍵一起按):任何時候都有效,不論 AI 在背景還是前景操作。Deskpass 會攔下這組鍵,不會傳給 Windows 或你正在用的 App(Windows 本來會把最前面的視窗推到最後面)。
- **Esc**(單按):只在 AI 以前景操作時有效,也就是前景動作進行中,以及那次動作的邊框還顯示的時候。其他時候單按 Esc 照常交給你正在用的 App,不會停掉 AI。

按下停止鍵之後:

- AI 立刻停止送出任何輸入(已送出、App 還沒讀到的 1 到 2 個字仍可能出現),並被要求結束這一回合、告訴你發生什麼事。
- 只有你親手按的才算;AI 自己送出的 Esc 或 Ctrl+Alt+Esc 不算停止。
- 停止之後 AI 若又要操作,Deskpass 會先問你「你先前按了 Esc 停止操作,要讓 AI 繼續嗎?」(在 AI client 的畫面裡,或在「Deskpass:要繼續嗎?」視窗)。這句話在你按 Ctrl+Alt+Esc 停止時也是這樣寫。你允許(視窗裡是「繼續」)才會繼續;拒絕(「不要繼續」)、在視窗按 Esc 或 60 秒沒有回答,就維持停止。

Deskpass 自己的視窗開著時,這兩個鍵怎麼作用,見「第一次 AI 要操作 App 時會發生什麼」(核准視窗:Esc 是拒絕)與「放寬限制為什麼要按確認視窗」(確認視窗:Esc 是取消,視窗不理會 Ctrl+Alt+Esc)。

## 未簽章的 helper 與 SHA-256

Deskpass 附帶一支 Rust 寫的 helper(`dist\deskpass-helper.exe`),真正的視窗操作、截圖與核准視窗都在這支程式裡。**helper 和安裝腳本都沒有數位簽章**(沒有購買程式碼簽章憑證),所以 Windows 的 SmartScreen 或公司的防毒軟體有可能對它提出警告;怎麼處理見下一節。

因為沒有簽章,請用雜湊值確認你拿到的 zip 沒有被換過。每個版本的 GitHub Release 說明裡都附有 zip 的 SHA-256。在 PowerShell 執行(把檔名換成你下載的):

```powershell
Get-FileHash .\deskpass-0.1.0.zip -Algorithm SHA256
```

輸出的 `Hash` 欄位要和 Release 說明裡的 SHA-256 完全相同(十六進位字母大小寫不影響)。不相同就不要安裝,請重新下載,還是不同就回報(見 [SECURITY](../SECURITY.md))。雜湊值只證明檔案和發布者放上去的一樣。zip 是 GitHub 的 Release workflow 從打了 tag 的原始碼建置的,並附有簽署過的建置來源證明;裝了 [GitHub CLI](https://cli.github.com/) 可以執行 `gh attestation verify deskpass-0.1.0.zip --repo norton77930/deskpass`,確認 zip 是哪一個 commit、由哪個 workflow 建置的。要連 GitHub 的建置都不用信任,可以自己照 [開發文件](development.md) 的「Build」和「Packaging」從原始碼建置、打包。

## SmartScreen 或公司資安擋下時怎麼辦

Deskpass 的程式沒有數位簽章(見上一節),所以 Windows 或公司的防毒軟體有可能攔下它。

- **從郵件或瀏覽器下載的 zip**:Windows 會把裡面每個檔案標成「從網路下載」。`install.ps1` 安裝完會替安裝好的程式檔解除這個標記,所以一般不會再看到 SmartScreen 的警告。
- **執行 `install.ps1` 時說「已停用指令碼執行」**:請用安裝步驟裡的 `powershell -ExecutionPolicy Bypass -File .\install.ps1`。
- **安裝時跳出 SmartScreen 或「Windows 已保護您的電腦」**:先照上一節核對 SHA-256,確認 zip 是你信任的來源,再按「其他資訊」→「仍要執行」。
- **點開始功能表的「Deskpass 設定」沒有反應**:捷徑是透過 Windows Script Host(wscript)啟動的,有些公司會停用它。請改在 PowerShell 執行:

  ```powershell
  node "$env:LOCALAPPDATA\Programs\Deskpass\dist\index.js" settings
  ```

- **防毒軟體刪掉或封鎖 `deskpass-helper.exe`,或公司政策不允許執行未簽章程式**:請不要自己關掉防毒,先找公司 IT。

其他問題,請到專案的 GitHub 頁面查看已知問題,或聯絡把 zip 給你的人;安全相關的問題請照 [SECURITY](../SECURITY.md)。

## 解除安裝

在 PowerShell 執行:

```powershell
powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\Deskpass\uninstall.ps1"
```

它會移除 Claude Code 裡的 `deskpass` 註冊(`claude` 在 PATH 上時)、開始功能表的捷徑(「Deskpass 設定」或「Deskpass Settings」,不論是哪種語言建立的),以及 `%LOCALAPPDATA%\Programs\Deskpass` 裡的程式檔。你的設定與日誌(`%LOCALAPPDATA%\deskpass`)預設保留。要連它一起刪,加上 `-RemoveData`:

```powershell
powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\Deskpass\uninstall.ps1" -RemoveData
```

`-RemoveData` 只多刪 `%LOCALAPPDATA%\deskpass` 這一個資料夾,不會刪其他東西。如果它說有些檔案刪不掉,多半是你的 AI client 還在使用 Deskpass:關掉 AI client 和設定網頁後再執行一次。

解除安裝腳本不會替其他 AI client(CodeBuddy、Cursor、Claude Desktop、Codex CLI)移除設定;如果你當初把定義貼進了它們的設定檔,請自己刪掉那一段。

## 已知限制

- 只支援 Windows 10 2004 以上與 Windows 11。沒有 macOS 或 Linux 版本。
- 不能操作以系統管理員身分執行的視窗,Deskpass 也不會替自己提升權限。
- 沒有簽章的 App 是依程式的路徑認出來的:程式搬到別的資料夾,或之後換成有簽章的版本,你對它做的設定(例如封鎖)就不再套用,而且日誌不會提醒。
- 密碼欄:AI 不能在密碼欄打字或貼上,會請你自己輸入;但 App 沒有正確標示密碼欄時,Deskpass 認不出來。
- 打字與按鍵跟著 App 的速度,比一般貼上慢;長文字要分成多次。
- 部分 Windows 內建 App(小算盤、設定這類 UWP App)無法確認每個字都被讀到,AI 會被要求截圖確認。
- 背景操作也有做不到或偶爾失靈的情形,見「背景操作」最後的「背景操作的已知限制」。
- 不支援 MCP elicitation 的 AI client 會改用 Windows 視窗問你,可能遇到 client 逾時,見「各 MCP client 的設定方式」。
- helper 與安裝腳本沒有數位簽章,見「未簽章的 helper 與 SHA-256」。
- 更多技術上的限制,見 [開發文件](development.md) 的「Technical limits」。

## 從開發版升級

如果你之前用過開發版(名稱是 `computer-use-mcp`):

- MCP server 改名為 `deskpass`,AI 看到的工具名稱前綴從 `mcp__computer-use-mcp__` 改成 `mcp__deskpass__`。執行 `install.ps1` 會註冊新的 `deskpass`;舊的註冊請自己移除(例如 `claude mcp remove computer-use-mcp`,加上你當初註冊用的 `--scope`)。若你有寫死工具名稱的設定或提示,也要一起改。
- 設定要重新來過:Deskpass 不讀舊版的永遠允許、調高與日誌。
- 舊的 `%LOCALAPPDATA%\computer-use-mcp` 資料夾 Deskpass 不讀、不搬、不刪,可以手動刪除。
