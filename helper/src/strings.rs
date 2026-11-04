//! Open-source ticket 03: every text the helper shows the user, one table per language with the
//! same keys: the approval dialog, the clipboard question, the "continue?" question after Esc, the
//! settings page's confirm window, the border's label and the corner notice.
//!
//! The helper never detects the language: the server passes the one it decided (`--lang en` or
//! `--lang zh-TW`, see [`set_from_args`]); without one, or with any other value, the texts are English.
//! Texts that name something (an app, a number of seconds) are templates with `{name}`-style
//! placeholders, filled by [`fill`].

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    ZhTw,
}

impl Lang {
    /// `zh-TW` is Traditional Chinese; anything else (`en`, a value this helper does not know) is English.
    pub fn parse(tag: &str) -> Lang {
        if tag == "zh-TW" {
            Lang::ZhTw
        } else {
            Lang::En
        }
    }
}

static ZH_TW_SELECTED: AtomicBool = AtomicBool::new(false);

/// The language the texts are in; English until [`set`] says otherwise.
pub fn current() -> Lang {
    if ZH_TW_SELECTED.load(Ordering::SeqCst) {
        Lang::ZhTw
    } else {
        Lang::En
    }
}

pub fn set(lang: Lang) {
    ZH_TW_SELECTED.store(lang == Lang::ZhTw, Ordering::SeqCst);
}

/// Reads `--lang <en|zh-TW>` from the helper's arguments (once, at start) and selects it.
pub fn set_from_args(args: impl IntoIterator<Item = String>) {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--lang" {
            set(args.next().map(|tag| Lang::parse(&tag)).unwrap_or(Lang::En));
            return;
        }
    }
}

pub const APPROVE_TITLE: &str = "approve.title";
pub const APPROVE_BODY: &str = "approve.body";
pub const APPROVE_ALWAYS_BODY: &str = "approve.always_body";
pub const CLIPBOARD_TITLE: &str = "clipboard.title";
pub const CLIPBOARD_BODY: &str = "clipboard.body";
pub const CONTINUE_TITLE: &str = "continue.title";
/// The question itself, also the text of the client elicitation (`CONTINUE_MESSAGES` in src/server.ts).
pub const CONTINUE_QUESTION: &str = "continue.question";
pub const CONTINUE_BODY: &str = "continue.body";
pub const CONFIRM_TITLE: &str = "confirm.title";
pub const CONFIRM_BODY: &str = "confirm.body";
pub const BUTTON_ALLOW_ONCE: &str = "button.allow_once";
pub const BUTTON_ALLOW_ALWAYS: &str = "button.allow_always";
pub const BUTTON_DENY: &str = "button.deny";
pub const BUTTON_CONTINUE: &str = "button.continue";
pub const BUTTON_DONT_CONTINUE: &str = "button.dont_continue";
pub const BUTTON_CONFIRM_ALLOW: &str = "button.confirm_allow";
pub const BUTTON_CONFIRM_CANCEL: &str = "button.confirm_cancel";
pub const LABEL_FOREGROUND: &str = "overlay.label_foreground";
pub const LABEL_BACKGROUND: &str = "overlay.label_background";
pub const NOTICE: &str = "overlay.notice";
pub const NOTICE_APP: &str = "overlay.notice_app";
pub const NOTICE_TITLE: &str = "overlay.notice_title";

