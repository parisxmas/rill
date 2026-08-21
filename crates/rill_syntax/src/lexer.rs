//! Lexer for Rill, producing a token stream with layout (Indent/Dedent/Newline)
//! tokens computed from indentation, Python-style. Layout is suppressed inside
//! parentheses so expressions can span lines freely.

use crate::SyntaxError;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    // keywords
    Fn,
    If,
    Then,
    Else,
    Type,
    Match,
    Spawn,
    Select,
    Import,
    Trait,
    Impl,
    Extern,
    Export,
    For,
    // literals / names
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    True,
    False,
    // operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    /// `|` on its own, which separates the alternatives of one pattern.
    Pipe,
    /// `@`, which names the whole of what a pattern matched.
    At,
    /// `{` and `}`, which appear in one place: `{ p | x = 9 }`.
    LBrace,
    RBrace,
    PipeGt,
    Arrow,
    Colon,
    Backslash,
    AmpAmp,
    PipePipe,
    Bang,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Hash,
    Question,
    Dot,
    DotDot,
    OpEq(char),
    Comma,
    // layout
    Newline,
    Indent,
    Dedent,
    Eof,
}

impl std::fmt::Display for Tok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tok::Fn => write!(f, "`fn`"),
            Tok::If => write!(f, "`if`"),
            Tok::Then => write!(f, "`then`"),
            Tok::Else => write!(f, "`else`"),
            Tok::Type => write!(f, "`type`"),
            Tok::Match => write!(f, "`match`"),
            Tok::Spawn => write!(f, "`spawn`"),
            Tok::Select => write!(f, "`select`"),
            Tok::Import => write!(f, "`import`"),
            Tok::Trait => write!(f, "`trait`"),
            Tok::Impl => write!(f, "`impl`"),
            Tok::Extern => write!(f, "`extern`"),
            Tok::Export => write!(f, "`export`"),
            Tok::Ident(s) => write!(f, "`{s}`"),
            Tok::Int(v) => write!(f, "`{v}`"),
            Tok::Float(v) => write!(f, "`{v}`"),
            Tok::Str(_) => write!(f, "string literal"),
            Tok::True => write!(f, "`true`"),
            Tok::False => write!(f, "`false`"),
            Tok::Plus => write!(f, "`+`"),
            Tok::Minus => write!(f, "`-`"),
            Tok::Star => write!(f, "`*`"),
            Tok::Slash => write!(f, "`/`"),
            Tok::Percent => write!(f, "`%`"),
            Tok::EqEq => write!(f, "`==`"),
            Tok::Ne => write!(f, "`!=`"),
            Tok::Lt => write!(f, "`<`"),
            Tok::Le => write!(f, "`<=`"),
            Tok::Gt => write!(f, "`>`"),
            Tok::Ge => write!(f, "`>=`"),
            Tok::Eq => write!(f, "`=`"),
            Tok::At => write!(f, "`@`"),
            Tok::LBrace => write!(f, "`{{`"),
            Tok::RBrace => write!(f, "`}}`"),
            Tok::Pipe => write!(f, "`|`"),
            Tok::PipeGt => write!(f, "`|>`"),
            Tok::Arrow => write!(f, "`->`"),
            Tok::Colon => write!(f, "`:`"),
            Tok::Backslash => write!(f, "`\\`"),
            Tok::AmpAmp => write!(f, "`&&`"),
            Tok::PipePipe => write!(f, "`||`"),
            Tok::Bang => write!(f, "`!`"),
            Tok::LParen => write!(f, "`(`"),
            Tok::RParen => write!(f, "`)`"),
            Tok::LBracket => write!(f, "`[`"),
            Tok::RBracket => write!(f, "`]`"),
            Tok::Hash => write!(f, "`#`"),
            Tok::Question => write!(f, "`?`"),
            Tok::Dot => write!(f, "`.`"),
            Tok::DotDot => write!(f, "`..`"),
            Tok::OpEq(c) => write!(f, "`{c}=`"),
            Tok::For => write!(f, "`for`"),
            Tok::Comma => write!(f, "`,`"),
            Tok::Newline => write!(f, "end of line"),
            Tok::Indent => write!(f, "indent"),
            Tok::Dedent => write!(f, "dedent"),
            Tok::Eof => write!(f, "end of file"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
    pub col: usize,
}

