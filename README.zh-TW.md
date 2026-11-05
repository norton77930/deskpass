<h1 align="center">Deskpass</h1>

<h3 align="center">讓 coding agent 使用你的 Windows 桌面,一次一個 App。</h3>

<p align="center">
  Deskpass 是 Windows 上的 computer use MCP server。你的 coding agent 可以看、<br>
  點擊、在桌面 App 裡打字,而哪些 App 可以碰,由你決定。
</p>

<table align="center">
  <tr>
    <td align="center" width="200"><b>每個 App 都要你核准</b><br><sub>AI 第一次碰一個 App 之前,會先跳出視窗問你</sub></td>
    <td align="center" width="200"><b>在背景操作</b><br><sub>按鈕、核取方塊、欄位都在背景改,不搶你的焦點</sub></td>
    <td align="center" width="200"><b>Ctrl+Alt+Esc 就停</b><br><sub>任何時候、在任何 App 裡都有效</sub></td>
    <td align="center" width="200"><b>先讀 UI 樹</b><br><sub>依角色、名稱與狀態找控制項,不只看像素</sub></td>
  </tr>
</table>

<p align="center">
  <a href="https://github.com/norton77930/deskpass/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/norton77930/deskpass/ci.yml?branch=master&style=flat-square&labelColor=16181d&label=CI" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-2ea44f?style=flat-square&labelColor=16181d" alt="MIT 授權"></a>
</p>

<p align="center">
  <a href="#安裝"><b>安裝</b></a> · <a href="#mcp-client-設定">MCP client 設定</a> · <a href="#運作方式">運作方式</a> · <a href="#證據">證據</a> · <a href="README.md">English</a>
</p>

## 為什麼不用截圖加座標點擊的 computer use?

讓模型使用桌面的常見做法,是給它一張截圖,讓它用真的滑鼠和鍵盤點擊像素座標。
這對任何畫在螢幕上的東西都有效,Deskpass 也保留這個做法,專門處理那些情況。它做不到
的是告訴模型某個控制項是什麼,或在模型做事時把桌面留給你。

| 情境 | 截圖加座標 | Deskpass |
| :--- | :--- | :--- |
| 目標是密集視窗裡的小按鈕 | 模型在縮小過的圖上估一個像素位置,可能點偏 | 模型讀視窗的 UI 樹,依編號點那個元素 |
| 截圖之後視窗改了大小 | 舊圖上的座標會落到別的地方 | 元素動作不靠座標;來自改大小之前那張截圖的座標會被拒絕(`stale_screenshot`),什麼都不送 |
| 模型要知道控制項的狀態 | 從像素判斷核取方塊有沒有勾、清單有沒有展開 | 樹會列出值與狀態(開、關、展開、焦點、停用),動作完成後讀回新狀態 |
| 模型要在文字欄位填字 | 一個鍵一個鍵打,跟著 App 的速度,輸入法可能組字或卡住字 | `computer_set_value` 經 UI Automation 一次寫入整段值,並讀回確認 |
| AI 做事時你繼續用電腦 | 每次點擊都要把視窗帶到前面,游標也被移走 | 按鈕、核取方塊、清單與欄位的動作在背景完成;App 搶走前景時立刻還給你,背景做不到的動作會被拒絕(`background_unavailable`),不會悄悄佔走你的畫面 |

截圖仍然在:`computer_screenshot` 與 `computer_zoom` 處理樹涵蓋不到的地方,例如網頁內容與畫布。

## 安裝

