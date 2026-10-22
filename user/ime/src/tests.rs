// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host tests of the IME: libchewing with the dictionary `make chewing`
//! built in `build/chewing/data`.

use super::*;
use abi::key::{Modifiers, key_for_char};
use std::sync::Mutex;

/// libchewing's C library has one file table: one test at a time.
static LOCK: Mutex<()> = Mutex::new(());

fn data_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/chewing/data")
}

fn start(user: &[u8]) -> Ime {
    let files = DICT_FILES
        .iter()
        .map(|&n| {
            let data = std::fs::read(data_dir().join(n))
                .unwrap_or_else(|e| panic!("{n}: {e} (run `make chewing`)"));
            (n, data)
        })
        .collect();
    Ime::new(files, user).expect("libchewing starts")
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        pressed: true,
        modifiers: Modifiers::default(),
    }
}

/// Types `keys` (ASCII, as on a US keyboard) and collects the commits.
fn type_keys(ime: &mut Ime, keys: &str) -> (String, bool) {
    let mut text = String::new();
    let mut all = true;
    for c in keys.chars() {
        let ev = key_for_char(c).expect("a key");
        let out = ime.key(&ev);
        all &= out.consumed;
        text.push_str(&out.commit);
    }
    (text, all)
}

#[test]
fn su3cl3_is_nihao() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ime = start(&[]);
    assert!(ime.chinese());
    let (text, all) = type_keys(&mut ime, "su3cl3");
    assert!(all, "every key went to the IME");
    assert_eq!(text, "");
    let c = ime.composition();
    assert_eq!(c.preedit, "你好");
    assert_eq!(c.cursor, 2);
    let out = ime.key(&press(KeyCode::Enter));
    assert!(out.consumed);
    assert_eq!(out.commit, "你好");
    assert_eq!(ime.composition().preedit, "");
    assert!(!ime.composing());
}

#[test]
fn zhuyin_shows_while_typing() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ime = start(&[]);
    type_keys(&mut ime, "su");
    let c = ime.composition();
    assert_eq!(c.preedit, "ㄋㄧ");
    assert_eq!(c.cursor, 2);
    // Backspace takes the last symbol back.
    assert!(ime.key(&press(KeyCode::Backspace)).consumed);
    assert_eq!(ime.composition().preedit, "ㄋ");
    // Esc clears everything.
    assert!(ime.key(&press(KeyCode::Escape)).consumed);
    assert!(!ime.composing());
}

#[test]
fn candidates_and_selection() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ime = start(&[]);
    type_keys(&mut ime, "su3");
    // Down (or Space) opens the list of characters for ㄋㄧˇ.
    assert!(ime.key(&press(KeyCode::Down)).consumed);
    let c = ime.composition();
    assert!(c.candidates.len() > 1, "{:?}", c.candidates);
    assert!(c.candidates.iter().any(|s| s == "你"));
    assert!(c.pages >= 1);
    // Choosing the second candidate replaces the character.
    let second = c.candidates[1].clone();
    let (_, all) = type_keys(&mut ime, "2");
    assert!(all);
    assert_eq!(ime.composition().preedit, second);
    assert!(ime.composition().candidates.is_empty());
    assert_eq!(ime.key(&press(KeyCode::Enter)).commit, second);
}

#[test]
fn keys_pass_through_when_idle_or_english() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ime = start(&[]);
    // Nothing composed: editing keys are the application's.
    for code in [
        KeyCode::Enter,
        KeyCode::Backspace,
        KeyCode::Left,
        KeyCode::Tab,
    ] {
        assert_eq!(ime.key(&press(code)), Outcome::default());
    }
    // Alt shortcuts and Ctrl letters too.
    let alt_s = KeyEvent {
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::default()
        },
        ..press(KeyCode::S)
    };
    assert_eq!(ime.key(&alt_s), Outcome::default());
    // Releases.
    let release = KeyEvent {
        pressed: false,
        ..press(KeyCode::S)
    };
    assert!(!ime.key(&release).consumed);

    // Ctrl-Space: English; letters pass through untouched.
    let ctrl_space = KeyEvent {
        modifiers: Modifiers {
            ctrl: true,
            ..Modifiers::default()
        },
        ..press(KeyCode::Space)
    };
    assert!(is_toggle(&ctrl_space));
    assert!(ime.key(&ctrl_space).consumed);
    assert!(!ime.chinese());
    assert!(!ime.composition().chinese);
    let (text, all) = type_keys(&mut ime, "su3");
    assert_eq!(text, "");
    assert!(!all);
    assert!(!ime.composing());
    // And back.
    ime.key(&ctrl_space);
    assert!(ime.chinese());
}

