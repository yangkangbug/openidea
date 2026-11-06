//! Recursive-descent parser for a documented Java syntax subset.
//! Expression precedence follows the Java Language Specification.
//! This is not a translation of IntelliJ's `JavaParser`.

use std::sync::Arc;

use crate::green::{GreenElement, GreenNode, GreenToken};
use crate::kind::SyntaxKind;
use crate::lex::{Token, lex};

pub fn parse_file(text: &str) -> Arc<GreenNode> {
    let mut p = Parser::new(text, lex(text));
    p.start(SyntaxKind::File);
    p.parse_package_opt();
    while p.at(SyntaxKind::Import) {
        p.parse_import();
    }
    while !p.eof() {
        let before = p.pos;
        if p.eat(SyntaxKind::Semi) {
            continue;
        }
        if p.looks_like_type_decl() {
            p.parse_type_decl();
        } else {
            p.error_one("expected type declaration");
        }
        if p.pos == before {
            p.bump();
        }
    }
    p.drain();
    p.finish()
}

pub fn parse_fragment(kind: SyntaxKind, text: &str) -> Arc<GreenNode> {
    let mut p = Parser::new(text, lex(text));
    match kind {
        SyntaxKind::EnumBody => p.parse_enum_body(),
        SyntaxKind::ClassBody | SyntaxKind::InterfaceBody | SyntaxKind::RecordBody => p.parse_body(kind),
        _ => p.parse_block(),
    }
}

