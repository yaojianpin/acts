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

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(source: &str) -> Result<Vec<Token>> {
        let mut lexer = Lexer::new(source);
        let mut tokens = Vec::new();
        loop {
            let token = lexer.next_token()?;
            let end = token == Token::End;
            tokens.push(token);
            if end {
                return Ok(tokens);
            }
        }
    }

    fn token(source: &str) -> Token {
        let tokens = lex(source).unwrap();
        assert_eq!(tokens.len(), 2, "{source}: {tokens:?}");
        tokens[0].clone()
    }

    fn error(source: &str) -> Error {
        lex(source).expect_err(source)
    }

    fn string(token: &Token) -> Arc<str> {
        match token {
            Token::Str(s) => s.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn ident(token: &Token) -> Arc<str> {
        match token {
            Token::Ident(s) => s.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn numbers_are_ints_or_floats() {
        assert_eq!(token("12"), Token::Int(12));
        assert_eq!(token("0"), Token::Int(0));
        assert_eq!(token("007"), Token::Int(7));
        assert_eq!(token("1.5"), Token::Float(1.5));
        assert_eq!(token("0.25"), Token::Float(0.25));
        assert_eq!(token("1e3"), Token::Float(1000.0));
        assert_eq!(token("1E3"), Token::Float(1000.0));
        assert_eq!(token("1e+3"), Token::Float(1000.0));
        assert_eq!(token("1e-1"), Token::Float(0.1));
        assert_eq!(token("2.5e2"), Token::Float(250.0));
    }

    #[test]
    fn a_dot_without_a_digit_is_member_access() {
        assert_eq!(
            lex("1.foo").unwrap(),
            [
                Token::Int(1),
                Token::Dot,
                Token::Ident(Arc::from("foo")),
                Token::End
            ]
        );
        // `1e` is not an exponent either: the `e` is an identifier.
        assert_eq!(
            lex("1e").unwrap(),
            [Token::Int(1), Token::Ident(Arc::from("e")), Token::End]
        );
    }

    #[test]
    fn a_number_out_of_range_is_a_parse_error() {
        let err = error("9223372036854775808");
        assert_eq!(
            err,
            Error::Parse {
                message: "the int literal '9223372036854775808' is out of range".to_string(),
                pos: 0
            }
        );
    }

    #[test]
    fn strings_accept_both_quotes_and_every_escape() {
        assert_eq!(string(&token("'abc'")).as_ref(), "abc");
        assert_eq!(string(&token("\"abc\"")).as_ref(), "abc");
        assert_eq!(string(&token(r"'a\nb'")).as_ref(), "a\nb");
        assert_eq!(string(&token(r"'a\rb'")).as_ref(), "a\rb");
        assert_eq!(string(&token(r"'a\tb'")).as_ref(), "a\tb");
        assert_eq!(string(&token(r"'a\\b'")).as_ref(), "a\\b");
        assert_eq!(string(&token(r"'a\'b'")).as_ref(), "a'b");
        assert_eq!(string(&token(r#""a\"b""#)).as_ref(), "a\"b");
        assert_eq!(string(&token(r"'\u0041'")).as_ref(), "A");
        assert_eq!(string(&token(r"'\u00e9'")).as_ref(), "é");
        // A bare UTF-8 character is copied whole, not byte by byte.
        assert_eq!(string(&token("'héllo'")).as_ref(), "héllo");
        assert_eq!(string(&token("''")).as_ref(), "");
    }

    #[test]
    fn malformed_strings_are_parse_errors() {
        assert_eq!(
            error("'abc"),
            Error::Parse {
                message: "unterminated string".to_string(),
                pos: 0
            }
        );
        assert_eq!(
            error("'abc\\"),
            Error::Parse {
                message: "unterminated string".to_string(),
                pos: 0
            }
        );
        assert_eq!(
            error(r"'\x'"),
            Error::Parse {
                message: "unknown escape '\\x'".to_string(),
                pos: 1
            }
        );
        assert_eq!(
            error(r"'\u12'"),
            Error::Parse {
                message: "a '\\u' escape needs four hex digits".to_string(),
                pos: 1
            }
        );
        assert_eq!(
            error(r"'\uZZZZ'"),
            Error::Parse {
                message: "a '\\u' escape needs four hex digits".to_string(),
                pos: 1
            }
        );
        assert_eq!(
            error(r"'\uD800'"),
            Error::Parse {
                message: "'\\uD800' is not a character".to_string(),
                pos: 1
            }
        );
    }

    #[test]
    fn identifiers_and_keywords() {
        assert_eq!(ident(&token("abc")), Arc::from("abc"));
        assert_eq!(ident(&token("_x1")), Arc::from("_x1"));
        assert_eq!(ident(&token("$env")), Arc::from("$env"));
        assert_eq!(ident(&token("a$b2")), Arc::from("a$b2"));
        assert_eq!(token("true"), Token::True);
        assert_eq!(token("false"), Token::False);
        assert_eq!(token("null"), Token::Null);
    }

    #[test]
    fn punctuators_map_to_their_tokens() {
        let cases = [
            ("+", Token::Plus),
            ("-", Token::Minus),
            ("*", Token::Star),
            ("/", Token::Slash),
            ("%", Token::Percent),
            ("==", Token::EqEq),
            ("!=", Token::NotEq),
            ("<", Token::Lt),
            ("<=", Token::Le),
            (">", Token::Gt),
            (">=", Token::Ge),
            ("&&", Token::AndAnd),
            ("||", Token::OrOr),
            ("!", Token::Not),
            (".", Token::Dot),
            (",", Token::Comma),
            ("(", Token::LParen),
            (")", Token::RParen),
            ("[", Token::LBracket),
            ("]", Token::RBracket),
        ];

        for (source, expected) in cases {
            assert_eq!(token(source), expected, "{source}");
        }
    }

    #[test]
    fn whitespace_is_skipped_and_positions_point_at_the_token() {
        let mut lexer = Lexer::new("  12\t+\n x ");
        assert_eq!(lexer.next_token().unwrap(), Token::Int(12));
        assert_eq!(lexer.token_start(), 2);
        assert_eq!(lexer.next_token().unwrap(), Token::Plus);
        assert_eq!(lexer.token_start(), 5);
        assert_eq!(lexer.next_token().unwrap(), Token::Ident(Arc::from("x")));
        assert_eq!(lexer.token_start(), 8);
        assert_eq!(lexer.next_token().unwrap(), Token::End);
        assert_eq!(lexer.token_start(), 10);
        assert_eq!(lexer.next_token().unwrap(), Token::End);
        assert_eq!(lexer.token_start(), 10);
    }

    #[test]
    fn lone_symbols_say_what_they_are() {
        assert_eq!(
            error("="),
            Error::Parse {
                message: "'=' is not an operator; write '==' to compare".to_string(),
                pos: 0
            }
        );
        assert_eq!(
            error("&"),
            Error::Parse {
                message: "unexpected character '&'".to_string(),
                pos: 0
            }
        );
        assert_eq!(
            error("|"),
            Error::Parse {
                message: "unexpected character '|'".to_string(),
                pos: 0
            }
        );
        assert_eq!(
            error("~"),
            Error::Parse {
                message: "unexpected character '~'".to_string(),
                pos: 0
            }
        );
        assert_eq!(
            error("é"),
            Error::Parse {
                message: "unexpected character 'é'".to_string(),
                pos: 0
            }
        );
        assert_eq!(
            error("?"),
            Error::Parse {
                message: "conditional expressions ('?:') are not supported".to_string(),
                pos: 0
            }
        );
        assert_eq!(
            error("{"),
            Error::Parse {
                message: "object literals ('{...}') are not supported; inject the object instead"
                    .to_string(),
                pos: 0
            }
        );
        assert_eq!(
            error("}"),
            Error::Parse {
                message: "unexpected '}'".to_string(),
                pos: 0
            }
        );
        assert_eq!(
            error(" ="),
            Error::Parse {
                message: "'=' is not an operator; write '==' to compare".to_string(),
                pos: 1
            }
        );
    }

    #[test]
    fn describe_and_text_read_as_the_source_does() {
        assert_eq!(Token::Int(1).describe(), "'1'");
        assert_eq!(Token::Float(1.5).describe(), "'1.5'");
        assert_eq!(string(&Token::Str(Arc::from("x"))).as_ref(), "x");
        assert_eq!(Token::Str(Arc::from("x")).describe(), "a string");
        assert_eq!(Token::Ident(Arc::from("x")).describe(), "'x'");
        assert_eq!(Token::True.describe(), "'true'");
        assert_eq!(Token::False.describe(), "'false'");
        assert_eq!(Token::Null.describe(), "'null'");
        assert_eq!(Token::End.describe(), "the end of the expression");
        assert_eq!(Token::Plus.describe(), "'+'");

        let punctuators = [
            (Token::Plus, "+"),
            (Token::Minus, "-"),
            (Token::Star, "*"),
            (Token::Slash, "/"),
            (Token::Percent, "%"),
            (Token::EqEq, "=="),
            (Token::NotEq, "!="),
            (Token::Lt, "<"),
            (Token::Le, "<="),
            (Token::Gt, ">"),
            (Token::Ge, ">="),
            (Token::AndAnd, "&&"),
            (Token::OrOr, "||"),
            (Token::Not, "!"),
            (Token::Dot, "."),
            (Token::Comma, ","),
            (Token::LParen, "("),
            (Token::RParen, ")"),
            (Token::LBracket, "["),
            (Token::RBracket, "]"),
        ];

        for (token, expected) in punctuators {
            assert_eq!(token.text(), expected);
        }
        assert_eq!(Token::Int(1).text(), "");
        assert_eq!(Token::Str(Arc::from("x")).text(), "");
        assert_eq!(Token::End.text(), "");
    }

    #[test]
    fn an_empty_source_is_only_the_end_token() {
        assert_eq!(lex("").unwrap(), [Token::End]);
        assert_eq!(lex("   ").unwrap(), [Token::End]);
    }
}
