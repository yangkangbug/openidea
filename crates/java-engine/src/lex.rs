//! Java lexer for the syntax subset. Tokens are lossless, including
//! whitespace and comments, so an edit can be compared against a full lex.

use crate::kind::SyntaxKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub start: u32,
    pub end: u32,
}

pub fn lex(text: &str) -> Vec<Token> {
    let mut lexer = Lexer { text, i: 0 };
    let mut out = Vec::new();
    while lexer.i < lexer.text.len() {
        out.push(lexer.next_token());
    }
    out
}

struct Lexer<'a> {
    text: &'a str,
    i: usize,
}

impl<'a> Lexer<'a> {
    fn next_token(&mut self) -> Token {
        let start = self.i;
        let ch = self.bump_char().expect("token at end");
        let kind = match ch {
            c if c.is_whitespace() => {
                self.eat_while(|c| c.is_whitespace());
                SyntaxKind::Whitespace
            }
            '/' if self.peek() == Some('/') => {
                self.bump_char();
                self.eat_while(|c| c != '\n');
                SyntaxKind::LineComment
            }
            '/' if self.peek() == Some('*') => self.block_comment(),
            '"' if self.starts_with("\"\"") => self.text_block(),
            '"' => self.string_or_char('"', SyntaxKind::StringLiteral),
            '\'' => self.string_or_char('\'', SyntaxKind::CharLiteral),
            c if is_ident_start(c) => self.ident(start),
            '.' if self.peek().is_some_and(|c| c.is_ascii_digit()) => self.number(start, true),
            c if c.is_ascii_digit() => self.number(start, false),
            _ => self.punctuation(start, ch),
        };
        Token { kind, start: start as u32, end: self.i as u32 }
    }

    fn ident(&mut self, start: usize) -> SyntaxKind {
        self.eat_while(is_ident_continue);
        if &self.text[start..self.i] == "non" && self.starts_with("-sealed") {
            self.i += "-sealed".len();
            return SyntaxKind::NonSealed;
        }
        SyntaxKind::keyword(&self.text[start..self.i]).unwrap_or(SyntaxKind::Ident)
    }

    fn block_comment(&mut self) -> SyntaxKind {
        self.bump_char();
        while self.i < self.text.len() {
            if self.starts_with("*/") {
                self.i += 2;
                return SyntaxKind::BlockComment;
            }
            self.bump_char();
        }
        SyntaxKind::ErrorToken
    }

    fn text_block(&mut self) -> SyntaxKind {
        self.i += 2;
        while self.i < self.text.len() {
            if self.starts_with("\"\"\"") {
                self.i += 3;
                return SyntaxKind::TextBlock;
            }
            if self.peek() == Some('\\') {
                self.bump_char();
                self.bump_char();
                continue;
            }
            self.bump_char();
        }
        SyntaxKind::ErrorToken
    }

    fn string_or_char(&mut self, quote: char, kind: SyntaxKind) -> SyntaxKind {
        while let Some(c) = self.peek() {
            if c == '\n' || c == '\r' {
                return SyntaxKind::ErrorToken;
            }
            self.bump_char();
            if c == '\\' {
                self.bump_char();
                continue;
            }
            if c == quote {
                return kind;
            }
        }
        SyntaxKind::ErrorToken
    }