struct Frame {
    kind: SyntaxKind,
    children: Vec<GreenElement>,
    message: Option<&'static str>,
}

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    stack: Vec<Frame>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, tokens: Vec<Token>) -> Self {
        Self { src, tokens, pos: 0, stack: Vec::new() }
    }

    fn start(&mut self, kind: SyntaxKind) {
        self.stack.push(Frame { kind, children: Vec::new(), message: None });
    }

    fn finish(&mut self) -> Arc<GreenNode> {
        let frame = self.stack.pop().expect("finish");
        let node = GreenNode::from_parts(frame.kind, frame.children, frame.message);
        if let Some(parent) = self.stack.last_mut() {
            parent.children.push(GreenElement::Node(Arc::clone(&node)));
        }
        node
    }

    fn set_message(&mut self, message: &'static str) {
        self.stack.last_mut().expect("frame").message = Some(message);
    }

    fn next_sig(&self) -> usize {
        let mut i = self.pos;
        while i < self.tokens.len() && self.tokens[i].kind.is_trivia() {
            i += 1;
        }
        i
    }

    fn kind(&self) -> SyntaxKind {
        let i = self.next_sig();
        if i >= self.tokens.len() { SyntaxKind::Eof } else { self.tokens[i].kind }
    }

    fn at(&self, kind: SyntaxKind) -> bool {
        self.kind() == kind
    }

    fn eof(&self) -> bool {
        self.kind() == SyntaxKind::Eof
    }

    fn drain(&mut self) {
        while self.pos < self.tokens.len() && self.tokens[self.pos].kind.is_trivia() {
            if self.stack.is_empty() {
                self.pos += 1;
            } else {
                self.push_token(self.pos);
                self.pos += 1;
            }
        }
    }

    fn push_token(&mut self, index: usize) {
        let token = &self.tokens[index];
        let text = self.src[token.start as usize..token.end as usize].to_string();
        let green = GreenToken::new(token.kind, text);
        self.stack.last_mut().expect("frame").children.push(GreenElement::Token(green));
    }

    fn bump(&mut self) {
        self.drain();
        if self.pos >= self.tokens.len() {
            return;
        }
        self.push_token(self.pos);
        self.pos += 1;
    }

    fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: SyntaxKind, message: &'static str) {
        if !self.eat(kind) {
            self.start(SyntaxKind::Error);
            self.set_message(message);
            self.finish();
        }
    }

    fn eat_gt(&mut self) -> bool {
        match self.kind() {
            SyntaxKind::Gt => {
                self.bump();
                true
            }
            SyntaxKind::GtGt | SyntaxKind::GtGtGt => {
                self.split_one_gt();
                self.bump();
                true
            }
            _ => false,
        }
    }

    fn split_one_gt(&mut self) {
        let index = self.next_sig();
        let token = self.tokens[index].clone();
        let rest = match token.kind {
            SyntaxKind::GtGt => SyntaxKind::Gt,
            SyntaxKind::GtGtGt => SyntaxKind::GtGt,
            _ => return,
        };
        let mid = token.start + 1;
        self.tokens[index] = Token { kind: SyntaxKind::Gt, start: token.start, end: mid };
        self.tokens.insert(index + 1, Token { kind: rest, start: mid, end: token.end });
    }

    fn pop(&mut self) -> GreenElement {
        self.stack.last_mut().expect("frame").children.pop().unwrap_or_else(|| {
            GreenElement::Node(GreenNode::from_parts(SyntaxKind::Error, Vec::new(), Some("expected expression")))
        })
    }

    fn push(&mut self, element: GreenElement) {
        self.stack.last_mut().expect("frame").children.push(element);
    }

    fn error_one(&mut self, message: &'static str) {
        self.start(SyntaxKind::Error);
        self.set_message(message);
        if !self.eof() {
            self.bump();
        }
        self.finish();
    }

    fn parse_package_opt(&mut self) {
        if !self.at(SyntaxKind::Package) {
            return;
        }
        self.start(SyntaxKind::PackageDecl);
        self.bump();
        self.parse_qualified_name(false);
        self.expect(SyntaxKind::Semi, "expected ';'");
        self.finish();
    }

    fn parse_import(&mut self) {
        self.start(SyntaxKind::ImportDecl);
        self.bump();
        self.eat(SyntaxKind::Static);
        self.parse_qualified_name(true);
        self.expect(SyntaxKind::Semi, "expected ';'");
        self.finish();
    }

    fn parse_qualified_name(&mut self, allow_star: bool) {
        self.start(SyntaxKind::Name);
        self.expect(SyntaxKind::Ident, "expected name");
        while self.eat(SyntaxKind::Dot) {
            if allow_star && self.eat(SyntaxKind::Star) {
                break;
            }
            self.expect(SyntaxKind::Ident, "expected name");
        }
        self.finish();
    }

    fn looks_like_type_decl(&self) -> bool {
        let mut look = Look::from(self);
        look.skip_mods();
        matches!(look.kind(), SyntaxKind::ClassKw | SyntaxKind::InterfaceKw | SyntaxKind::EnumKw | SyntaxKind::RecordKw)
    }

    fn parse_type_decl(&mut self) {
        let mut look = Look::from(self);
        look.skip_mods();
        match look.kind() {
            SyntaxKind::InterfaceKw => self.parse_class_like(SyntaxKind::Interface, SyntaxKind::InterfaceBody),
            SyntaxKind::EnumKw => self.parse_enum(),
            SyntaxKind::RecordKw => self.parse_record(),
            SyntaxKind::ClassKw => self.parse_class_like(SyntaxKind::Class, SyntaxKind::ClassBody),
            _ => self.error_one("expected type declaration"),
        }
    }

    fn parse_class_like(&mut self, node: SyntaxKind, body: SyntaxKind) {
        self.start(node);
        self.parse_modifier_list();
        self.bump();
        self.expect(SyntaxKind::Ident, "expected name");
        if self.at(SyntaxKind::Lt) {
            self.parse_type_params();
        }
        if self.eat(SyntaxKind::Extends) {
            self.parse_type();
            if node == SyntaxKind::Interface {
                while self.eat(SyntaxKind::Comma) {
                    self.parse_type();
                }
            }
        }
        if self.eat(SyntaxKind::Implements) {
            self.parse_type_list();
        }
        if self.eat(SyntaxKind::Permits) {
            self.parse_type_list();
        }
        self.parse_body(body);
        self.finish();
    }

    fn parse_enum(&mut self) {
        self.start(SyntaxKind::Enum);
        self.parse_modifier_list();
        self.bump();
        self.expect(SyntaxKind::Ident, "expected name");
        if self.at(SyntaxKind::Lt) {
            self.parse_type_params();
        }
        if self.eat(SyntaxKind::Implements) {
            self.parse_type_list();
        }
        self.parse_enum_body();
        self.finish();
    }

    fn parse_record(&mut self) {
        self.start(SyntaxKind::Record);
        self.parse_modifier_list();
        self.bump();
        self.expect(SyntaxKind::Ident, "expected name");
        if self.at(SyntaxKind::Lt) {
            self.parse_type_params();
        }
        self.parse_params();
        if self.eat(SyntaxKind::Implements) {
            self.parse_type_list();
        }
        self.parse_body(SyntaxKind::RecordBody);
        self.finish();
    }

    fn parse_type_list(&mut self) {
        self.parse_type();
        while self.eat(SyntaxKind::Comma) {
            self.parse_type();
        }
    }

    fn parse_type_params(&mut self) {
        self.start(SyntaxKind::TypeParamList);
        self.expect(SyntaxKind::Lt, "expected '<'");
        loop {
            self.start(SyntaxKind::TypeParam);
            self.expect(SyntaxKind::Ident, "expected type parameter");
            if self.eat(SyntaxKind::Extends) {
                self.parse_type();
                while self.eat(SyntaxKind::Amp) {
                    self.parse_type();
                }
            }
            self.finish();
            if !self.eat(SyntaxKind::Comma) {
                break;
            }
        }
        if !self.eat_gt() {
            self.expect(SyntaxKind::Gt, "expected '>'");
        }
        self.finish();
    }

    fn parse_body(&mut self, kind: SyntaxKind) -> Arc<GreenNode> {
        // Leading trivia stays in the parent so this node starts at `{`.
        self.drain();
        self.start(kind);
        self.expect(SyntaxKind::LBrace, "expected '{'");
        while !self.eof() && !self.at(SyntaxKind::RBrace) {
            let before = self.pos;
            self.parse_member();
            if self.pos == before {
                self.error_one("expected member");
            }
        }
        self.expect(SyntaxKind::RBrace, "expected '}'");
        self.finish()
    }

    fn parse_enum_body(&mut self) -> Arc<GreenNode> {
        self.drain();
        self.start(SyntaxKind::EnumBody);
        self.expect(SyntaxKind::LBrace, "expected '{'");
        while !self.eof() && !self.at(SyntaxKind::RBrace) {
            if self.at(SyntaxKind::Semi) {
                self.bump();
                self.parse_members_until_brace();
                break;
            }
            self.parse_enum_constant();
            if self.eat(SyntaxKind::Comma) {
                if self.at(SyntaxKind::Semi) || self.at(SyntaxKind::RBrace) {
                    continue;
                }
                continue;
            }
            if self.eat(SyntaxKind::Semi) {
                self.parse_members_until_brace();
            }
            break;
        }
        self.expect(SyntaxKind::RBrace, "expected '}'");
        self.finish()
    }

    fn parse_members_until_brace(&mut self) {
        while !self.eof() && !self.at(SyntaxKind::RBrace) {
            let before = self.pos;
            self.parse_member();
            if self.pos == before {
                self.error_one("expected member");
            }
        }
    }

    fn parse_enum_constant(&mut self) {
        self.start(SyntaxKind::EnumConstant);
        while self.at(SyntaxKind::At) {
            self.parse_annotation();
        }
        self.expect(SyntaxKind::Ident, "expected enum constant");
        if self.at(SyntaxKind::LParen) {
            self.parse_args();
        }
        if self.at(SyntaxKind::LBrace) {
            self.parse_body(SyntaxKind::ClassBody);
        }
        self.finish();
    }

    fn member_shape(&self) -> MemberShape {
        let mut look = Look::from(self);
        look.skip_mods();
        if look.kind() == SyntaxKind::LBrace {
            return MemberShape::Initializer;
        }
        if matches!(
            look.kind(),
            SyntaxKind::ClassKw | SyntaxKind::InterfaceKw | SyntaxKind::EnumKw | SyntaxKind::RecordKw
        ) {
            return MemberShape::Type;
        }
        if look.kind() == SyntaxKind::Lt {
            let mut after = look;
            if after.scan_type_params() {
                if after.kind() == SyntaxKind::Ident && after.nth(1) == SyntaxKind::LParen {
                    return MemberShape::Constructor;
                }
                return MemberShape::Method;
            }
        }
        if look.kind() == SyntaxKind::Ident && look.nth(1) == SyntaxKind::LParen {
            return MemberShape::Constructor;
        }
        if look.scan_type() && look.kind() == SyntaxKind::Ident && look.nth(1) == SyntaxKind::LParen {
            return MemberShape::Method;
        }
        MemberShape::Field
    }

    fn parse_member(&mut self) {
        if self.at(SyntaxKind::Semi) {
            self.bump();
            return;
        }
        match self.member_shape() {
            MemberShape::Initializer => self.parse_initializer(),
            MemberShape::Type => self.parse_type_decl(),
            MemberShape::Constructor => self.parse_constructor(),
            MemberShape::Method => self.parse_method(),
            MemberShape::Field => self.parse_field(),
        }
    }

    fn parse_initializer(&mut self) {
        self.start(SyntaxKind::Initializer);
        self.parse_modifier_list();
        self.parse_block();
        self.finish();
    }

    fn parse_constructor(&mut self) {
        self.start(SyntaxKind::Constructor);
        self.parse_modifier_list();
        if self.at(SyntaxKind::Lt) {
            self.parse_type_params();
        }
        self.expect(SyntaxKind::Ident, "expected constructor name");
        self.parse_params();
        if self.eat(SyntaxKind::Throws) {
            self.parse_type_list();
        }
        if self.at(SyntaxKind::LBrace) {
            self.parse_block();
        } else {
            self.expect(SyntaxKind::Semi, "expected constructor body");
        }
        self.finish();
    }

    fn parse_method(&mut self) {
        self.start(SyntaxKind::Method);
        self.parse_modifier_list();
        if self.at(SyntaxKind::Lt) {
            self.parse_type_params();
        }
        self.parse_type();
        self.expect(SyntaxKind::Ident, "expected method name");
        self.parse_params();
        if self.eat(SyntaxKind::Throws) {
            self.parse_type_list();
        }
        if self.at(SyntaxKind::LBrace) {
            self.parse_block();
        } else {
            self.expect(SyntaxKind::Semi, "expected body or ';'");
        }
        self.finish();
    }

    fn parse_field(&mut self) {
        self.start(SyntaxKind::Field);
        self.parse_modifier_list();
        self.parse_type();
        self.parse_declarators();
        self.expect(SyntaxKind::Semi, "expected ';'");
        self.finish();
    }

    fn parse_declarators(&mut self) {
        self.expect(SyntaxKind::Ident, "expected name");
        if self.eat(SyntaxKind::Eq) {
            self.parse_expr();
        }
        while self.eat(SyntaxKind::Comma) {
            self.expect(SyntaxKind::Ident, "expected name");
            if self.eat(SyntaxKind::Eq) {
                self.parse_expr();
            }
        }
    }

    fn parse_modifier_list(&mut self) {
        if !self.has_modifier() {
            return;
        }
        self.start(SyntaxKind::ModifierList);
        while self.at(SyntaxKind::At) || self.kind().is_modifier() {
            if self.at(SyntaxKind::At) {
                self.parse_annotation();
            } else {
                self.bump();
            }
        }
        self.finish();
    }

    fn has_modifier(&self) -> bool {
        self.at(SyntaxKind::At) || self.kind().is_modifier()
    }

    fn parse_annotation(&mut self) {
        self.start(SyntaxKind::Annotation);
        self.expect(SyntaxKind::At, "expected '@'");
        self.parse_qualified_name(false);
        if self.eat(SyntaxKind::LParen) {
            if !self.at(SyntaxKind::RParen) {
                loop {
                    if self.at(SyntaxKind::Ident) && self.nth(1) == SyntaxKind::Eq {
                        self.bump();
                        self.bump();
                    }
                    self.parse_element_value();
                    if !self.eat(SyntaxKind::Comma) || self.at(SyntaxKind::RParen) {
                        break;
                    }
                }
            }
            self.expect(SyntaxKind::RParen, "expected ')'");
        }
        self.finish();
    }

    fn parse_element_value(&mut self) {
        if self.at(SyntaxKind::At) {
            self.parse_annotation();
        } else if self.at(SyntaxKind::LBrace) {
            self.parse_array_init();
        } else {
            self.parse_expr();
        }
    }

    /// `n == 0` is the current non-trivia token.
    fn nth(&self, n: usize) -> SyntaxKind {
        let mut i = self.pos;
        let mut seen = 0;
        while i < self.tokens.len() {
            if self.tokens[i].kind.is_trivia() {
                i += 1;
                continue;
            }
            if seen == n {
                return self.tokens[i].kind;
            }
            seen += 1;
            i += 1;
        }
        SyntaxKind::Eof
    }

    fn parse_params(&mut self) {
        self.start(SyntaxKind::ParamList);
        self.expect(SyntaxKind::LParen, "expected '('");
        if !self.at(SyntaxKind::RParen) && !self.eof() {
            loop {
                self.parse_param();
                if !self.eat(SyntaxKind::Comma) || self.at(SyntaxKind::RParen) {
                    break;
                }
            }
        }
        self.expect(SyntaxKind::RParen, "expected ')'");
        self.finish();
    }

    fn parse_param(&mut self) {
        self.start(SyntaxKind::Parameter);
        self.parse_modifier_list();
        self.parse_type();
        self.eat(SyntaxKind::DotDotDot);
        self.expect(SyntaxKind::Ident, "expected parameter name");
        self.finish();
    }

    fn parse_type(&mut self) {
        self.start(SyntaxKind::Type);
        while self.at(SyntaxKind::At) {
            self.parse_annotation();
        }
        if self.eat(SyntaxKind::Question) {
            if self.eat(SyntaxKind::Extends) || self.eat(SyntaxKind::Super) {
                self.parse_type_rest(false);
            }
        } else {
            self.parse_type_rest(true);
        }
        while self.eat(SyntaxKind::LBracket) {
            self.expect(SyntaxKind::RBracket, "expected ']'");
        }
        self.finish();
    }

    fn parse_type_rest(&mut self, allow_primitive: bool) {
        if allow_primitive && self.eat_primitive() {
            return;
        }
        self.start(SyntaxKind::Name);
        self.expect(SyntaxKind::Ident, "expected type name");
        if self.at(SyntaxKind::Lt) {
            self.parse_type_args();
        }
        while self.eat(SyntaxKind::Dot) {
            self.expect(SyntaxKind::Ident, "expected type name");
            if self.at(SyntaxKind::Lt) {
                self.parse_type_args();
            }
        }
        self.finish();
    }

    fn eat_primitive(&mut self) -> bool {
        match self.kind() {
            SyntaxKind::BooleanKw
            | SyntaxKind::Byte
            | SyntaxKind::CharKw
            | SyntaxKind::Short
            | SyntaxKind::IntKw
            | SyntaxKind::LongKw
            | SyntaxKind::FloatKw
            | SyntaxKind::DoubleKw
            | SyntaxKind::Void => {
                self.bump();
                true
            }
            _ => false,
        }
    }

    fn parse_type_args(&mut self) {
        self.start(SyntaxKind::TypeArgList);
        self.expect(SyntaxKind::Lt, "expected '<'");
        if !self.closes_args() {
            loop {
                self.parse_type();
                if self.eat(SyntaxKind::Comma) && !self.closes_args() {
                    continue;
                }
                break;
            }
        }
        if !self.eat_gt() {
            self.expect(SyntaxKind::Gt, "expected '>'");
        }
        self.finish();
    }

    fn closes_args(&self) -> bool {
        matches!(self.kind(), SyntaxKind::Gt | SyntaxKind::GtGt | SyntaxKind::GtGtGt)
    }

    fn parse_block(&mut self) -> Arc<GreenNode> {
        self.drain();
        self.start(SyntaxKind::Block);
        self.expect(SyntaxKind::LBrace, "expected '{'");
        self.parse_stmts();
        self.expect(SyntaxKind::RBrace, "expected '}'");
        self.finish()
    }

    fn parse_stmts(&mut self) {
        while !self.eof() && !self.at(SyntaxKind::RBrace) {
            let before = self.pos;
            self.parse_stmt();
            if self.pos == before {
                self.error_one("expected statement");
            }
        }
    }

    fn parse_stmt(&mut self) {
        match self.kind() {
            SyntaxKind::Semi => {
                self.start(SyntaxKind::EmptyStmt);
                self.bump();
                self.finish();
            }
            SyntaxKind::LBrace => {
                self.parse_block();
            }
            SyntaxKind::If => self.parse_if(),
            SyntaxKind::While => self.parse_while(),
            SyntaxKind::Do => self.parse_do(),
            SyntaxKind::For => self.parse_for(),
            SyntaxKind::Return => self.parse_jump(SyntaxKind::ReturnStmt, true),
            SyntaxKind::Throw => self.parse_jump(SyntaxKind::ThrowStmt, true),
            SyntaxKind::Break => self.parse_jump(SyntaxKind::BreakStmt, false),
            SyntaxKind::Continue => self.parse_jump(SyntaxKind::ContinueStmt, false),
            SyntaxKind::Assert => self.parse_assert(),
            SyntaxKind::Try => self.parse_try(),
            SyntaxKind::Switch => self.recover_switch(),
            _ if self.looks_like_decl() => self.parse_local(true),
            _ if self.can_start_expr() => {
                self.start(SyntaxKind::ExprStmt);
                self.parse_expr();
                self.expect(SyntaxKind::Semi, "expected ';'");
                self.finish();
            }
            _ => self.recover_stmt(),
        }
    }

    fn parse_if(&mut self) {
        self.start(SyntaxKind::IfStmt);
        self.bump();
        self.expect(SyntaxKind::LParen, "expected '('");
        self.parse_expr();
        self.expect(SyntaxKind::RParen, "expected ')'");
        self.parse_stmt();
        if self.eat(SyntaxKind::Else) {
            self.parse_stmt();
        }
        self.finish();
    }

    fn parse_while(&mut self) {
        self.start(SyntaxKind::WhileStmt);
        self.bump();
        self.expect(SyntaxKind::LParen, "expected '('");
        self.parse_expr();
        self.expect(SyntaxKind::RParen, "expected ')'");
        self.parse_stmt();
        self.finish();
    }

    fn parse_do(&mut self) {
        self.start(SyntaxKind::DoStmt);
        self.bump();
        self.parse_stmt();
        self.expect(SyntaxKind::While, "expected 'while'");
        self.expect(SyntaxKind::LParen, "expected '('");
        self.parse_expr();
        self.expect(SyntaxKind::RParen, "expected ')'");
        self.expect(SyntaxKind::Semi, "expected ';'");
        self.finish();
    }

    fn parse_for(&mut self) {
        self.start(SyntaxKind::ForStmt);
        self.bump();
        self.expect(SyntaxKind::LParen, "expected '('");
        if !self.at(SyntaxKind::Semi) && !self.at(SyntaxKind::RParen) {
            if self.looks_like_decl() {
                self.parse_local(false);
            } else if self.can_start_expr() {
                self.parse_expr();
            } else {
                self.error_one("expected for initializer");
            }
        }
        if self.eat(SyntaxKind::Colon) {
            self.parse_expr();
        } else {
            self.expect(SyntaxKind::Semi, "expected ';'");
            if !self.at(SyntaxKind::Semi) && self.can_start_expr() {
                self.parse_expr();
            }
            self.expect(SyntaxKind::Semi, "expected ';'");
            if !self.at(SyntaxKind::RParen) && self.can_start_expr() {
                self.parse_expr();
            }
        }
        self.expect(SyntaxKind::RParen, "expected ')'");
        self.parse_stmt();
        self.finish();
    }

    fn parse_jump(&mut self, kind: SyntaxKind, expr: bool) {
        self.start(kind);
        self.bump();
        if expr {
            if !self.at(SyntaxKind::Semi) && self.can_start_expr() {
                self.parse_expr();
            }
        } else if self.at(SyntaxKind::Ident) {
            self.bump();
        }
        self.expect(SyntaxKind::Semi, "expected ';'");
        self.finish();
    }

    fn parse_assert(&mut self) {
        self.start(SyntaxKind::AssertStmt);
        self.bump();
        self.parse_expr();
        if self.eat(SyntaxKind::Colon) {
            self.parse_expr();
        }
        self.expect(SyntaxKind::Semi, "expected ';'");
        self.finish();
    }

    fn parse_try(&mut self) {
        self.start(SyntaxKind::TryStmt);
        self.bump();
        if self.at(SyntaxKind::LParen) {
            self.start(SyntaxKind::Error);
            self.set_message("try-with-resources is not in this subset");
            self.bump();
            let mut depth = 1i32;
            while !self.eof() && depth > 0 {
                match self.kind() {
                    SyntaxKind::LParen => depth += 1,
                    SyntaxKind::RParen => depth -= 1,
                    _ => {}
                }
                self.bump();
            }
            self.finish();
        }
        if self.at(SyntaxKind::LBrace) {
            self.parse_block();
        }
        while self.at(SyntaxKind::Catch) {
            self.start(SyntaxKind::CatchClause);
            self.bump();
            self.expect(SyntaxKind::LParen, "expected '('");
            self.parse_type();
            while self.eat(SyntaxKind::Pipe) {
                self.parse_type();
            }
            self.expect(SyntaxKind::Ident, "expected name");
            self.expect(SyntaxKind::RParen, "expected ')'");
            self.parse_block();
            self.finish();
        }
        if self.eat(SyntaxKind::Finally) {
            self.parse_block();
        }
        self.finish();
    }

    fn recover_switch(&mut self) {
        self.start(SyntaxKind::Error);
        self.set_message("switch is not parsed in this subset");
        self.bump();
        let mut depth = 0i32;
        let mut saw_brace = false;
        while !self.eof() {
            match self.kind() {
                SyntaxKind::LBrace => {
                    depth += 1;
                    saw_brace = true;
                    self.bump();
                }
                SyntaxKind::RBrace => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    self.bump();
                    if depth == 0 && saw_brace {
                        break;
                    }
                }
                SyntaxKind::Semi if depth == 0 => {
                    self.bump();
                    break;
                }
                _ => self.bump(),
            }
        }
        self.finish();
    }

    fn recover_stmt(&mut self) {
        self.start(SyntaxKind::Error);
        self.set_message("expected statement");
        while !self.eof() {
            let kind = self.kind();
            if kind == SyntaxKind::RBrace || kind == SyntaxKind::LBrace || is_stmt_keyword(kind) {
                break;
            }
            self.bump();
            if kind == SyntaxKind::Semi {
                break;
            }
        }
        self.finish();
    }

    fn parse_local(&mut self, semi: bool) {
        self.start(SyntaxKind::LocalVar);
        self.parse_modifier_list();
        if self.eat(SyntaxKind::Var) {
            self.expect(SyntaxKind::Ident, "expected name");
            if self.eat(SyntaxKind::Eq) {
                self.parse_expr();
            }
            while self.eat(SyntaxKind::Comma) {
                self.expect(SyntaxKind::Ident, "expected name");
                if self.eat(SyntaxKind::Eq) {
                    self.parse_expr();
                }
            }
        } else {
            self.parse_type();
            self.parse_declarators();
        }
        if semi {
            self.expect(SyntaxKind::Semi, "expected ';'");
        }
        self.finish();
    }

    fn looks_like_decl(&self) -> bool {
        let mut look = Look::from(self);
        look.skip_mods();
        if look.eat(SyntaxKind::Var) {
            return look.kind() == SyntaxKind::Ident;
        }
        look.scan_type() && look.kind() == SyntaxKind::Ident
    }

    fn can_start_expr(&self) -> bool {
        can_start_unary(self.kind()) || self.at(SyntaxKind::LBrace) || self.type_then_colon_colon()
    }

    fn type_args_then_colon_colon(&self) -> bool {
        let mut look = Look::from(self);
        look.scan_args() && look.kind() == SyntaxKind::ColonColon
    }

    fn looks_like_cast(&self) -> bool {
        let mut look = Look::from(self);
        if !look.eat(SyntaxKind::LParen) {
            return false;
        }
        let mut probe = look;
        probe.skip_annotations();
        let primitive = probe.is_primitive();
        if !look.scan_type() || !look.eat(SyntaxKind::RParen) {
            return false;
        }
        let next = look.kind();
        if matches!(next, SyntaxKind::PlusPlus | SyntaxKind::MinusMinus) {
            return primitive;
        }
        can_start_unary(next)
    }

    fn parse_expr(&mut self) {
        self.parse_expr_bp(0);
    }

    fn parse_expr_bp(&mut self, min: u8) {
        self.parse_unary();
        loop {
            let op = self.kind();
            let Some((left_bp, right_bp)) = infix_power(op) else { break };
            if left_bp < min {
                break;
            }
            let left = self.pop();
            self.start(SyntaxKind::BinaryExpr);
            self.push(left);
            self.bump();
            if op == SyntaxKind::Instanceof {
                self.parse_type();
            } else if op == SyntaxKind::Question {
                self.parse_expr_bp(0);
                self.expect(SyntaxKind::Colon, "expected ':'");
                self.parse_expr_bp(right_bp);
            } else {
                self.parse_expr_bp(right_bp);
            }
            self.finish();
        }
    }

    fn parse_unary(&mut self) {
        if matches!(
            self.kind(),
            SyntaxKind::PlusPlus
                | SyntaxKind::MinusMinus
                | SyntaxKind::Plus
                | SyntaxKind::Minus
                | SyntaxKind::Bang
                | SyntaxKind::Tilde
        ) {
            self.start(SyntaxKind::UnaryExpr);
            self.bump();
            self.parse_unary();
            self.finish();
        } else if self.lambda_decision().is_some() {
            // Same commit point as IntelliJ `parseLambdaAfterParenth`: a parameter
            // list is a lambda only when `->` follows it. Otherwise `(int) x` stays a cast.
            self.parse_lambda();
        } else if self.type_then_colon_colon() {
            self.parse_type_method_ref();
        } else if self.at(SyntaxKind::LParen) && self.looks_like_cast() {
            // The operand is a unary expression, so postfix binds inside the cast:
            // `(int) x.f` is a cast of `x.f`, not a field access on the cast.
            self.start(SyntaxKind::CastExpr);
            self.bump();
            self.parse_type();
            self.expect(SyntaxKind::RParen, "expected ')'");
            self.parse_unary();
            self.finish();
        } else {
            self.parse_primary();
            self.parse_postfix();
        }
    }

    /// `Some(true)` is a typed parameter list, `Some(false)` an untyped one.
    /// Mirrors the lookahead in `PrattExpressionParser.parseLambdaAfterParenth`
    /// (intellij-community 840d3be11dfdeada533d05052008b48816b6cc4e): `()`,
    /// `(a, b)`, `(a)`, modifiers or primitives, and a type followed by a name
    /// are lambdas only when `->` is next. A failed attempt is not parsed.
    fn lambda_decision(&self) -> Option<bool> {
        if self.at(SyntaxKind::Ident) && self.nth(1) == SyntaxKind::Arrow {
            return Some(false);
        }
        if !self.at(SyntaxKind::LParen) {
            return None;
        }
        let mut look = Look::from(self);
        look.bump();
        let first = look.kind();
        let mut second_look = look;
        second_look.bump();
        let second = second_look.kind();
        let mut third_look = second_look;
        third_look.bump();
        let third = third_look.kind();
        if first == SyntaxKind::RParen && second == SyntaxKind::Arrow {
            return Some(false);
        }
        if first == SyntaxKind::Ident
            && (second == SyntaxKind::Comma || (second == SyntaxKind::RParen && third == SyntaxKind::Arrow))
        {
            return Some(false);
        }
        if first == SyntaxKind::Ident && second == SyntaxKind::Arrow {
            return None;
        }
        let typed_start =
            first == SyntaxKind::At || first.is_modifier() || look.is_primitive() || first == SyntaxKind::Ident;
        if typed_start && self.typed_lambda_arrow() {
            return Some(true);
        }
        None
    }

    fn typed_lambda_arrow(&self) -> bool {
        let mut look = Look::from(self);
        if !look.eat(SyntaxKind::LParen) {
            return false;
        }
        if look.kind() == SyntaxKind::RParen {
            look.bump();
            return look.kind() == SyntaxKind::Arrow;
        }
        loop {
            look.skip_mods();
            if !look.scan_type() {
                return false;
            }
            if look.kind() == SyntaxKind::DotDotDot {
                look.bump();
            }
            if look.kind() != SyntaxKind::Ident {
                return false;
            }
            look.bump();
            if look.eat(SyntaxKind::Comma) && look.kind() != SyntaxKind::RParen {
                continue;
            }
            break;
        }
        look.eat(SyntaxKind::RParen) && look.kind() == SyntaxKind::Arrow
    }

    fn parse_lambda(&mut self) {
        let typed = self.lambda_decision().unwrap_or(false);
        self.start(SyntaxKind::LambdaExpr);
        if self.at(SyntaxKind::Ident) && self.nth(1) == SyntaxKind::Arrow {
            self.start(SyntaxKind::ParamList);
            self.start(SyntaxKind::Parameter);
            self.bump();
            self.finish();
            self.finish();
        } else {
            self.start(SyntaxKind::ParamList);
            self.expect(SyntaxKind::LParen, "expected '('");
            if !self.at(SyntaxKind::RParen) && !self.eof() {
                loop {
                    self.start(SyntaxKind::Parameter);
                    if typed {
                        self.parse_modifier_list();
                        self.parse_type();
                        self.eat(SyntaxKind::DotDotDot);
                    }
                    self.expect(SyntaxKind::Ident, "expected parameter name");
                    self.finish();
                    if !self.eat(SyntaxKind::Comma) || self.at(SyntaxKind::RParen) {
                        break;
                    }
                }
            }
            self.expect(SyntaxKind::RParen, "expected ')'");
            self.finish();
        }
        self.expect(SyntaxKind::Arrow, "expected '->'");
        if self.at(SyntaxKind::LBrace) {
            self.parse_block();
        } else if self.can_start_expr() {
            self.parse_expr();
        } else {
            self.start(SyntaxKind::Error);
            self.set_message("expected lambda body");
            self.finish();
        }
        self.finish();
    }

    fn type_then_colon_colon(&self) -> bool {
        let mut look = Look::from(self);
        if !(look.is_primitive() || look.kind() == SyntaxKind::Ident) {
            return false;
        }
        look.scan_type() && look.kind() == SyntaxKind::ColonColon
    }

    fn parse_type_method_ref(&mut self) {
        self.start(SyntaxKind::MethodRefExpr);
        self.parse_type();
        self.finish_method_ref_tail();
    }

    fn finish_method_ref_tail(&mut self) {
        self.expect(SyntaxKind::ColonColon, "expected '::'");
        if self.at(SyntaxKind::Lt) {
            self.parse_type_args();
        }
        if !self.eat(SyntaxKind::Ident) && !self.eat(SyntaxKind::New) {
            self.expect(SyntaxKind::Ident, "expected name");
        }
        self.finish();
    }

    fn parse_primary(&mut self) {
        match self.kind() {
            SyntaxKind::IntLiteral
            | SyntaxKind::LongLiteral
            | SyntaxKind::FloatLiteral
            | SyntaxKind::DoubleLiteral
            | SyntaxKind::CharLiteral
            | SyntaxKind::StringLiteral
            | SyntaxKind::TextBlock
            | SyntaxKind::True
            | SyntaxKind::False
            | SyntaxKind::Null => self.leaf_expr(SyntaxKind::LiteralExpr),
            SyntaxKind::This => self.leaf_expr(SyntaxKind::ThisExpr),
            SyntaxKind::Super => self.leaf_expr(SyntaxKind::SuperExpr),
            SyntaxKind::Ident => self.leaf_expr(SyntaxKind::ReferenceExpr),
            SyntaxKind::LParen => {
                self.start(SyntaxKind::ParenExpr);
                self.bump();
                self.parse_expr();
                self.expect(SyntaxKind::RParen, "expected ')'");
                self.finish();
            }
            SyntaxKind::New => self.parse_new(),
            SyntaxKind::LBrace => self.parse_array_init(),
            _ => {
                self.start(SyntaxKind::Error);
                self.set_message("expected expression");
                if !self.eof()
                    && !matches!(
                        self.kind(),
                        SyntaxKind::Semi
                            | SyntaxKind::RParen
                            | SyntaxKind::RBrace
                            | SyntaxKind::RBracket
                            | SyntaxKind::Comma
                    )
                {
                    self.bump();
                }
                self.finish();
            }
        }
    }

    fn leaf_expr(&mut self, kind: SyntaxKind) {
        self.start(kind);
        self.bump();
        self.finish();
    }

    fn parse_postfix(&mut self) {
        loop {
            if self.at(SyntaxKind::Lt) && self.type_args_then_colon_colon() {
                let base = self.pop();
                self.start(SyntaxKind::MethodRefExpr);
                self.push(base);
                self.parse_type_args();
                self.finish_method_ref_tail();
            } else if self.at(SyntaxKind::ColonColon) {
                let base = self.pop();
                self.start(SyntaxKind::MethodRefExpr);
                self.push(base);
                self.finish_method_ref_tail();
            } else if self.at(SyntaxKind::LParen) {
                let callee = self.pop();
                self.start(SyntaxKind::CallExpr);
                self.push(callee);
                self.parse_args();
                self.finish();
            } else if self.at(SyntaxKind::LBracket) {
                let base = self.pop();
                self.start(SyntaxKind::IndexExpr);
                self.push(base);
                self.bump();
                if !self.at(SyntaxKind::RBracket) {
                    self.parse_expr();
                }
                self.expect(SyntaxKind::RBracket, "expected ']'");
                self.finish();
            } else if self.at(SyntaxKind::Dot) {
                let base = self.pop();
                self.start(SyntaxKind::FieldExpr);
                self.push(base);
                self.bump();
                if !self.eat(SyntaxKind::Ident)
                    && !self.eat(SyntaxKind::This)
                    && !self.eat(SyntaxKind::Super)
                    && !self.eat(SyntaxKind::ClassKw)
                {
                    self.expect(SyntaxKind::Ident, "expected name");
                }
                self.finish();
            } else if matches!(self.kind(), SyntaxKind::PlusPlus | SyntaxKind::MinusMinus) {
                let base = self.pop();
                self.start(SyntaxKind::UnaryExpr);
                self.push(base);
                self.bump();
                self.finish();
            } else {
                break;
            }
        }
    }

    fn parse_new(&mut self) {
        self.start(SyntaxKind::NewExpr);
        self.bump();
        self.parse_creator_type();
        if self.at(SyntaxKind::LParen) {
            self.parse_args();
            if self.at(SyntaxKind::LBrace) {
                self.parse_body(SyntaxKind::ClassBody);
            }
        } else {
            let mut dims = false;
            while self.eat(SyntaxKind::LBracket) {
                dims = true;
                if !self.at(SyntaxKind::RBracket) {
                    self.parse_expr();
                }
                self.expect(SyntaxKind::RBracket, "expected ']'");
            }
            if !dims {
                self.expect(SyntaxKind::LParen, "expected '(' or '['");
            }
            if self.at(SyntaxKind::LBrace) {
                self.parse_array_init();
            }
        }
        self.finish();
    }

    fn parse_creator_type(&mut self) {
        self.start(SyntaxKind::Type);
        while self.at(SyntaxKind::At) {
            self.parse_annotation();
        }
        if !self.eat_primitive() {
            self.parse_type_rest(false);
        }
        self.finish();
    }

    fn parse_array_init(&mut self) {
        self.start(SyntaxKind::ArrayInit);
        self.expect(SyntaxKind::LBrace, "expected '{'");
        if !self.at(SyntaxKind::RBrace) {
            loop {
                if self.at(SyntaxKind::LBrace) {
                    self.parse_array_init();
                } else {
                    self.parse_expr();
                }
                if !self.eat(SyntaxKind::Comma) || self.at(SyntaxKind::RBrace) {
                    break;
                }
            }
        }
        self.expect(SyntaxKind::RBrace, "expected '}'");
        self.finish();
    }

    fn parse_args(&mut self) {
        self.start(SyntaxKind::ArgList);
        self.expect(SyntaxKind::LParen, "expected '('");
        if !self.at(SyntaxKind::RParen) && !self.eof() {
            loop {
                self.parse_expr();
                if !self.eat(SyntaxKind::Comma) || self.at(SyntaxKind::RParen) {
                    break;
                }
            }
        }
        self.expect(SyntaxKind::RParen, "expected ')'");
        self.finish();
    }
}