#[test]
fn toggle_commits_the_composition() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ime = start(&[]);
    type_keys(&mut ime, "su3cl3");
    let out = ime.toggle();
    assert_eq!(out.commit, "你好");
    assert!(!ime.chinese());
    assert!(!ime.composing());
}

#[test]
fn user_phrases_are_saved_and_read_back() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let saved = {
        let mut ime = start(&[]);
        // A new user file is made when libchewing starts.
        let first = ime.changed_user_data().expect("a new user file");
        assert!(!first.is_empty());
        assert!(ime.changed_user_data().is_none(), "dirty flag is cleared");
        // Committing a phrase teaches it (the hash back end appends a
        // record).
        type_keys(&mut ime, "su3cl3");
        ime.key(&press(KeyCode::Enter));
        let learned = ime.changed_user_data().expect("learned");
        assert!(
            learned.len() > first.len(),
            "{} {}",
            learned.len(),
            first.len()
        );
        learned
    };
    // A second context reads the file back without rewriting it.
    let mut ime = start(&saved);
    assert!(ime.changed_user_data().is_none());
    type_keys(&mut ime, "su3cl3");
    assert_eq!(ime.key(&press(KeyCode::Enter)).commit, "你好");
}

#[test]
fn one_context_at_a_time() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let ime = start(&[]);
    assert!(matches!(Ime::new(Vec::new(), &[]), Err(InitError::Busy)));
    drop(ime);
    // Missing dictionary files: libchewing does not start.
    assert!(matches!(Ime::new(Vec::new(), &[]), Err(InitError::Chewing)));
    // And the slot is free again.
    drop(start(&[]));
}

#[test]
fn reset_drops_the_composition() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ime = start(&[]);
    type_keys(&mut ime, "su3");
    ime.key(&press(KeyCode::Down));
    assert!(!ime.composition().candidates.is_empty());
    ime.reset();
    assert!(!ime.composing());
    assert_eq!(
        ime.composition(),
        Composition {
            chinese: true,
            ..Composition::default()
        }
    );
    ime.set_chinese(false);
    assert!(!ime.chinese());
}

#[test]
fn settings_parse_and_print() {
    assert_eq!(Settings::parse(""), Settings::default());
    let s = Settings::parse("layout=pinyin\ncandidates=7\njunk\ncandidates=99\n");
    assert_eq!(s.layout, Layout::Pinyin);
    assert_eq!(s.candidates, 7);
    assert_eq!(Settings::parse(&s.text()), s);
    let mut s = Settings::default();
    s.step_layout(true);
    assert_eq!(s.layout, Layout::Eten);
    s.step_layout(false);
    s.step_layout(false);
    assert_eq!(s.layout, Layout::Pinyin);
    s.candidates = 10;
    s.step_candidates(true);
    assert_eq!(s.candidates, 10);
    s.candidates = 4;
    s.step_candidates(false);
    assert_eq!(s.candidates, 4);
}

#[test]
fn pinyin_and_fewer_candidates() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ime = start(&[]);
    ime.apply(&Settings {
        layout: Layout::Pinyin,
        candidates: 5,
    });
    let (text, _) = type_keys(&mut ime, "ni3hao3");
    assert_eq!(text, "");
    assert_eq!(ime.composition().preedit, "你好");
    ime.apply(&Settings {
        layout: Layout::Dachen,
        candidates: 5,
    });
    assert!(!ime.composing(), "apply drops the composition");
    type_keys(&mut ime, "su3");
    ime.key(&press(KeyCode::Down));
    let c = ime.composition();
    assert_eq!(c.candidates.len(), 5);
    assert!(c.pages > 1);
}
