// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shell command lines with pipes and redirections (P11,
//! `docs/userland.md` §6): `a x | b < in | c >> out &`. Pure logic, tested
//! on the host.
//!
//! ```text
//!  line     = stage { "|" stage } [ "&" ]
//!  stage    = word { word | "<" word | ">" word | ">>" word }
//! ```
//!
//! Words are separated by spaces; `|`, `<`, `>`, `>>` and `&` need no
//! spaces around them. Only the first stage may read a file (`<`) and
//! only the last may write one (`>`, `>>`).

use alloc::string::String;
use alloc::vec::Vec;

/// One program of a pipeline.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stage {
    /// The program and its arguments.
    pub args: Vec<String>,
}

/// A parsed command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pipeline {
    /// The programs, the output of each going to the next.
    pub stages: Vec<Stage>,
    /// `< file`: the first program's input.
    pub input: Option<String>,
    /// `> file` (false) or `>> file` (true): the last program's output.
    pub output: Option<(String, bool)>,
    /// `&`: do not wait.
    pub background: bool,
}

/// Why a line is not a pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Nothing to run (`| a`, `a |`, `&`).
    EmptyStage,
    /// `<` or `>` without a file name.
    MissingFile,
    /// `<` after the first stage, `>` before the last, or twice.
    MisplacedRedirect,
    /// Something after `&`.
    TrailingText,
}

impl ParseError {
    /// A message for the user.
    pub fn describe(self) -> &'static str {
        match self {
            ParseError::EmptyStage => "a program is missing around |",
            ParseError::MissingFile => "a file name is missing after < or >",
            ParseError::MisplacedRedirect => "< goes with the first program, > with the last",
            ParseError::TrailingText => "& must end the line",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    Pipe,
    In,
    Out,
    Append,
    Background,
}

fn tokens(line: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut chars = line.chars().peekable();
    let flush = |word: &mut String, out: &mut Vec<Token>| {
        if !word.is_empty() {
            out.push(Token::Word(core::mem::take(word)));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => flush(&mut word, &mut out),
            '|' => {
                flush(&mut word, &mut out);
                out.push(Token::Pipe);
            }
            '<' => {
                flush(&mut word, &mut out);
                out.push(Token::In);
            }
            '>' => {
                flush(&mut word, &mut out);
                if chars.peek() == Some(&'>') {
                    chars.next();
                    out.push(Token::Append);
                } else {
                    out.push(Token::Out);
                }
            }
            '&' => {
                flush(&mut word, &mut out);
                out.push(Token::Background);
            }
            c => word.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Whether `line` uses any of `| < > &` (else it is a plain command).
pub fn is_pipeline(line: &str) -> bool {
    line.contains(['|', '<', '>', '&'])
}

/// Parses `line`.
pub fn parse(line: &str) -> Result<Pipeline, ParseError> {
    let mut p = Pipeline::default();
    let mut stage = Stage::default();
    let mut toks = tokens(line).into_iter().peekable();
    while let Some(t) = toks.next() {
        if p.background {
            return Err(ParseError::TrailingText);
        }
        match t {
            Token::Word(w) => stage.args.push(w),
            Token::Pipe => {
                if stage.args.is_empty() || p.output.is_some() {
                    return Err(if stage.args.is_empty() {
                        ParseError::EmptyStage
                    } else {
                        ParseError::MisplacedRedirect
                    });
                }
                p.stages.push(core::mem::take(&mut stage));
            }
            Token::In | Token::Out | Token::Append => {
                let Some(Token::Word(file)) = toks.next() else {
                    return Err(ParseError::MissingFile);
                };
                match t {
                    Token::In if p.stages.is_empty() && p.input.is_none() => p.input = Some(file),
                    Token::Out | Token::Append if p.output.is_none() => {
                        p.output = Some((file, t == Token::Append))
                    }
                    _ => return Err(ParseError::MisplacedRedirect),
                }
            }
            Token::Background => {
                if stage.args.is_empty() {
                    return Err(ParseError::EmptyStage);
                }
                p.background = true;
            }
        }
    }
    if stage.args.is_empty() {
        return Err(ParseError::EmptyStage);
    }
    p.stages.push(stage);
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &[&str]) -> Stage {
        Stage {
            args: s.iter().map(|w| String::from(*w)).collect(),
        }
    }

    #[test]
    fn stages_and_redirections() {
        let p = parse("hello a | wc").unwrap();
        assert_eq!(p.stages, [words(&["hello", "a"]), words(&["wc"])]);
        assert_eq!((p.input, p.output, p.background), (None, None, false));
        let p = parse("wc<in.txt|tr x>>out.txt&").unwrap();
        assert_eq!(p.stages, [words(&["wc"]), words(&["tr", "x"])]);
        assert_eq!(p.input.as_deref(), Some("in.txt"));
        assert_eq!(p.output, Some((String::from("out.txt"), true)));
        assert!(p.background);
        let p = parse("hello > Data:/a b").unwrap();
        assert_eq!(p.stages, [words(&["hello", "b"])]);
        assert_eq!(p.output, Some((String::from("Data:/a"), false)));
        assert!(is_pipeline("a | b") && is_pipeline("spin &") && !is_pipeline("ls Data:/"));
    }

    #[test]
    fn mistakes() {
        assert_eq!(parse("| wc"), Err(ParseError::EmptyStage));
        assert_eq!(parse("hello |"), Err(ParseError::EmptyStage));
        assert_eq!(parse("&"), Err(ParseError::EmptyStage));
        assert_eq!(parse("hello >"), Err(ParseError::MissingFile));
        assert_eq!(parse("a | b < x"), Err(ParseError::MisplacedRedirect));
        assert_eq!(parse("a > x | b"), Err(ParseError::MisplacedRedirect));
        assert_eq!(parse("a > x > y"), Err(ParseError::MisplacedRedirect));
        assert_eq!(parse("a & b"), Err(ParseError::TrailingText));
        assert!(!ParseError::TrailingText.describe().is_empty());
    }
}