你需要 Windows 10(2004 以上)或 Windows 11,以及 [Node.js](https://nodejs.org/) 22
以上。不需要系統管理員權限,也不需要建置工具。

1. 從 [GitHub Releases](https://github.com/norton77930/deskpass/releases) 下載
   `deskpass-<版本>.zip`。
2. 用 release 說明裡的 SHA-256 檢查它:

   ```powershell
   Get-FileHash .\deskpass-0.1.0.zip -Algorithm SHA256
   ```

   Release 由 [Release workflow](.github/workflows/release.yml) 從打了 tag 的原始碼
   建置,並附有簽署過的建置來源證明(build provenance)。裝了
   [GitHub CLI](https://cli.github.com/) 的話,可以確認 zip 就是它建置的那一份:
   `gh attestation verify deskpass-0.1.0.zip --repo norton77930/deskpass`。

3. 解壓縮 zip,在解壓縮出來的資料夾開 PowerShell,執行:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

安裝腳本把 Deskpass 複製到 `%LOCALAPPDATA%\Programs\Deskpass`;PATH 上找得到 `claude`
時在 Claude Code 註冊;建立開始功能表捷徑「Deskpass 設定」;並印出這台電腦的 MCP server
定義、複製到剪貼簿,給其他 client 使用(見 [MCP client 設定](#mcp-client-設定))。完成後
重新啟動你的 AI client。

**SmartScreen:** helper 與腳本都沒有程式碼簽章。`install.ps1` 會替安裝好的檔案解除「從網路
下載」的標記,所以 SmartScreen 通常不會出聲;如果它還是警告,先確認 zip 與公布的 SHA-256
相符,再按「其他資訊」→「仍要執行」。防毒軟體或公司政策擋下 `deskpass-helper.exe` 時,請
找公司 IT,不要自己關掉防護。

介面(核准對話框、角落提示、設定網頁與安裝腳本)跟著 Windows 介面語言:zh-TW、zh-HK、
zh-MO 或任何 zh-Hant 語言時是繁體中文,其他是英文。把環境變數 `DESKPASS_LANG` 設成 `en`
或 `zh-TW` 可以指定。

解除安裝:

```powershell
powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\Deskpass\uninstall.ps1"
```

`%LOCALAPPDATA%\deskpass` 裡的設定與日誌會保留,加上 `-RemoveData` 才一起刪。

## 運作方式

Deskpass 分成兩個行程。Node.js 的 MCP server 負責所有政策判斷:核准、風險等級、單一
操作者鎖、停止狀態、截圖縮放與動作日誌。Rust helper 只執行交代給它的 Win32 工作:擷取
視窗、UI Automation、輸入、邊框與角落提示、核准對話框,以及停止鍵。

- **逐 App 核准。** AI 第一次要用某個 App 時會問你。「本次允許」只到這個 session 結束;
  「永遠允許」只對有簽章的 App 提供,記在有簽章保護的授權檔裡。終端機、登錄編輯程式、
  工作管理員與 UAC 視窗一律封鎖;瀏覽器、郵件、檔案總管與密碼管理工具每個 session 都要問。
- **先讀 UI 樹。** `computer_get_ui_tree` 列出視窗的控制項並編號;帶 `element_index` 的
  `computer_click`、`computer_element_action` 與 `computer_set_value` 對它們動作。截圖會
  縮到模型的影像預算內,座標再換算回去。
- **背景操作。** 元素動作經 UI Automation 完成,不把視窗帶到前面。每個動作前,Deskpass
  先等你的鍵盤停 0.3 秒(最多等 3 秒);App 還是搶走前景時,立刻還給你的視窗。
- **停止鍵。** 實體的 Ctrl+Alt+Esc 任何時候都能停止 AI;AI 在前景操作時,單按 Esc 也可以。
  AI 自己送出的按鍵不算。停止之後,Deskpass 會先問你,AI 才能繼續。
- **日誌不記內容。** 日誌記下每個動作與它操作的 App,不記輸入的文字、剪貼簿內容或截圖。

設計說明、每個工具與技術限制見 [docs/development.md](docs/development.md)(英文)。
術語定義在 [CONTEXT.md](CONTEXT.md),設計決定在 [docs/adr/](docs/adr/)。

## 支援範圍

- **Windows 10(2004 以上)與 Windows 11,x64。** ARM64 未測試。沒有 macOS 或 Linux 版本。
- **已在 Claude Code 測試。**
- **CodeBuddy、Cursor、Claude Desktop 與 Codex CLI:** 下面有設定方式,協定已用一個沒有
  elicitation 的通用 stdio MCP client 驗證,但還沒有在這些 client 裡實際測試。
- 其他 stdio MCP client 用同樣方式應該可行,未測試。

## MCP client 設定

Deskpass 是 stdio MCP server:指令是 `node`,唯一的參數是安裝好的 `dist\index.js` 的完整
路徑。`install.ps1` 會印出這台電腦的確切定義並複製到剪貼簿,貼上即可。下面的例子用預設
位置 `%LOCALAPPDATA%\Programs\Deskpass\dist\index.js`;把 `<you>` 換成你的 Windows 使用者
名稱。在 JSON 與 TOML 裡,每個反斜線都要寫兩次。

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
| Claude Code | `claude` 指令,在任何資料夾執行。PATH 上有 `claude` 時,`install.ps1` 會替你執行。 | `claude mcp add --scope user --transport stdio deskpass -- node "C:\Users\<you>\AppData\Local\Programs\Deskpass\dist\index.js"`,之後 `claude mcp list` 會列出 `deskpass`。 |
| CodeBuddy | CodeBuddy 設定(聊天面板右上角)→ MCP 分頁 → 新增 MCP(JSON 編輯器)。 | 下面那一段。CodeBuddy 需要 `"type": "stdio"`。 |
| Cursor | 所有專案用 `%USERPROFILE%\.cursor\mcp.json`,單一專案用專案裡的 `.cursor\mcp.json`。 | 印出來的定義,原樣貼上。 |
| Claude Desktop | `%APPDATA%\Claude\claude_desktop_config.json`(Settings → Developer → Edit Config)。 | 印出來的定義,原樣貼上;檔案裡已經有 `mcpServers` 物件時,只把 `"deskpass": { ... }` 這一項加進去。之後重新啟動 Claude Desktop。 |
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

其他 stdio MCP client 也一樣:指令 `node`,一個參數(`dist\index.js` 的完整路徑),server
名稱隨你取。改完 client 的設定後,重新啟動 client。

已知限制:不支援 MCP elicitation 的 client,會改由 Windows 核准對話框詢問,而不是在 client
裡詢問。如果那個 client 的工具呼叫逾時比你回答對話框的時間短,即使你按了允許,這次呼叫
也可能在 client 端失敗。這時先在對話框按允許,再請 agent 重做一次;核准會依你的設定記住。

完整的繁體中文使用手冊(第一次核准、設定網頁、背景操作、緊急停止、SmartScreen、解除安裝)
在 [docs/user-guide.zh-TW.md](docs/user-guide.zh-TW.md)。

## 證據

只放可以重現的數字,不放速度或成功率的 benchmark。

- **npm 測試:** 711 個測試,707 個通過、4 個略過(會開出真實視窗的端對端測試在沒有桌面
  時略過),照 CI 的方式以 `DESKPASS_NO_DESKTOP=1` 執行。
- **Rust 單元測試:** 109 個通過(在 `helper/` 執行 `cargo test --bins`)。
- **Rust 桌面測試:** `helper/tests` 的整合測試會操作真實視窗,所以 CI 只確認它們能編譯;
  它們在真實桌面上執行。
- **CI:** 每次 push 到 `master` 與每個 pull request,都會在 `windows-latest` 上跑
  TypeScript 建置、Rust 單元測試與 npm 測試([workflow](.github/workflows/ci.yml))。

## 已知限制

- 只支援 Windows 10(2004 以上)與 Windows 11。
- 不能操作以系統管理員身分執行的視窗,Deskpass 也不會替自己提升權限。
- 沒有簽章的 App 是依路徑認出來的:程式搬了位置,或之後換成有簽章的版本,你對它做的
  設定(例如封鎖)就不再套用,而且日誌不會提醒。有簽章但檔名和版本資訊裡的原始檔名不同
  (改名的副本)、或沒有版本資訊的 App 也一樣,只能允許一次。
- AI 不能在密碼欄打字或貼上,會請你自己輸入;但 App 沒有標示成密碼欄的欄位,Deskpass
  認不出來。
- 打字與按鍵跟著 App 的速度,比貼上慢;長文字要分成多次。部分 Windows 內建的 UWP App
  (小算盤、設定)無法確認每個鍵都被讀到,AI 會被要求截圖確認。
- Deskpass 改用把 App 帶到前面的方式時,偶爾會掉一個鍵;測試中在自然情況下約 30 次 1 次。
- 背景操作:
  - 不是每個 App 都做得到。網頁內容與自己畫畫面的 App(例如小畫家的畫布)通常沒有可以
    動作的元素,AI 會收到 `background_unavailable`,可以改用前景重做。
  - 背景點擊是 UI Automation 動作,不是真的滑鼠點擊。少數 App 只認真的滑鼠:動作回報
    成功,畫面卻沒變。AI 會截圖確認。
  - 少數 App 的核取方塊,偶爾背景勾選沒有生效;結果會如實回報狀態沒變,AI 可以重做。
  - App 搶走前景到 Deskpass 還給你之間的那一瞬間,你剛好打的鍵可能進到那個 App。所以
    Deskpass 會先等你停手。
  - 偶爾前景沒能還給你:動作已經完成,但你的視窗失去焦點。AI 會知道,不會重做;自己點回
    你的視窗即可。
  - 你最前面的視窗若是不能取得焦點的視窗(例如螢幕小鍵盤),動作後視窗的疊放順序可能
    和原本不同。
  - App 自己畫的下拉選單彈出視窗可能出現在你的視窗上面;Deskpass 不會移動它。

## 測試

需要 Node.js 22 以上,以及 Rust stable 的 MSVC 工具鏈(Visual Studio C++ 建置工具)。

```powershell
npm ci
npm run build:ts
$env:DESKPASS_NO_DESKTOP = "1"   # 和 CI 一樣,略過會開出真實視窗的端對端測試
npm test
cargo test --manifest-path helper/Cargo.toml --bins
```

不設 `DESKPASS_NO_DESKTOP` 時,`npm test` 也會跑端對端測試;`npm run test:rust` 會跑
`helper/tests` 的桌面測試。它們會對本專案自己的測試視窗送出真實輸入:請保持螢幕解鎖、
不要動滑鼠鍵盤,也不要同時跑這兩套。細節見 [docs/development.md](docs/development.md)。

## 參與貢獻

建置、測試與兩份 README 如何保持一致,見 [CONTRIBUTING.md](CONTRIBUTING.md)。版本紀錄在
[CHANGELOG.md](CHANGELOG.md)。回報安全問題請照 [SECURITY.md](SECURITY.md)。

## 致謝

- [trycua/cua](https://github.com/trycua/cua):它的 Windows driver 明確區分背景與前景兩種
  送達方式,影響了 [ADR 0001](docs/adr/0001-uia-actions-background-delivery.md)。Deskpass
  參考了它的做法,沒有複製程式碼。
- [Hallpass](https://github.com/norton77930/hallpass):Chrome 版的姊妹專案,Deskpass 的
  安裝腳本照它的做法。

## 授權

[MIT](LICENSE)。