    fn number(&mut self, start: usize, leading_dot: bool) -> SyntaxKind {
        if !leading_dot && (self.starts_with("x") || self.starts_with("X")) && self.text[start..].starts_with('0') {
            self.bump_char();
            let digits = self.eat_digits(|c| c.is_ascii_hexdigit());
            if digits == 0 {
                return SyntaxKind::ErrorToken;
            }
            return self.suffix(false);
        }
        if !leading_dot && (self.starts_with("b") || self.starts_with("B")) && self.text[start..].starts_with('0') {
            self.bump_char();
            let digits = self.eat_digits(|c| c == '0' || c == '1');
            if digits == 0 {
                return SyntaxKind::ErrorToken;
            }
            return self.suffix(false);
        }
        if !leading_dot {
            self.eat_digits(|c| c.is_ascii_digit());
        }
        let mut real = leading_dot;
        if !leading_dot && self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump_char();
            self.eat_digits(|c| c.is_ascii_digit());
            real = true;
        } else if leading_dot {
            self.eat_digits(|c| c.is_ascii_digit());
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let saved = self.i;
            self.bump_char();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump_char();
            }
            if self.eat_digits(|c| c.is_ascii_digit()) == 0 {
                self.i = saved;
            } else {
                real = true;
            }
        }
        self.suffix(real)
    }

    fn eat_digits(&mut self, digit: impl Fn(char) -> bool) -> usize {
        let mut n = 0;
        loop {
            match self.peek() {
                Some('_') => {
                    self.bump_char();
                }
                Some(c) if digit(c) => {
                    self.bump_char();
                    n += 1;
                }
                _ => break,
            }
        }
        n
    }

    fn suffix(&mut self, real: bool) -> SyntaxKind {
        match self.peek() {
            Some('f' | 'F') => {
                self.bump_char();
                SyntaxKind::FloatLiteral
            }
            Some('d' | 'D') => {
                self.bump_char();
                SyntaxKind::DoubleLiteral
            }
            Some('l' | 'L') if !real => {
                self.bump_char();
                SyntaxKind::LongLiteral
            }
            _ if real => SyntaxKind::DoubleLiteral,
            _ => SyntaxKind::IntLiteral,
        }
    }

    fn punctuation(&mut self, _start: usize, ch: char) -> SyntaxKind {
        let two = self.peek();
        let three = self.peek_at(1);
        match (ch, two, three) {
            ('.', Some('.'), Some('.')) => {
                self.i += 2;
                SyntaxKind::DotDotDot
            }
            ('-', Some('>'), _) => {
                self.bump_char();
                SyntaxKind::Arrow
            }
            (':', Some(':'), _) => {
                self.bump_char();
                SyntaxKind::ColonColon
            }
            ('=', Some('='), _) => {
                self.bump_char();
                SyntaxKind::EqEq
            }
            ('!', Some('='), _) => {
                self.bump_char();
                SyntaxKind::NotEq
            }
            ('+', Some('+'), _) => {
                self.bump_char();
                SyntaxKind::PlusPlus
            }
            ('+', Some('='), _) => {
                self.bump_char();
                SyntaxKind::PlusEq
            }
            ('-', Some('-'), _) => {
                self.bump_char();
                SyntaxKind::MinusMinus
            }
            ('-', Some('='), _) => {
                self.bump_char();
                SyntaxKind::MinusEq
            }
            ('*', Some('='), _) => {
                self.bump_char();
                SyntaxKind::StarEq
            }
            ('/', Some('='), _) => {
                self.bump_char();
                SyntaxKind::SlashEq
            }
            ('%', Some('='), _) => {
                self.bump_char();
                SyntaxKind::PercentEq
            }
            ('&', Some('&'), _) => {
                self.bump_char();
                SyntaxKind::AmpAmp
            }
            ('&', Some('='), _) => {
                self.bump_char();
                SyntaxKind::AmpEq
            }
            ('|', Some('|'), _) => {
                self.bump_char();
                SyntaxKind::PipePipe
            }
            ('|', Some('='), _) => {
                self.bump_char();
                SyntaxKind::PipeEq
            }
            ('^', Some('='), _) => {
                self.bump_char();
                SyntaxKind::CaretEq
            }
            ('<', Some('<'), Some('=')) => {
                self.i += 2;
                SyntaxKind::LtLtEq
            }
            ('<', Some('<'), _) => {
                self.bump_char();
                SyntaxKind::LtLt
            }
            ('<', Some('='), _) => {
                self.bump_char();
                SyntaxKind::LtEq
            }
            ('>', Some('>'), Some('>')) if self.peek_at(2) == Some('=') => {
                self.i += 3;
                SyntaxKind::GtGtGtEq
            }
            ('>', Some('>'), Some('=')) => {
                self.i += 2;
                SyntaxKind::GtGtEq
            }
            ('>', Some('>'), Some('>')) => {
                self.i += 2;
                SyntaxKind::GtGtGt
            }
            ('>', Some('>'), _) => {
                self.bump_char();
                SyntaxKind::GtGt
            }
            ('>', Some('='), _) => {
                self.bump_char();
                SyntaxKind::GtEq
            }
            _ => single(ch),
        }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.i..].chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.text[self.i..].chars().nth(n)
    }

    fn bump_char(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += c.len_utf8();
        Some(c)
    }

    fn eat_while(&mut self, pred: impl Fn(char) -> bool) {
        while self.peek().is_some_and(&pred) {
            self.bump_char();
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        self.text[self.i..].starts_with(s)
    }
}