#[derive(Clone, Copy)]
enum MemberShape {
    Initializer,
    Type,
    Constructor,
    Method,
    Field,
}

fn is_stmt_keyword(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::If
            | SyntaxKind::While
            | SyntaxKind::Do
            | SyntaxKind::For
            | SyntaxKind::Return
            | SyntaxKind::Throw
            | SyntaxKind::Break
            | SyntaxKind::Continue
            | SyntaxKind::Assert
            | SyntaxKind::Try
            | SyntaxKind::Switch
    )
}

fn can_start_unary(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::Ident
            | SyntaxKind::IntLiteral
            | SyntaxKind::LongLiteral
            | SyntaxKind::FloatLiteral
            | SyntaxKind::DoubleLiteral
            | SyntaxKind::CharLiteral
            | SyntaxKind::StringLiteral
            | SyntaxKind::TextBlock
            | SyntaxKind::True
            | SyntaxKind::False
            | SyntaxKind::Null
            | SyntaxKind::This
            | SyntaxKind::Super
            | SyntaxKind::New
            | SyntaxKind::LParen
            | SyntaxKind::Bang
            | SyntaxKind::Tilde
            | SyntaxKind::Plus
            | SyntaxKind::Minus
            | SyntaxKind::PlusPlus
            | SyntaxKind::MinusMinus
    )
}

