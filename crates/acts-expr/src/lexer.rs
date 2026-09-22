//! The tokenizer: a single forward pass over the source bytes.
//!
//! Identifiers may start with `$` (`$env`, `$get`), so an engine's
//! `$`-prefixed built-ins are ordinary identifiers here and need no rewriting
//! before compilation.

use crate::{Error, Result};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    Int(i64),
    Float(f64),
    Str(Arc<str>),
    Ident(Arc<str>),
    True,
    False,
    Null,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Not,

    Dot,
    Comma,
    LParen,
    RParen,
    LBracket,
    RBracket,
    End,
}

impl Token {
    /// How the token reads in an error message.
    pub(crate) fn describe(&self) -> String {
        match self {
            Token::Int(i) => format!("'{i}'"),
            Token::Float(f) => format!("'{f}'"),
            Token::Str(_) => "a string".to_string(),
            Token::Ident(name) => format!("'{name}'"),
            Token::True => "'true'".to_string(),
            Token::False => "'false'".to_string(),
            Token::Null => "'null'".to_string(),
            Token::End => "the end of the expression".to_string(),
            other => format!("'{}'", other.text()),
        }
    }

    /// The token's source text, for the punctuators.
    pub(crate) fn text(&self) -> &'static str {
        match self {
            Token::Plus => "+",
            Token::Minus => "-",
            Token::Star => "*",
            Token::Slash => "/",
            Token::Percent => "%",
            Token::EqEq => "==",
            Token::NotEq => "!=",
            Token::Lt => "<",
            Token::Le => "<=",
            Token::Gt => ">",
            Token::Ge => ">=",
            Token::AndAnd => "&&",
            Token::OrOr => "||",
            Token::Not => "!",
            Token::Dot => ".",
            Token::Comma => ",",
            Token::LParen => "(",
            Token::RParen => ")",
            Token::LBracket => "[",
            Token::RBracket => "]",
            _ => "",
        }
    }
}

pub(crate) struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    token_start: usize,
}