fn single(ch: char) -> SyntaxKind {
    match ch {
        '(' => SyntaxKind::LParen,
        ')' => SyntaxKind::RParen,
        '{' => SyntaxKind::LBrace,
        '}' => SyntaxKind::RBrace,
        '[' => SyntaxKind::LBracket,
        ']' => SyntaxKind::RBracket,
        ';' => SyntaxKind::Semi,
        ',' => SyntaxKind::Comma,
        '.' => SyntaxKind::Dot,
        ':' => SyntaxKind::Colon,
        '@' => SyntaxKind::At,
        '?' => SyntaxKind::Question,
        '=' => SyntaxKind::Eq,
        '+' => SyntaxKind::Plus,
        '-' => SyntaxKind::Minus,
        '*' => SyntaxKind::Star,
        '/' => SyntaxKind::Slash,
        '%' => SyntaxKind::Percent,
        '!' => SyntaxKind::Bang,
        '~' => SyntaxKind::Tilde,
        '&' => SyntaxKind::Amp,
        '|' => SyntaxKind::Pipe,
        '^' => SyntaxKind::Caret,
        '<' => SyntaxKind::Lt,
        '>' => SyntaxKind::Gt,
        _ => SyntaxKind::ErrorToken,
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_ident_continue(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<SyntaxKind> {
        lex(text).into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_keywords_comments_and_shifts() {
        assert_eq!(
            kinds("class // c\nFoo { int x = 1 >>> 2; }"),
            vec![
                SyntaxKind::ClassKw,
                SyntaxKind::Whitespace,
                SyntaxKind::LineComment,
                SyntaxKind::Whitespace,
                SyntaxKind::Ident,
                SyntaxKind::Whitespace,
                SyntaxKind::LBrace,
                SyntaxKind::Whitespace,
                SyntaxKind::IntKw,
                SyntaxKind::Whitespace,
                SyntaxKind::Ident,
                SyntaxKind::Whitespace,
                SyntaxKind::Eq,
                SyntaxKind::Whitespace,
                SyntaxKind::IntLiteral,
                SyntaxKind::Whitespace,
                SyntaxKind::GtGtGt,
                SyntaxKind::Whitespace,
                SyntaxKind::IntLiteral,
                SyntaxKind::Semi,
                SyntaxKind::Whitespace,
                SyntaxKind::RBrace,
            ]
        );
    }

    #[test]
    fn string_hides_braces_and_text_block_runs_to_close() {
        let tokens = lex(r#""{}" """ { } """"#);
        assert_eq!(tokens[0].kind, SyntaxKind::StringLiteral);
        assert_eq!(tokens[2].kind, SyntaxKind::TextBlock);
        assert!(!tokens.iter().any(|t| t.kind == SyntaxKind::LBrace));
    }

    #[test]
    fn unterminated_string_stops_at_newline() {
        let tokens = lex("\"abc\nint y;");
        assert_eq!(tokens[0].kind, SyntaxKind::ErrorToken);
        assert!(tokens.iter().any(|t| t.kind == SyntaxKind::IntKw));
    }
}
