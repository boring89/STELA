// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Menus (the toolbox's Menu Manager): an application describes its menu
//! bar; the window server draws it and reports choices as
//! `Event::Menu(menu, item)`.

use alloc::string::String;
use alloc::vec::Vec;

/// One menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    /// Its title in the menu bar.
    pub title: String,
    /// Its items; `-` is a separator line.
    pub items: Vec<String>,
}

/// An application's menu bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuBar {
    /// The menus, left to right.
    pub menus: Vec<Menu>,
}

impl MenuBar {
    /// An empty menu bar.
    pub fn new() -> MenuBar {
        MenuBar { menus: Vec::new() }
    }

    /// Adds a menu; returns its index.
    pub fn add(&mut self, title: &str, items: &[&str]) -> usize {
        self.menus.push(Menu {
            title: String::from(title),
            items: items.iter().map(|s| String::from(*s)).collect(),
        });
        self.menus.len() - 1
    }

    /// The text the window server takes (`SET_MENUBAR`): one line per
    /// menu, the title and the items separated by tabs.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for m in &self.menus {
            text.push_str(&m.title);
            for item in &m.items {
                text.push('\t');
                text.push_str(item);
            }
            text.push('\n');
        }
        text
    }

    /// The item's text for a `Event::Menu(menu, item)`.
    pub fn item(&self, menu: u32, item: u32) -> Option<&str> {
        self.menus
            .get(menu as usize)?
            .items
            .get(item as usize)
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_lookup() {
        let mut bar = MenuBar::new();
        assert_eq!(bar.add("File", &["New Folder", "-", "Close"]), 0);
        assert_eq!(bar.add("View", &["Icons", "List"]), 1);
        assert_eq!(
            bar.to_text(),
            "File\tNew Folder\t-\tClose\nView\tIcons\tList\n"
        );
        assert_eq!(bar.item(1, 1), Some("List"));
        assert_eq!(bar.item(2, 0), None);
    }
}