pub const EN: &[(&str, &str)] = &[
    (APPROVE_TITLE, "Deskpass: Approve app"),
    (APPROVE_BODY, "Allow the AI to operate this app for this session?\r\n\r\n{name}\r\n{path}\r\n\r\n\"Allow once\" only lasts for this session. No answer within {seconds} seconds counts as a refusal."),
    (
        APPROVE_ALWAYS_BODY,
        "Allow the AI to operate this app?\r\n\r\n{name}\r\n{path}\r\n\r\n\"Allow once\" only lasts for this session; \"Always allow\" remembers this app and stops asking, and can be revoked on the Deskpass settings page. No answer within {seconds} seconds counts as a refusal.",
    ),
    (CLIPBOARD_TITLE, "Deskpass: Read clipboard"),
    (
        CLIPBOARD_BODY,
        "Allow the AI to read the text on your clipboard for this session?\r\n\r\nThe clipboard may hold passwords or other private content. Only text is read, never images or files. \"Allow once\" only lasts for this session. No answer within {seconds} seconds counts as a refusal.",
    ),
    (CONTINUE_TITLE, "Deskpass: Continue?"),
    (CONTINUE_QUESTION, "You pressed Esc to stop Deskpass earlier. Let the AI continue?"),
    (CONTINUE_BODY, "{question}\r\n\r\nNo answer within {seconds} seconds counts as not continuing."),
    (CONFIRM_TITLE, "Deskpass confirmation"),
    (
        CONFIRM_BODY,
        "The settings page asks to loosen a limit\r\n\r\nRestore \"{name}\" from \"{current}\" to its default \"{restored}\".\r\n\r\nAfter that, the AI can ask you again to approve operating it. If you did not press this on the settings page, choose \"Cancel\". No answer within {seconds} seconds counts as cancelling.",
    ),
    (BUTTON_ALLOW_ONCE, "Allow once"),
    (BUTTON_ALLOW_ALWAYS, "Always allow"),
    (BUTTON_DENY, "Deny"),
    (BUTTON_CONTINUE, "Continue"),
    (BUTTON_DONT_CONTINUE, "Don't continue"),
    (BUTTON_CONFIRM_ALLOW, "Allow"),
    (BUTTON_CONFIRM_CANCEL, "Cancel"),
    (LABEL_FOREGROUND, "AI is operating · press Esc to stop"),
    (LABEL_BACKGROUND, "AI is operating · press Ctrl+Alt+Esc to stop"),
    (NOTICE, "AI is operating · {key} to stop"),
    (NOTICE_APP, "AI is operating {app} · {key} to stop"),
    (NOTICE_TITLE, "AI is operating"),
];

pub const ZH_TW: &[(&str, &str)] = &[
    (APPROVE_TITLE, "Deskpass:核准操作 App"),
    (APPROVE_BODY, "要允許 AI 在本次工作階段操作這個 App 嗎?\r\n\r\n{name}\r\n{path}\r\n\r\n「本次允許」只在本次工作階段有效。{seconds} 秒內沒有回答視為拒絕。"),
    (
        APPROVE_ALWAYS_BODY,
        "要允許 AI 操作這個 App 嗎?\r\n\r\n{name}\r\n{path}\r\n\r\n「本次允許」只在本次工作階段有效;「永遠允許」會記住這個 App,之後不再詢問,可在 Deskpass 設定網頁撤銷。{seconds} 秒內沒有回答視為拒絕。",
    ),
    (CLIPBOARD_TITLE, "Deskpass:讀取剪貼簿"),
    (
        CLIPBOARD_BODY,
        "要允許 AI 在本次工作階段讀取剪貼簿裡的文字嗎?\r\n\r\n剪貼簿可能有密碼或其他私人內容。只讀文字,不讀圖片與檔案。「本次允許」只在本次工作階段有效。{seconds} 秒內沒有回答視為拒絕。",
    ),
    (CONTINUE_TITLE, "Deskpass:要繼續嗎?"),
    (CONTINUE_QUESTION, "你先前按了 Esc 停止操作,要讓 AI 繼續嗎?"),
    (CONTINUE_BODY, "{question}\r\n\r\n{seconds} 秒內沒有回答視為不繼續。"),
    (CONFIRM_TITLE, "Deskpass 確認"),
    (
        CONFIRM_BODY,
        "設定網頁要求放寬限制\r\n\r\n把「{name}」從「{current}」恢復成預設「{restored}」。\r\n\r\n恢復後,AI 可以再請你核准操作它。如果不是你在設定網頁按的,請按「取消」。{seconds} 秒內沒有回答視為取消。",
    ),
    (BUTTON_ALLOW_ONCE, "本次允許"),
    (BUTTON_ALLOW_ALWAYS, "永遠允許"),
    (BUTTON_DENY, "拒絕"),
    (BUTTON_CONTINUE, "繼續"),
    (BUTTON_DONT_CONTINUE, "不要繼續"),
    (BUTTON_CONFIRM_ALLOW, "允許"),
    (BUTTON_CONFIRM_CANCEL, "取消"),
    (LABEL_FOREGROUND, "AI 正在操作 · 按 Esc 停止"),
    (LABEL_BACKGROUND, "AI 正在操作 · 按 Ctrl+Alt+Esc 停止"),
    (NOTICE, "AI 正在操作 · {key} 停止"),
    (NOTICE_APP, "AI 正在操作 {app} · {key} 停止"),
    (NOTICE_TITLE, "AI 正在操作"),
];