fn infix_power(kind: SyntaxKind) -> Option<(u8, u8)> {
    Some(match kind {
        SyntaxKind::Eq
        | SyntaxKind::PlusEq
        | SyntaxKind::MinusEq
        | SyntaxKind::StarEq
        | SyntaxKind::SlashEq
        | SyntaxKind::PercentEq
        | SyntaxKind::AmpEq
        | SyntaxKind::PipeEq
        | SyntaxKind::CaretEq
        | SyntaxKind::LtLtEq
        | SyntaxKind::GtGtEq
        | SyntaxKind::GtGtGtEq => (2, 1),
        SyntaxKind::Question => (4, 3),
        SyntaxKind::PipePipe => (6, 6),
        SyntaxKind::AmpAmp => (8, 8),
        SyntaxKind::Pipe => (10, 10),
        SyntaxKind::Caret => (12, 12),
        SyntaxKind::Amp => (14, 14),
        SyntaxKind::EqEq | SyntaxKind::NotEq => (16, 16),
        SyntaxKind::Lt | SyntaxKind::Gt | SyntaxKind::LtEq | SyntaxKind::GtEq | SyntaxKind::Instanceof => (18, 18),
        SyntaxKind::LtLt | SyntaxKind::GtGt | SyntaxKind::GtGtGt => (20, 20),
        SyntaxKind::Plus | SyntaxKind::Minus => (22, 22),
        SyntaxKind::Star | SyntaxKind::Slash | SyntaxKind::Percent => (24, 24),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
struct Look<'a> {
    tokens: &'a [Token],
    i: usize,
    pending: u8,
}

impl<'a> Look<'a> {
    fn from(parser: &'a Parser<'_>) -> Self {
        Self { tokens: &parser.tokens, i: parser.pos, pending: 0 }
    }

    fn skip(&mut self) {
        while self.i < self.tokens.len() && self.tokens[self.i].kind.is_trivia() {
            self.i += 1;
        }
    }

    fn kind(&mut self) -> SyntaxKind {
        if self.pending > 0 {
            return SyntaxKind::Gt;
        }
        self.skip();
        if self.i >= self.tokens.len() { SyntaxKind::Eof } else { self.tokens[self.i].kind }
    }

    /// `n == 0` is the current non-trivia token.
    fn nth(&mut self, n: usize) -> SyntaxKind {
        let saved = *self;
        for _ in 0..n {
            self.bump();
        }
        let kind = self.kind();
        *self = saved;
        kind
    }

    fn bump(&mut self) {
        if self.pending > 0 {
            self.pending -= 1;
            return;
        }
        self.skip();
        if self.i < self.tokens.len() {
            self.i += 1;
        }
    }

    fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.kind() == kind {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_gt(&mut self) -> bool {
        if self.pending > 0 {
            self.pending -= 1;
            return true;
        }
        self.skip();
        if self.i >= self.tokens.len() {
            return false;
        }
        match self.tokens[self.i].kind {
            SyntaxKind::Gt => {
                self.i += 1;
                true
            }
            SyntaxKind::GtGt => {
                self.i += 1;
                self.pending = 1;
                true
            }
            SyntaxKind::GtGtGt => {
                self.i += 1;
                self.pending = 2;
                true
            }
            _ => false,
        }
    }

    fn is_gt(&mut self) -> bool {
        matches!(self.kind(), SyntaxKind::Gt | SyntaxKind::GtGt | SyntaxKind::GtGtGt)
    }

    fn is_primitive(&mut self) -> bool {
        matches!(
            self.kind(),
            SyntaxKind::BooleanKw
                | SyntaxKind::Byte
                | SyntaxKind::CharKw
                | SyntaxKind::Short
                | SyntaxKind::IntKw
                | SyntaxKind::LongKw
                | SyntaxKind::FloatKw
                | SyntaxKind::DoubleKw
                | SyntaxKind::Void
        )
    }

    fn skip_annotations(&mut self) {
        while self.kind() == SyntaxKind::At {
            self.skip_annotation();
        }
    }

    fn skip_mods(&mut self) {
        loop {
            if self.kind() == SyntaxKind::At {
                self.skip_annotation();
            } else if self.kind().is_modifier() {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn skip_annotation(&mut self) {
        if !self.eat(SyntaxKind::At) {
            return;
        }
        if self.kind() == SyntaxKind::Ident {
            self.bump();
            while self.eat(SyntaxKind::Dot) && self.kind() == SyntaxKind::Ident {
                self.bump();
            }
        }
        if self.eat(SyntaxKind::LParen) {
            let mut depth = 1i32;
            while self.kind() != SyntaxKind::Eof && depth > 0 {
                match self.kind() {
                    SyntaxKind::LParen => depth += 1,
                    SyntaxKind::RParen => depth -= 1,
                    _ => {}
                }
                self.bump();
            }
        }
    }

    fn scan_type(&mut self) -> bool {
        self.skip_annotations();
        if self.eat(SyntaxKind::Question) {
            if self.eat(SyntaxKind::Extends) || self.eat(SyntaxKind::Super) {
                return self.scan_type();
            }
            return true;
        }
        if self.is_primitive() {
            self.bump();
        } else if !self.scan_class() {
            return false;
        }
        while self.eat(SyntaxKind::LBracket) {
            if !self.eat(SyntaxKind::RBracket) {
                return false;
            }
        }
        true
    }

    fn scan_class(&mut self) -> bool {
        if self.kind() != SyntaxKind::Ident {
            return false;
        }
        self.bump();
        if self.kind() == SyntaxKind::Lt && !self.scan_args() {
            return false;
        }
        while self.eat(SyntaxKind::Dot) {
            if self.kind() != SyntaxKind::Ident {
                return false;
            }
            self.bump();
            if self.kind() == SyntaxKind::Lt && !self.scan_args() {
                return false;
            }
        }
        true
    }

    fn scan_args(&mut self) -> bool {
        if !self.eat(SyntaxKind::Lt) {
            return false;
        }
        if self.is_gt() {
            return self.eat_gt();
        }
        loop {
            if !self.scan_type() {
                return false;
            }
            if self.eat(SyntaxKind::Comma) && !self.is_gt() {
                continue;
            }
            return self.eat_gt();
        }
    }

    fn scan_type_params(&mut self) -> bool {
        if !self.eat(SyntaxKind::Lt) {
            return false;
        }
        loop {
            if self.kind() != SyntaxKind::Ident {
                return false;
            }
            self.bump();
            if self.eat(SyntaxKind::Extends) {
                if !self.scan_type() {
                    return false;
                }
                while self.eat(SyntaxKind::Amp) {
                    if !self.scan_type() {
                        return false;
                    }
                }
            }
            if !self.eat(SyntaxKind::Comma) {
                break;
            }
        }
        self.eat_gt()
    }
}
