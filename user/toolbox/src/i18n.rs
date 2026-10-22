// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The user-interface language (P12, `docs/setup.md` §3): English or
//! Traditional Chinese. A text is looked up by its English form, so code
//! reads naturally (`tr("Shut Down")`) and a text without a translation
//! stays English. Setup.app and Mouseion's menus are translated so far.
//!
//! The choice is kept in `Data:/.stela/language` (`en` or `zh-TW`);
//! [`load`] reads it.

use core::sync::atomic::{AtomicU8, Ordering};

/// The languages STELA's interface has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    /// English (the texts as written).
    English,
    /// 繁體中文.
    TraditionalChinese,
}

impl Language {
    /// The code stored in `Data:/.stela/language`.
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::TraditionalChinese => "zh-TW",
        }
    }

    /// The language with `code` (`en`, `zh-TW`, also `zh`).
    pub fn from_code(code: &str) -> Option<Language> {
        match code.trim() {
            "en" => Some(Language::English),
            "zh-TW" | "zh" => Some(Language::TraditionalChinese),
            _ => None,
        }
    }

    /// Its own name.
    pub fn name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::TraditionalChinese => "繁體中文",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Chooses the language for [`tr`].
pub fn set(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

/// The language [`tr`] uses.
pub fn current() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::TraditionalChinese,
        _ => Language::English,
    }
}

/// Where the choice is kept.
pub const LANGUAGE_FILE: &str = "Data:/.stela/language";

/// Sets the language from [`LANGUAGE_FILE`] (English without it).
#[cfg(target_os = "none")]
pub fn load() {
    if let Ok(data) = libstela::fs::read_path(LANGUAGE_FILE)
        && let Some(l) = core::str::from_utf8(&data)
            .ok()
            .and_then(Language::from_code)
    {
        set(l);
    }
}

/// English → Traditional Chinese.
const ZH_TW: &[(&str, &str)] = &[
    // Mouseion's menus.
    ("About Mouseion", "關於 Mouseion"),
    ("About STELA", "關於 STELA"),
    ("File", "檔案"),
    ("New Folder", "新增資料夾"),
    ("Open", "打開"),
    ("Get Info", "簡介"),
    ("Close", "關閉"),
    ("Edit", "編輯"),
    ("View", "顯示方式"),
    ("Icons", "圖示"),
    ("List", "列表"),
    ("Special", "特殊"),
    ("Terminal", "終端機"),
    ("Monitor", "監視器"),
    ("Empty Trash...", "清空垃圾桶…"),
    ("Restart", "重新開機"),
    ("Shut Down", "關機"),
    // Setup.app.
    ("Welcome to STELA", "歡迎使用 STELA"),
    ("Language", "語言"),
    (
        "Choose a language with the arrow keys, then press Enter.",
        "用方向鍵選擇語言，然後按 Enter。",
    ),
    ("Time zone", "時區"),
    (
        "Type the offset from UTC (for example +08:00), then press Enter.",
        "輸入與 UTC 的時差（例如 +08:00），然後按 Enter。",
    ),
    ("Data volume", "Data 卷宗"),
    (
        "Your files and programs live on the Data volume.",
        "你的檔案與程式放在 Data 卷宗。",
    ),
    ("Found the Data volume:", "找到 Data 卷宗："),
    (
        "No Data volume. Empty places that can become one:",
        "沒有 Data 卷宗。可以變成 Data 的空白位置：",
    ),
    (
        "Nothing can become a Data volume.",
        "沒有可以變成 Data 卷宗的地方。",
    ),
    (
        "Erases it. Type its name to confirm, then press Enter.",
        "會清除它。輸入它的名稱確認，然後按 Enter。",
    ),
    ("Formatting...", "格式化中…"),
    ("Network", "網路"),
    ("Address", "位址"),
    ("DNS server", "DNS 伺服器"),
    ("Package server", "套件伺服器"),
    ("not configured", "沒有設定"),
    ("reachable", "連得到"),
    ("Packages", "套件"),
    (
        "Space selects, Enter installs the selected ones (or goes on).",
        "空白鍵選擇，Enter 安裝選擇的套件（或繼續）。",
    ),
    ("Installing...", "安裝中…"),
    ("installed", "已安裝"),
    ("Classic", "Classic"),
    (
        "Mac ROM and System disk images on other volumes:",
        "其他卷宗上的 Mac ROM 與 System 磁碟映像：",
    ),
    (
        "Space selects, Enter copies them to Data:/Classic/ (or goes on).",
        "空白鍵選擇，Enter 把它們複製到 Data:/Classic/（或繼續）。",
    ),
    ("None found. Press Enter.", "沒有找到。按 Enter。"),
    // P14 K7: the install page.
    ("Install to disk", "安裝到磁碟"),
    (
        "STELA can be copied to a whole empty disk (ESP, System, Data).",
        "可以把 STELA 複製到一整顆空白的磁碟（ESP、System、Data）。",
    ),
    (
        "Type the disk's name to install there, or press Enter to go on.",
        "輸入磁碟的名稱安裝到那裡，或按 Enter 繼續。",
    ),
    (
        "installed; start from it without the installer",
        "已安裝；拿掉安裝用的磁碟後從它開機",
    ),
    // P14 K2: the display page.
    ("Display", "顯示"),
    (
        "Up and down choose a line, left and right change it.",
        "上下選一行，左右改變它。",
    ),
    ("Interface scale", "介面倍數"),
    ("automatic", "自動"),
    ("Sleep after", "熄屏時間"),
    ("never", "永不"),
    ("minutes", "分鐘"),
    ("Desktop pattern", "桌面圖樣"),
    ("plain", "素色"),
    ("meander", "回紋"),
    ("waves", "波浪"),
    ("(from the next start)", "（下次開機起）"),
    ("Done", "完成"),
    (
        "STELA is ready. Press Enter to start.",
        "STELA 準備好了。按 Enter 開始。",
    ),
    ("Enter: next   Esc: back", "Enter：下一步   Esc：上一步"),
    ("Error", "錯誤"),
];

/// `text` in the current language (unchanged without a translation).
pub fn tr(text: &str) -> &str {
    if current() == Language::English {
        return text;
    }
    ZH_TW
        .iter()
        .find(|(en, _)| *en == text)
        .map_or(text, |(_, zh)| zh)
}

/// A menu item (`Label^K` keeps its shortcut).
pub fn tr_menu(item: &str) -> alloc::string::String {
    match item.split_once('^') {
        Some((label, key)) => alloc::format!("{}^{key}", tr(label)),
        None => alloc::string::String::from(tr(item)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages() {
        assert_eq!(
            Language::from_code("zh-TW"),
            Some(Language::TraditionalChinese)
        );
        assert_eq!(Language::from_code(" en\n"), Some(Language::English));
        assert_eq!(Language::from_code("fr"), None);
        set(Language::TraditionalChinese);
        assert_eq!(tr("Shut Down"), "關機");
        assert_eq!(tr("not in the table"), "not in the table");
        assert_eq!(tr_menu("Open^O"), "打開^O");
        set(Language::English);
        assert_eq!(tr("Shut Down"), "Shut Down");
        assert_eq!(tr_menu("Close^W"), "Close^W");
        assert_eq!(Language::TraditionalChinese.code(), "zh-TW");
    }
}