/// A `#` comment, kept out of the token stream but recorded for the
/// formatter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub line: usize,
    pub col: usize,
    /// Includes the leading `#`, with trailing whitespace removed.
    pub text: String,
    /// Nothing but whitespace precedes it on its line.
    pub own_line: bool,
}

/// Whether the `#` under the scanner opens a comment rather than asking for
/// a length: a space, the end of the line or the end of the file follows it.
fn comment_here(s: &Scanner<'_>) -> bool {
    let mut ahead = s.chars.clone();
    ahead.next();
    matches!(ahead.peek(), None | Some(' ') | Some('\n') | Some('\r'))
}

struct Scanner<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    line: usize,
    col: usize,
}

impl<'a> Scanner<'a> {
    fn new(src: &'a str) -> Self {
        Scanner { chars: src.chars().peekable(), line: 1, col: 0 }
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        if c == '\n' {
            self.line += 1;
            self.col = 0;
        } else {
            self.col += 1;
        }
        Some(c)
    }
}

pub fn lex(src: &str) -> Result<Vec<Token>, SyntaxError> {
    lex_with_comments(src).map(|(toks, _)| toks)
}

pub fn lex_with_comments(src: &str) -> Result<(Vec<Token>, Vec<Comment>), SyntaxError> {
    let mut s = Scanner::new(src);
    let mut toks: Vec<Token> = Vec::new();
    let mut comments: Vec<Comment> = Vec::new();
    let mut indents: Vec<usize> = vec![0];
    let mut paren_depth: usize = 0;
    let mut last_tok_line: usize = 0; // 0 = no token emitted yet

    loop {
        // Skip whitespace and comments; tabs are not allowed in indentation.
        loop {
            match s.peek() {
                Some(' ') | Some('\n') | Some('\r') => {
                    s.bump();
                }
                Some('\t') => {
                    return Err(SyntaxError::new(
                        s.line,
                        s.col,
                        "tabs are not allowed; use spaces".to_string(),
                    ));
                }
                // `#` starts a comment when a space or the end of the line
                // follows it; written against something, it is the length
                // operator. Every comment in the language is written `# ...`.
                Some('#') if !comment_here(&s) => break,
                Some('#') => {
                    let (cline, ccol) = (s.line, s.col);
                    let mut text = String::new();
                    while let Some(c) = s.peek() {
                        if c == '\n' {
                            break;
                        }
                        text.push(c);
                        s.bump();
                    }
                    comments.push(Comment {
                        line: cline,
                        col: ccol,
                        text: text.trim_end().to_string(),
                        own_line: last_tok_line != cline,
                    });
                }
                _ => break,
            }
        }

        let (line, col) = (s.line, s.col);
        let c = match s.peek() {
            None => break,
            Some(c) => c,
        };

        // Layout: first token of a new line (outside parens) decides
        // Newline/Indent/Dedent relative to the indent stack.
        if paren_depth == 0 && line != last_tok_line {
            let top = *indents.last().unwrap();
            if last_tok_line == 0 {
                if col != 0 {
                    return Err(SyntaxError::new(
                        line,
                        col,
                        "first line must not be indented".to_string(),
                    ));
                }
            } else if col > top {
                indents.push(col);
                toks.push(Token { tok: Tok::Indent, line, col });
            } else {
                // Dedents come before the end-of-line marker so that a block
                // closing here consumes only its Dedent, leaving the Newline
                // to separate statements in the block that contains it.
                while col < *indents.last().unwrap() {
                    indents.pop();
                    toks.push(Token { tok: Tok::Dedent, line, col });
                }
                if col != *indents.last().unwrap() {
                    return Err(SyntaxError::new(
                        line,
                        col,
                        "indentation does not match any enclosing block".to_string(),
                    ));
                }
                toks.push(Token { tok: Tok::Newline, line, col });
            }
        }
        last_tok_line = line;

        let tok = match c {
            '0'..='9' => {
                let mut text = String::new();
                while let Some(c) = s.peek() {
                    if c.is_ascii_digit() {
                        text.push(c);
                        s.bump();
                    } else {
                        break;
                    }
                }
                let mut is_float = false;
                if s.peek() == Some('.') {
                    // Only a float if a digit follows the dot.
                    let mut ahead = s.chars.clone();
                    ahead.next();
                    if ahead.peek().is_some_and(|c| c.is_ascii_digit()) {
                        is_float = true;
                        text.push('.');
                        s.bump();
                        while let Some(c) = s.peek() {
                            if c.is_ascii_digit() {
                                text.push(c);
                                s.bump();
                            } else {
                                break;
                            }
                        }
                    }
                }
                if is_float {
                    Tok::Float(text.parse().map_err(|_| {
                        SyntaxError::new(line, col, format!("invalid float literal `{text}`"))
                    })?)
                } else {
                    Tok::Int(text.parse().map_err(|_| {
                        SyntaxError::new(line, col, format!("integer literal `{text}` is too large"))
                    })?)
                }
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut text = String::new();
                loop {
                    while let Some(c) = s.peek() {
                        if c.is_ascii_alphanumeric() || c == '_' {
                            text.push(c);
                            s.bump();
                        } else {
                            break;
                        }
                    }
                    // A dot joins a module alias to the name it qualifies —
                    // `geo.area` is one identifier, since Rill has no field
                    // access for `.` to mean anything else.
                    let mut ahead = s.chars.clone();
                    if s.peek() == Some('.')
                        && {
                            ahead.next();
                            ahead.peek().is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
                        }
                    {
                        text.push('.');
                        s.bump();
                    } else {
                        break;
                    }
                }
                match text.as_str() {
                    "for" => Tok::For,
                    "fn" => Tok::Fn,
                    "if" => Tok::If,
                    "then" => Tok::Then,
                    "else" => Tok::Else,
                    "type" => Tok::Type,
                    "match" => Tok::Match,
                    "spawn" => Tok::Spawn,
                    "select" => Tok::Select,
                    "import" => Tok::Import,
                    "trait" => Tok::Trait,
                    "impl" => Tok::Impl,
                    "extern" => Tok::Extern,
                    "export" => Tok::Export,
                    "true" => Tok::True,
                    "false" => Tok::False,
                    _ => Tok::Ident(text),
                }
            }
            '"' => {
                s.bump();
                let mut text = String::new();
                loop {
                    match s.bump() {
                        None | Some('\n') => {
                            return Err(SyntaxError::new(
                                line,
                                col,
                                "unterminated string literal".to_string(),
                            ));
                        }
                        Some('"') => break,
                        Some('\\') => match s.bump() {
                            Some('n') => text.push('\n'),
                            Some('r') => text.push('\r'),
                            Some('t') => text.push('\t'),
                            Some('\\') => text.push('\\'),
                            Some('"') => text.push('"'),
                            other => {
                                return Err(SyntaxError::new(
                                    line,
                                    col,
                                    format!(
                                        "unknown escape `\\{}`",
                                        other.map(String::from).unwrap_or_default()
                                    ),
                                ));
                            }
                        },
                        Some(c) => text.push(c),
                    }
                }
                Tok::Str(text)
            }
            _ => {
                s.bump();
                match c {
                    '+' => {
                        if s.peek() == Some('=') {
                            s.bump();
                            Tok::OpEq('+')
                        } else {
                            Tok::Plus
                        }
                    }
                    '-' => {
                        if s.peek() == Some('>') {
                            s.bump();
                            Tok::Arrow
                        } else if s.peek() == Some('=') {
                            s.bump();
                            Tok::OpEq('-')
                        } else {
                            Tok::Minus
                        }
                    }
                    '*' => {
                        if s.peek() == Some('=') {
                            s.bump();
                            Tok::OpEq('*')
                        } else {
                            Tok::Star
                        }
                    }
                    '/' => {
                        if s.peek() == Some('=') {
                            s.bump();
                            Tok::OpEq('/')
                        } else {
                            Tok::Slash
                        }
                    }
                    '#' => Tok::Hash,
                    '?' => Tok::Question,
                    '.' => {
                        if s.peek() == Some('.') {
                            s.bump();
                            Tok::DotDot
                        } else {
                            // A dot against a name is part of it — the rule
                            // above joins `geo.area` and `p.x` alike, and the
                            // checker tells a module from a record. This is
                            // the other one: a dot after something that is not
                            // a name, as in `f(x).y`.
                            Tok::Dot
                        }
                    }
                    '%' => Tok::Percent,
                    '@' => Tok::At,
                    '{' => Tok::LBrace,
                    '}' => Tok::RBrace,
                    ',' => Tok::Comma,
                    ':' => Tok::Colon,
                    '\\' => Tok::Backslash,
                    '(' => {
                        paren_depth += 1;
                        Tok::LParen
                    }
                    ')' => {
                        paren_depth = paren_depth.saturating_sub(1);
                        Tok::RParen
                    }
                    // Brackets join the paren count, so an index expression may
                    // span lines for the same reason a call may.
                    '[' => {
                        paren_depth += 1;
                        Tok::LBracket
                    }
                    ']' => {
                        paren_depth = paren_depth.saturating_sub(1);
                        Tok::RBracket
                    }
                    '=' => {
                        if s.peek() == Some('=') {
                            s.bump();
                            Tok::EqEq
                        } else {
                            Tok::Eq
                        }
                    }
                    '!' => {
                        if s.peek() == Some('=') {
                            s.bump();
                            Tok::Ne
                        } else {
                            Tok::Bang
                        }
                    }
                    '&' => {
                        if s.peek() == Some('&') {
                            s.bump();
                            Tok::AmpAmp
                        } else {
                            return Err(SyntaxError::new(
                                line,
                                col,
                                "unexpected `&`; did you mean `&&`?".to_string(),
                            ));
                        }
                    }
                    '<' => {
                        if s.peek() == Some('=') {
                            s.bump();
                            Tok::Le
                        } else {
                            Tok::Lt
                        }
                    }
                    '>' => {
                        if s.peek() == Some('=') {
                            s.bump();
                            Tok::Ge
                        } else {
                            Tok::Gt
                        }
                    }
                    '|' => {
                        if s.peek() == Some('>') {
                            s.bump();
                            Tok::PipeGt
                        } else if s.peek() == Some('|') {
                            s.bump();
                            Tok::PipePipe
                        } else {
                            Tok::Pipe
                        }
                    }
                    other => {
                        return Err(SyntaxError::new(
                            line,
                            col,
                            format!("unexpected character `{other}`"),
                        ));
                    }
                }
            }
        };
        toks.push(Token { tok, line, col });
    }

    // Close any open blocks at end of file.
    let eof_line = s.line;
    while indents.len() > 1 {
        indents.pop();
        toks.push(Token { tok: Tok::Dedent, line: eof_line, col: 0 });
    }
    if last_tok_line != 0 {
        toks.push(Token { tok: Tok::Newline, line: eof_line, col: 0 });
    }
    toks.push(Token { tok: Tok::Eof, line: eof_line, col: 0 });
    Ok((toks, comments))
}