impl<'a> Lexer<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        Lexer {
            src,
            pos: 0,
            token_start: 0,
        }
    }

    /// The byte offset the token [`Lexer::next_token`] last returned starts
    /// at, which is where an error about that token points.
    pub(crate) fn token_start(&self) -> usize {
        self.token_start
    }

    fn bytes(&self) -> &'a [u8] {
        self.src.as_bytes()
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes().get(self.pos + offset).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(0), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    /// The next token, or [`Token::End`] past the source.
    pub(crate) fn next_token(&mut self) -> Result<Token> {
        self.skip_whitespace();

        let start = self.pos;
        self.token_start = start;
        let Some(c) = self.peek(0) else {
            return Ok(Token::End);
        };

        match c {
            b'0'..=b'9' => self.number(),
            b'\'' | b'"' => self.string(c),
            b'A'..=b'Z' | b'a'..=b'z' | b'_' | b'$' => Ok(self.ident()),
            b'+' => self.punct(Token::Plus),
            b'-' => self.punct(Token::Minus),
            b'*' => self.punct(Token::Star),
            b'/' => self.punct(Token::Slash),
            b'%' => self.punct(Token::Percent),
            b'.' => self.punct(Token::Dot),
            b',' => self.punct(Token::Comma),
            b'(' => self.punct(Token::LParen),
            b')' => self.punct(Token::RParen),
            b'[' => self.punct(Token::LBracket),
            b']' => self.punct(Token::RBracket),
            b'!' => match self.peek(1) {
                Some(b'=') => {
                    self.pos += 2;
                    Ok(Token::NotEq)
                }
                _ => self.punct(Token::Not),
            },
            b'=' => match self.peek(1) {
                Some(b'=') => {
                    self.pos += 2;
                    Ok(Token::EqEq)
                }
                _ => Err(Error::parse(
                    "'=' is not an operator; write '==' to compare",
                    start,
                )),
            },
            b'<' => match self.peek(1) {
                Some(b'=') => {
                    self.pos += 2;
                    Ok(Token::Le)
                }
                _ => self.punct(Token::Lt),
            },
            b'>' => match self.peek(1) {
                Some(b'=') => {
                    self.pos += 2;
                    Ok(Token::Ge)
                }
                _ => self.punct(Token::Gt),
            },
            b'&' if self.peek(1) == Some(b'&') => {
                self.pos += 2;
                Ok(Token::AndAnd)
            }
            b'|' if self.peek(1) == Some(b'|') => {
                self.pos += 2;
                Ok(Token::OrOr)
            }
            // The forms this evaluator deliberately leaves out say so, rather
            // than failing as a stray character.
            b'?' => Err(Error::parse(
                "conditional expressions ('?:') are not supported",
                start,
            )),
            b'{' => Err(Error::parse(
                "object literals ('{...}') are not supported; inject the object instead",
                start,
            )),
            b'}' => Err(Error::parse("unexpected '}'", start)),
            _ => {
                let c = self.src[start..].chars().next().unwrap_or_default();
                Err(Error::parse(format!("unexpected character '{c}'"), start))
            }
        }
    }

    fn punct(&mut self, token: Token) -> Result<Token> {
        self.pos += 1;
        Ok(token)
    }

    fn number(&mut self) -> Result<Token> {
        let start = self.pos;
        let mut float = false;

        while matches!(self.peek(0), Some(b'0'..=b'9')) {
            self.pos += 1;
        }

        // A fraction only if a digit follows the dot, so `1.foo` stays a
        // member access on an int.
        if self.peek(0) == Some(b'.') && matches!(self.peek(1), Some(b'0'..=b'9')) {
            float = true;
            self.pos += 1;
            while matches!(self.peek(0), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        if matches!(self.peek(0), Some(b'e' | b'E')) {
            let sign = matches!(self.peek(1), Some(b'+' | b'-'));
            if matches!(self.peek(if sign { 2 } else { 1 }), Some(b'0'..=b'9')) {
                float = true;
                self.pos += if sign { 2 } else { 1 };
                while matches!(self.peek(0), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
        }

        let text = &self.src[start..self.pos];

        if float {
            return text
                .parse::<f64>()
                .map(Token::Float)
                .map_err(|_| Error::parse(format!("'{text}' is not a number"), start));
        }

        text.parse::<i64>()
            .map(Token::Int)
            .map_err(|_| Error::parse(format!("the int literal '{text}' is out of range"), start))
    }

    fn string(&mut self, quote: u8) -> Result<Token> {
        let start = self.pos;
        self.pos += 1;

        let mut text = String::new();
        loop {
            let Some(c) = self.peek(0) else {
                return Err(Error::parse("unterminated string", start));
            };
            match c {
                c if c == quote => {
                    self.pos += 1;
                    return Ok(Token::Str(Arc::from(text)));
                }
                b'\\' => {
                    self.pos += 1;
                    let escaped = self.peek(0);
                    let Some(escaped) = escaped else {
                        return Err(Error::parse("unterminated string", start));
                    };
                    match escaped {
                        b'n' => text.push('\n'),
                        b'r' => text.push('\r'),
                        b't' => text.push('\t'),
                        b'\\' => text.push('\\'),
                        b'\'' => text.push('\''),
                        b'"' => text.push('"'),
                        b'u' => {
                            let hex_start = self.pos + 1;
                            let end = hex_start + 4;
                            let Some(hex) = self.src.get(hex_start..end) else {
                                return Err(Error::parse(
                                    "a '\\u' escape needs four hex digits",
                                    self.pos - 1,
                                ));
                            };
                            let Ok(code) = u32::from_str_radix(hex, 16) else {
                                return Err(Error::parse(
                                    "a '\\u' escape needs four hex digits",
                                    self.pos - 1,
                                ));
                            };
                            let Some(c) = char::from_u32(code) else {
                                return Err(Error::parse(
                                    format!("'\\u{hex}' is not a character"),
                                    self.pos - 1,
                                ));
                            };
                            text.push(c);
                            self.pos += 4;
                        }
                        other => {
                            return Err(Error::parse(
                                format!("unknown escape '\\{}'", other as char),
                                self.pos - 1,
                            ));
                        }
                    }
                    self.pos += 1;
                }
                _ => {
                    // Copy the whole UTF-8 character, not the byte.
                    let c = self.src[self.pos..].chars().next().unwrap_or_default();
                    text.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    fn ident(&mut self) -> Token {
        let start = self.pos;
        while matches!(
            self.peek(0),
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'$')
        ) {
            self.pos += 1;
        }

        match &self.src[start..self.pos] {
            "true" => Token::True,
            "false" => Token::False,
            "null" => Token::Null,
            name => Token::Ident(Arc::from(name)),
        }
    }
}