fn table(lang: Lang) -> &'static [(&'static str, &'static str)] {
    match lang {
        Lang::En => EN,
        Lang::ZhTw => ZH_TW,
    }
}

/// The text `key` has in `lang`; the key itself when the table has none (the tests keep both tables complete).
pub fn text_in(lang: Lang, key: &'static str) -> &'static str {
    table(lang).iter().find(|(name, _)| *name == key).map_or(key, |(_, text)| text)
}

/// The text `key` has in the selected language.
pub fn text(key: &'static str) -> &'static str {
    text_in(current(), key)
}

/// `template` with each `{name}` replaced by its value in `values`, in one pass (a value that
/// itself looks like a placeholder is not filled again); an unknown `{name}` is left as it is.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        match tail.find('}') {
            Some(close) => {
                let name = &tail[1..close];
                match values.iter().find(|(key, _)| *key == name) {
                    Some((_, value)) => out.push_str(value),
                    None => out.push_str(&tail[..=close]),
                }
                rest = &tail[close + 1..];
            }
            None => {
                out.push_str(tail);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// `fill` of the selected language's text for `key`.
pub fn filled(key: &'static str, values: &[(&str, &str)]) -> String {
    fill(text(key), values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(table: &[(&'static str, &'static str)]) -> Vec<&'static str> {
        let mut keys: Vec<&str> = table.iter().map(|(key, _)| *key).collect();
        keys.sort_unstable();
        keys
    }

    fn placeholders(template: &str) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}') else { break };
            found.push(rest[open + 1..open + close].to_string());
            rest = &rest[open + close + 1..];
        }
        found.sort_unstable();
        found
    }

    fn has_cjk(text: &str) -> bool {
        text.chars().any(|c| matches!(c as u32, 0x3000..=0x303F | 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xFF00..=0xFFEF))
    }

    #[test]
    fn english_and_traditional_chinese_have_the_same_keys() {
        assert_eq!(keys(EN), keys(ZH_TW));
        for table in [EN, ZH_TW] {
            let mut seen = keys(table);
            seen.dedup();
            assert_eq!(seen.len(), table.len(), "a key is listed twice");
        }
    }

    #[test]
    fn a_text_has_the_same_placeholders_in_both_languages() {
        for (key, english) in EN {
            assert_eq!(placeholders(english), placeholders(text_in(Lang::ZhTw, key)), "placeholders of {key}");
        }
    }

    #[test]
    fn english_has_no_cjk_and_no_text_is_empty() {
        for (key, text) in EN {
            assert!(!has_cjk(text), "{key} has CJK in English: {text}");
        }
        for (key, text) in EN.iter().chain(ZH_TW) {
            assert!(!text.trim().is_empty(), "{key} is empty");
        }
    }

    #[test]
    fn no_language_is_english() {
        assert_eq!(Lang::parse("en"), Lang::En);
        assert_eq!(Lang::parse("zh-TW"), Lang::ZhTw);
        assert_eq!(Lang::parse("fr"), Lang::En);
        assert_eq!(Lang::parse(""), Lang::En);
        assert_eq!(Lang::parse("zh-tw"), Lang::En, "only the exact tags the server sends select Chinese");
    }

    #[test]
    fn fill_replaces_each_placeholder_once() {
        assert_eq!(fill("{a} and {b}, {a}", &[("a", "1"), ("b", "{a}")]), "1 and {a}, 1");
        assert_eq!(fill("{missing} {", &[]), "{missing} {");
    }
}
