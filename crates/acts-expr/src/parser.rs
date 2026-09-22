//! The parser: precedence climbing over the token stream, into an arena of
//! nodes.
//!
//! Nodes reference their children by index rather than by pointer, so a
//! compiled expression is one allocation, has no drop glue to speak of, and
//! can be shared and evaluated from any number of threads.

use crate::lexer::{Lexer, Token};
use crate::{Error, Result};
use std::sync::Arc;

/// How deep operands may nest (`((((...))))`, `!!!!x`, `f(f(f(...)))`). A
/// source is data from a workflow author, not trusted code, and the evaluator
/// walks the tree recursively.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    /// The operator as it reads in the source, for error messages.
    pub(crate) fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Arc<str>),
    Ident(Arc<str>),
    /// `target.name`
    Member(u32, Arc<str>),
    /// `target[index]`
    Index(u32, u32),
    /// `name(args)`
    Call(Arc<str>, Vec<u32>),
    /// `target.name(args)`
    Method(u32, Arc<str>, Vec<u32>),
    /// `!operand`
    Not(u32),
    /// `-operand`
    Neg(u32),
    Binary(BinOp, u32, u32),
}

pub(crate) struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
    current_pos: usize,
    nodes: Vec<Node>,
    depth: usize,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(source: &'a str) -> Result<Self> {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token()?;
        let current_pos = lexer.token_start();

        Ok(Parser {
            lexer,
            current,
            current_pos,
            nodes: Vec::new(),
            depth: 0,
        })
    }

    /// Parse the whole source into `(nodes, root)`.
    pub(crate) fn parse(mut self) -> Result<(Vec<Node>, u32)> {
        let root = self.expression()?;

        if self.current != Token::End {
            return Err(self.unexpected("after the expression"));
        }

        Ok((self.nodes, root))
    }

    fn push(&mut self, node: Node) -> u32 {
        self.nodes.push(node);
        (self.nodes.len() - 1) as u32
    }

    /// Consume the current token and return it.
    fn advance(&mut self) -> Result<Token> {
        let next = self.lexer.next_token()?;
        self.current_pos = self.lexer.token_start();
        Ok(std::mem::replace(&mut self.current, next))
    }

    fn unexpected(&self, expected: &str) -> Error {
        Error::parse(
            format!("expected {expected}, found {}", self.current.describe()),
            self.current_pos,
        )
    }

    fn expect(&mut self, token: Token, expected: &str) -> Result<()> {
        if self.current != token {
            return Err(self.unexpected(expected));
        }
        self.advance()?;
        Ok(())
    }

    /// `||`, the loosest operator.
    fn expression(&mut self) -> Result<u32> {
        let mut left = self.and()?;

        while self.current == Token::OrOr {
            self.advance()?;
            let right = self.and()?;
            left = self.push(Node::Binary(BinOp::Or, left, right));
        }

        Ok(left)
    }

    fn and(&mut self) -> Result<u32> {
        let mut left = self.equality()?;

        while self.current == Token::AndAnd {
            self.advance()?;
            let right = self.equality()?;
            left = self.push(Node::Binary(BinOp::And, left, right));
        }

        Ok(left)
    }

    fn equality(&mut self) -> Result<u32> {
        let mut left = self.comparison()?;

        loop {
            let op = match self.current {
                Token::EqEq => BinOp::Eq,
                Token::NotEq => BinOp::Ne,
                _ => break,
            };
            self.advance()?;
            let right = self.comparison()?;
            left = self.push(Node::Binary(op, left, right));
        }

        Ok(left)
    }

    /// `<`, `<=`, `>` and `>=`, which do not chain: `1 < 2 < 3` reads as a
    /// mistake (the first comparison is a bool) and is rejected here rather
    /// than at evaluation.
    fn comparison(&mut self) -> Result<u32> {
        let left = self.additive()?;

        let op = match self.current {
            Token::Lt => BinOp::Lt,
            Token::Le => BinOp::Le,
            Token::Gt => BinOp::Gt,
            Token::Ge => BinOp::Ge,
            _ => return Ok(left),
        };
        self.advance()?;
        let right = self.additive()?;
        let tree = self.push(Node::Binary(op, left, right));

        if matches!(self.current, Token::Lt | Token::Le | Token::Gt | Token::Ge) {
            return Err(Error::parse(
                format!(
                    "comparison operators do not chain; found {}",
                    self.current.describe()
                ),
                self.current_pos,
            ));
        }

        Ok(tree)
    }

    fn additive(&mut self) -> Result<u32> {
        let mut left = self.multiplicative()?;

        loop {
            let op = match self.current {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance()?;
            let right = self.multiplicative()?;
            left = self.push(Node::Binary(op, left, right));
        }

        Ok(left)
    }

    fn multiplicative(&mut self) -> Result<u32> {
        let mut left = self.unary()?;

        loop {
            let op = match self.current {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::Percent => BinOp::Rem,
                _ => break,
            };
            self.advance()?;
            let right = self.unary()?;
            left = self.push(Node::Binary(op, left, right));
        }

        Ok(left)
    }

    /// `!x` and `-x`, which bind tighter than every binary operator: the
    /// operand is another unary or a postfix expression, never a whole
    /// expression (so `-1 - 2` is `(-1) - 2` and `!a && b` is `(!a) && b`).
    fn unary(&mut self) -> Result<u32> {
        let negate = match self.current {
            Token::Not => false,
            Token::Minus => true,
            _ => return self.postfix(),
        };

        let pos = self.current_pos;
        self.enter(pos)?;
        self.advance()?;
        let operand = self.unary()?;
        self.leave();

        Ok(self.push(if negate {
            Node::Neg(operand)
        } else {
            Node::Not(operand)
        }))
    }

    /// `target.name`, `target[index]` and `name(args)` / `target.name(args)`,
    /// left to right.
    fn postfix(&mut self) -> Result<u32> {
        let mut target = self.primary()?;

        loop {
            match self.current {
                Token::Dot => {
                    self.advance()?;
                    let name = self.member_name()?;
                    target = self.push(Node::Member(target, name));
                }
                Token::LBracket => {
                    let pos = self.current_pos;
                    self.advance()?;
                    let index = self.nested(pos)?;
                    self.expect(Token::RBracket, "']' to close the index")?;
                    target = self.push(Node::Index(target, index));
                }
                Token::LParen => {
                    let pos = self.current_pos;
                    let args = self.arguments()?;
                    // A call needs a name: either the function itself
                    // (`f(x)`) or the member to call on the receiver
                    // (`a.f(x)`), which is how an injected method reads.
                    let called = match &self.nodes[target as usize] {
                        Node::Ident(name) => Some((name.clone(), None)),
                        Node::Member(receiver, name) => Some((name.clone(), Some(*receiver))),
                        _ => None,
                    };
                    target = match called {
                        Some((name, Some(receiver))) => {
                            self.push(Node::Method(receiver, name, args))
                        }
                        Some((name, None)) => self.push(Node::Call(name, args)),
                        None => {
                            return Err(Error::parse(
                                "only a function or a method can be called",
                                pos,
                            ));
                        }
                    };
                }
                _ => break,
            }
        }

        Ok(target)
    }

    /// An operand, one level deeper — the depth a deeply nested source is
    /// rejected at.
    fn nested(&mut self, pos: usize) -> Result<u32> {
        self.enter(pos)?;
        let result = self.expression();
        self.leave();

        result
    }

    fn enter(&mut self, pos: usize) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(Error::parse("the expression nests too deeply", pos));
        }

        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn arguments(&mut self) -> Result<Vec<u32>> {
        self.expect(Token::LParen, "'('")?;

        if self.current == Token::RParen {
            self.advance()?;
            return Ok(Vec::new());
        }

        let mut args = Vec::new();
        loop {
            let pos = self.current_pos;
            args.push(self.nested(pos)?);
            if self.current != Token::Comma {
                break;
            }
            self.advance()?;
        }

        self.expect(Token::RParen, "')' to close the arguments")?;
        Ok(args)
    }

    fn member_name(&mut self) -> Result<Arc<str>> {
        let pos = self.current_pos;

        match self.advance()? {
            Token::Ident(name) => Ok(name),
            // `a.true` and friends: a keyword is not a member name.
            other => Err(Error::parse(
                format!(
                    "expected a member name after '.', found {}",
                    other.describe()
                ),
                pos,
            )),
        }
    }

    fn primary(&mut self) -> Result<u32> {
        let pos = self.current_pos;

        match self.advance()? {
            Token::Int(i) => Ok(self.push(Node::Int(i))),
            Token::Float(f) => Ok(self.push(Node::Float(f))),
            Token::Str(s) => Ok(self.push(Node::Str(s))),
            Token::True => Ok(self.push(Node::Bool(true))),
            Token::False => Ok(self.push(Node::Bool(false))),
            Token::Null => Ok(self.push(Node::Null)),
            Token::Ident(name) => Ok(self.push(Node::Ident(name))),
            Token::LParen => {
                let inner = self.nested(pos)?;
                self.expect(Token::RParen, "')' to close the group")?;
                Ok(inner)
            }
            Token::LBracket => Err(Error::parse(
                "list literals ('[...]') are not supported; inject the list instead",
                pos,
            )),
            other => Err(Error::parse(
                format!("expected a value, found {}", other.describe()),
                pos,
            )),
        }
    }
}
