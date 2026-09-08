//! ScoreIR/1 surface syntax: a span-preserving lexer and recursive-descent
//! parser. Semantic object/field typing deliberately lives in the validator
//! layer; this module accepts the complete context-free grammar and produces a
//! typed, tagged syntax tree.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use num_rational::BigRational;
use num_traits::ToPrimitive;
use serde_json::{Map, Value as JsonValue};

use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, Span};
use crate::exact::{
    parse_rational_with_limit, rational_parts, Rational, RationalError, MAX_RATIONAL_BITS,
};

pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IDENTIFIER_BYTES: usize = 128;
pub const MAX_NESTING_DEPTH: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseOptions {
    pub max_source_bytes: usize,
    pub max_identifier_bytes: usize,
    pub max_nesting_depth: usize,
    pub max_rational_bits: u64,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            max_source_bytes: MAX_SOURCE_BYTES,
            max_identifier_bytes: MAX_IDENTIFIER_BYTES,
            max_nesting_depth: MAX_NESTING_DEPTH,
            max_rational_bits: MAX_RATIONAL_BITS,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Document {
    pub version: u32,
    pub objects: BTreeMap<String, Object>,
    source: String,
}

impl Document {
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn slice(&self, span: Span) -> &str {
        span.slice(&self.source)
    }

    pub fn object(&self, id: &str) -> Option<&Object> {
        self.objects.get(id)
    }

    pub fn objects(&self) -> impl Iterator<Item = (&str, &Object)> {
        self.objects
            .iter()
            .map(|(id, object)| (id.as_str(), object))
    }

    pub fn to_syntax_json_value(&self) -> JsonValue {
        let mut objects = Map::new();
        for (id, object) in &self.objects {
            objects.insert(id.clone(), object.to_syntax_json_value());
        }
        let mut root = Map::new();
        root.insert("version".to_owned(), JsonValue::from(self.version));
        root.insert("objects".to_owned(), JsonValue::Object(objects));
        JsonValue::Object(root)
    }

    pub fn to_syntax_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.to_syntax_json_value())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Object {
    pub id: String,
    pub kind: String,
    pub span: Span,
    pub kind_span: Span,
    pub id_span: Span,
    pub fields: BTreeMap<String, Field>,
    pub children: BTreeMap<String, Object>,
}

impl Object {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.get(name)
    }

    pub fn child(&self, id: &str) -> Option<&Object> {
        self.children.get(id)
    }

    pub fn to_syntax_json_value(&self) -> JsonValue {
        let mut fields = Map::new();
        for (name, field) in &self.fields {
            fields.insert(name.clone(), field.value.to_syntax_json_value());
        }
        let mut children = Map::new();
        for (id, child) in &self.children {
            children.insert(id.clone(), child.to_syntax_json_value());
        }
        let mut object = Map::new();
        object.insert("kind".to_owned(), JsonValue::String(self.kind.clone()));
        object.insert("fields".to_owned(), JsonValue::Object(fields));
        object.insert("children".to_owned(), JsonValue::Object(children));
        JsonValue::Object(object)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub name: String,
    /// Span from the field name through its terminating semicolon.
    pub span: Span,
    pub name_span: Span,
    pub value: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Value {
    pub kind: ValueKind,
    pub span: Span,
}

impl Value {
    pub fn new(kind: ValueKind, span: Span) -> Self {
        Self { kind, span }
    }

    pub fn as_rational(&self) -> Option<&Rational> {
        match &self.kind {
            ValueKind::Number(value) => Some(value),
            ValueKind::Quantity { value, .. } => Some(value),
            _ => None,
        }
    }

    pub fn unit(&self) -> Option<Unit> {
        match self.kind {
            ValueKind::Quantity { unit, .. } => Some(unit),
            _ => None,
        }
    }

    pub fn reference(&self) -> Option<&Reference> {
        match &self.kind {
            ValueKind::Reference(reference) => Some(reference),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match &self.kind {
            ValueKind::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_symbol(&self) -> Option<&str> {
        match &self.kind {
            ValueKind::Symbol(value) => Some(value),
            _ => None,
        }
    }

    pub fn to_syntax_json_value(&self) -> JsonValue {
        let mut value = Map::new();
        match &self.kind {
            ValueKind::Number(number) => {
                let (n, d) = rational_parts(number);
                value.insert("t".to_owned(), JsonValue::String("number".to_owned()));
                value.insert("n".to_owned(), JsonValue::String(n));
                value.insert("d".to_owned(), JsonValue::String(d));
            }
            ValueKind::Quantity {
                value: number,
                unit,
            } => {
                let (n, d) = rational_parts(number);
                value.insert("t".to_owned(), JsonValue::String("quantity".to_owned()));
                value.insert("n".to_owned(), JsonValue::String(n));
                value.insert("d".to_owned(), JsonValue::String(d));
                value.insert("u".to_owned(), JsonValue::String(unit.as_str().to_owned()));
            }
            ValueKind::String(text) => {
                value.insert("t".to_owned(), JsonValue::String("string".to_owned()));
                value.insert("v".to_owned(), JsonValue::String(text.clone()));
            }
            ValueKind::Symbol(symbol) => {
                value.insert("t".to_owned(), JsonValue::String("symbol".to_owned()));
                value.insert("v".to_owned(), JsonValue::String(symbol.clone()));
            }
            ValueKind::Boolean(boolean) => {
                value.insert("t".to_owned(), JsonValue::String("boolean".to_owned()));
                value.insert("v".to_owned(), JsonValue::Bool(*boolean));
            }
            ValueKind::Reference(reference) => {
                value.insert("t".to_owned(), JsonValue::String("ref".to_owned()));
                value.insert(
                    "path".to_owned(),
                    JsonValue::Array(
                        reference
                            .path
                            .iter()
                            .cloned()
                            .map(JsonValue::String)
                            .collect(),
                    ),
                );
                value.insert(
                    "port".to_owned(),
                    reference
                        .port
                        .as_ref()
                        .map(|port| JsonValue::String(port.clone()))
                        .unwrap_or(JsonValue::Null),
                );
            }
            ValueKind::Call { function, args } => {
                value.insert("t".to_owned(), JsonValue::String("call".to_owned()));
                value.insert("fn".to_owned(), JsonValue::String(function.clone()));
                value.insert(
                    "args".to_owned(),
                    JsonValue::Array(args.iter().map(Value::to_syntax_json_value).collect()),
                );
            }
            ValueKind::List(items) => {
                value.insert("t".to_owned(), JsonValue::String("list".to_owned()));
                value.insert(
                    "items".to_owned(),
                    JsonValue::Array(items.iter().map(Value::to_syntax_json_value).collect()),
                );
            }
            ValueKind::Tuple(items) => {
                value.insert("t".to_owned(), JsonValue::String("tuple".to_owned()));
                value.insert(
                    "items".to_owned(),
                    JsonValue::Array(items.iter().map(Value::to_syntax_json_value).collect()),
                );
            }
            ValueKind::Record(fields) => {
                let mut record = Map::new();
                for (name, field) in fields {
                    record.insert(name.clone(), field.value.to_syntax_json_value());
                }
                value.insert("t".to_owned(), JsonValue::String("record".to_owned()));
                value.insert("fields".to_owned(), JsonValue::Object(record));
            }
        }
        JsonValue::Object(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValueKind {
    Number(Rational),
    Quantity { value: Rational, unit: Unit },
    String(String),
    Symbol(String),
    Boolean(bool),
    Reference(Reference),
    Call { function: String, args: Vec<Value> },
    List(Vec<Value>),
    Tuple(Vec<Value>),
    Record(BTreeMap<String, Field>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reference {
    pub path: Vec<String>,
    pub port: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Unit {
    Q,
    S,
    Ms,
    Frame,
    Hz,
    KHz,
    Bpm,
    Ct,
    Db,
}

impl Unit {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Q => "q",
            Self::S => "s",
            Self::Ms => "ms",
            Self::Frame => "frame",
            Self::Hz => "Hz",
            Self::KHz => "kHz",
            Self::Bpm => "bpm",
            Self::Ct => "ct",
            Self::Db => "dB",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "q" => Self::Q,
            "s" => Self::S,
            "ms" => Self::Ms,
            "frame" => Self::Frame,
            "Hz" => Self::Hz,
            "kHz" => Self::KHz,
            "bpm" => Self::Bpm,
            "ct" => Self::Ct,
            "dB" => Self::Db,
            _ => return None,
        })
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    Ident,
    /// A pitch token has lexical precedence in value positions. Plain pitch
    /// spellings such as `C4` are still accepted where an identifier is
    /// expected because they match the identifier grammar; accidental/sign
    /// spellings cannot become object or field IDs.
    Pitch,
    Number,
    String,
    Ampersand,
    Colon,
    Dot,
    Comma,
    Semi,
    Equal,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Slash,
    Eof,
    Invalid,
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    span: Span,
}

struct Lexer<'a> {
    source: &'a str,
    options: &'a ParseOptions,
    cursor: usize,
    diagnostics: Diagnostics,
    tokens: Vec<Token>,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str, options: &'a ParseOptions) -> Self {
        Self {
            source,
            options,
            cursor: 0,
            diagnostics: Diagnostics::new(),
            tokens: Vec::new(),
        }
    }

    fn lex(mut self) -> (Vec<Token>, Diagnostics) {
        let bytes = self.source.as_bytes();
        while self.cursor < bytes.len() {
            let start = self.cursor;
            let byte = bytes[self.cursor];
            if byte.is_ascii_whitespace() {
                self.cursor += 1;
                continue;
            }
            if byte == b'/' && self.peek_byte(1) == Some(b'/') {
                self.cursor += 2;
                while self.cursor < bytes.len() && bytes[self.cursor] != b'\n' {
                    self.cursor += 1;
                }
                continue;
            }
            if byte == b'/' && self.peek_byte(1) == Some(b'*') {
                self.cursor += 2;
                let mut closed = false;
                while self.cursor + 1 < bytes.len() {
                    if bytes[self.cursor] == b'*' && bytes[self.cursor + 1] == b'/' {
                        self.cursor += 2;
                        closed = true;
                        break;
                    }
                    self.cursor += 1;
                }
                if !closed {
                    self.cursor = bytes.len();
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::Syntax,
                        "unterminated block comment",
                        Some(Span::new(start, bytes.len())),
                    ));
                }
                continue;
            }
            if byte == b'"' {
                self.lex_string(start);
                continue;
            }
            if byte.is_ascii_digit()
                || ((byte == b'+' || byte == b'-')
                    && self.peek_byte(1).is_some_and(|next| next.is_ascii_digit()))
            {
                self.lex_number(start);
                continue;
            }
            if byte.is_ascii_alphabetic() || byte == b'_' {
                self.lex_identifier(start);
                continue;
            }
            let kind = match byte {
                b'&' => TokenKind::Ampersand,
                b':' => TokenKind::Colon,
                b'.' => TokenKind::Dot,
                b',' => TokenKind::Comma,
                b';' => TokenKind::Semi,
                b'=' => TokenKind::Equal,
                b'{' => TokenKind::LBrace,
                b'}' => TokenKind::RBrace,
                b'[' => TokenKind::LBracket,
                b']' => TokenKind::RBracket,
                b'(' => TokenKind::LParen,
                b')' => TokenKind::RParen,
                b'/' => TokenKind::Slash,
                _ => {
                    let width = self.source[self.cursor..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(1);
                    self.cursor += width;
                    self.tokens.push(Token {
                        kind: TokenKind::Invalid,
                        span: Span::new(start, self.cursor),
                    });
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::Syntax,
                        format!(
                            "unexpected character {:?}",
                            &self.source[start..self.cursor]
                        ),
                        Some(Span::new(start, self.cursor)),
                    ));
                    continue;
                }
            };
            self.cursor += 1;
            self.tokens.push(Token {
                kind,
                span: Span::new(start, self.cursor),
            });
        }
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(self.source.len(), self.source.len()),
        });
        (self.tokens, self.diagnostics)
    }

    fn peek_byte(&self, offset: usize) -> Option<u8> {
        self.source.as_bytes().get(self.cursor + offset).copied()
    }

    fn lex_identifier(&mut self, start: usize) {
        let bytes = self.source.as_bytes();
        self.cursor += 1;
        while self.cursor < bytes.len()
            && (bytes[self.cursor].is_ascii_alphanumeric() || bytes[self.cursor] == b'_')
        {
            self.cursor += 1;
        }
        // Pitch spellings may include accidental/sign characters that are not
        // identifier characters. Scan all A-G spellings, including C-1 and
        // A+4, and retain a distinct token kind for identifier contexts.
        let pitch = if matches!(bytes[start], b'A'..=b'G') {
            pitch_end(self.source, start)
        } else {
            None
        };
        if let Some(end) = pitch {
            self.cursor = end;
        }
        let span = Span::new(start, self.cursor);
        let text = span.slice(self.source);
        if text.len() > self.options.max_identifier_bytes {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::ResourceLimit,
                format!(
                    "identifier exceeds {} bytes",
                    self.options.max_identifier_bytes
                ),
                Some(span),
            ));
        }
        self.tokens.push(Token {
            kind: if pitch.is_some() {
                TokenKind::Pitch
            } else {
                TokenKind::Ident
            },
            span,
        });
    }

    fn lex_number(&mut self, start: usize) {
        let bytes = self.source.as_bytes();
        if matches!(bytes.get(self.cursor), Some(b'+' | b'-')) {
            self.cursor += 1;
        }
        while self.cursor < bytes.len() && bytes[self.cursor].is_ascii_digit() {
            self.cursor += 1;
        }
        if matches!(bytes.get(self.cursor), Some(b'/' | b'.')) {
            self.cursor += 1;
            while self.cursor < bytes.len() && bytes[self.cursor].is_ascii_digit() {
                self.cursor += 1;
            }
        }
        self.tokens.push(Token {
            kind: TokenKind::Number,
            span: Span::new(start, self.cursor),
        });
    }

    fn lex_string(&mut self, start: usize) {
        let bytes = self.source.as_bytes();
        self.cursor += 1;
        let mut closed = false;
        while self.cursor < bytes.len() {
            match bytes[self.cursor] {
                b'"' => {
                    self.cursor += 1;
                    closed = true;
                    break;
                }
                b'\\' => {
                    self.cursor += 1;
                    if self.cursor < bytes.len() {
                        self.cursor += 1;
                    }
                }
                byte if byte < 0x20 => {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::Syntax,
                        "unescaped control character in string",
                        Some(Span::new(start, self.cursor + 1)),
                    ));
                    self.cursor += 1;
                }
                _ => self.cursor += 1,
            }
        }
        let span = Span::new(start, self.cursor);
        if !closed {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::Syntax,
                "unterminated string",
                Some(span),
            ));
        } else if let Err(message) = decode_string(span.slice(self.source)) {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::Syntax,
                message,
                Some(span),
            ));
        }
        self.tokens.push(Token {
            kind: if closed {
                TokenKind::String
            } else {
                TokenKind::Invalid
            },
            span,
        });
    }
}

fn pitch_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if !matches!(bytes.get(start), Some(b'A'..=b'G')) {
        return None;
    }
    let mut cursor = start + 1;
    if matches!(bytes.get(cursor), Some(b'#' | b'b')) {
        let accidental = bytes[cursor];
        cursor += 1;
        if bytes.get(cursor) == Some(&accidental) {
            cursor += 1;
        }
    }
    if matches!(bytes.get(cursor), Some(b'+' | b'-')) {
        cursor += 1;
    }
    let digit_start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    if cursor == digit_start {
        return None;
    }
    let boundary = bytes.get(cursor).copied();
    if boundary.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        None
    } else {
        Some(cursor)
    }
}

fn decode_string(raw: &str) -> Result<String, String> {
    let text: String =
        serde_json::from_str(raw).map_err(|error| format!("invalid JSON string: {error}"))?;
    if text
        .chars()
        .any(|ch| (0xD800..=0xDFFF).contains(&(ch as u32)))
    {
        return Err("unpaired surrogate in string".to_owned());
    }
    Ok(text)
}

struct Parser<'a> {
    source: &'a str,
    options: ParseOptions,
    tokens: Vec<Token>,
    cursor: usize,
    diagnostics: Diagnostics,
    object_path: Vec<String>,
}

impl<'a> Parser<'a> {
    fn new(
        source: &'a str,
        options: ParseOptions,
        tokens: Vec<Token>,
        diagnostics: Diagnostics,
    ) -> Self {
        Self {
            source,
            options,
            tokens,
            cursor: 0,
            diagnostics,
            object_path: Vec::new(),
        }
    }

    fn parse_document(mut self) -> Result<Document, Diagnostics> {
        if !self.expect_text("maac", "document header") {
            return Err(self.diagnostics);
        }
        let version_token = match self.expect_kind(TokenKind::Number, "language version") {
            Some(token) => token,
            None => return Err(self.diagnostics),
        };
        let version_text = version_token.span.slice(self.source);
        let version = match parse_rational_with_limit(version_text, self.options.max_rational_bits)
        {
            Ok(number)
                if version_text.bytes().all(|byte| byte.is_ascii_digit())
                    && number.denom() == &num_bigint::BigInt::from(1u8)
                    && number >= BigRational::from_integer(0.into()) =>
            {
                number.numer().to_u32().unwrap_or(0)
            }
            Ok(_) => {
                self.push_error(
                    DiagnosticCode::Syntax,
                    "document version must be an unsigned integer",
                    Some(version_token.span),
                );
                0
            }
            Err(error) => {
                self.push_rational_error(error, version_token.span);
                0
            }
        };
        self.expect_kind(TokenKind::Semi, "after document version");
        if version != 1 {
            self.push_error(
                DiagnosticCode::Version,
                "only ScoreIR language version 1 is supported",
                Some(version_token.span),
            );
        }

        let mut objects = BTreeMap::new();
        while !self.at(TokenKind::Eof) {
            let Some(object) = self.parse_object(1) else {
                self.recover_member();
                if self.at(TokenKind::Eof) {
                    break;
                }
                continue;
            };
            if objects.contains_key(&object.id) {
                self.push_error(
                    DiagnosticCode::DuplicateId,
                    format!("duplicate top-level object ID `{}`", object.id),
                    Some(object.id_span),
                );
            } else {
                objects.insert(object.id.clone(), object);
            }
        }
        if self.diagnostics.has_errors() {
            Err(self.diagnostics)
        } else {
            Ok(Document {
                version,
                objects,
                source: self.source.to_owned(),
            })
        }
    }

    fn parse_object(&mut self, depth: usize) -> Option<Object> {
        if depth > self.options.max_nesting_depth {
            let span = self.current().span;
            self.push_error(
                DiagnosticCode::ResourceLimit,
                format!("nesting exceeds {} levels", self.options.max_nesting_depth),
                Some(span),
            );
            // The enclosing member loop must make progress after rejecting a
            // deep child. Consume its kind token before returning to recovery.
            if !self.at(TokenKind::Eof) {
                self.cursor += 1;
            }
            return None;
        }
        let kind_token = self.expect_identifier("object kind")?;
        let id_token = self.expect_identifier("object identifier")?;
        let kind = kind_token.span.slice(self.source).to_owned();
        let id = id_token.span.slice(self.source).to_owned();
        self.expect_kind(TokenKind::LBrace, "after object identifier")?;
        self.object_path.push(id.clone());
        let mut fields = BTreeMap::new();
        let mut children = BTreeMap::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if !self.current_is_identifier() {
                self.push_error(
                    DiagnosticCode::Syntax,
                    "expected a field or child object",
                    Some(self.current().span),
                );
                self.recover_member();
                continue;
            }
            if self.peek_is_identifier(0) && self.peek_kind(1) == Some(TokenKind::Equal) {
                if let Some(field) = self.parse_field(depth) {
                    if fields.contains_key(&field.name) {
                        self.push_error(
                            DiagnosticCode::DuplicateField,
                            format!("duplicate field `{}`", field.name),
                            Some(field.name_span),
                        );
                    } else {
                        fields.insert(field.name.clone(), field);
                    }
                }
            } else if self.peek_is_identifier(0) && self.peek_is_identifier(1) {
                if let Some(child) = self.parse_object(depth + 1) {
                    if children.contains_key(&child.id) {
                        self.push_error(
                            DiagnosticCode::DuplicateId,
                            format!("duplicate child ID `{}`", child.id),
                            Some(child.id_span),
                        );
                    } else {
                        children.insert(child.id.clone(), child);
                    }
                }
            } else {
                self.push_error(
                    DiagnosticCode::Syntax,
                    "expected `=` for a field or an identifier for a child object",
                    Some(self.current().span),
                );
                self.recover_member();
            }
        }
        let close = self.expect_kind(TokenKind::RBrace, "to close object")?;
        self.object_path.pop();
        for name in fields.keys() {
            if children.contains_key(name) {
                self.push_error(
                    DiagnosticCode::DuplicateId,
                    format!("field and child cannot share ID `{name}`"),
                    Some(fields[name].name_span),
                );
            }
        }
        Some(Object {
            id,
            kind,
            span: Span::new(kind_token.span.start, close.span.end),
            kind_span: kind_token.span,
            id_span: id_token.span,
            fields,
            children,
        })
    }

    fn parse_field(&mut self, depth: usize) -> Option<Field> {
        let name_token = self.expect_identifier("field name")?;
        let name = name_token.span.slice(self.source).to_owned();
        self.expect_kind(TokenKind::Equal, "after field name")?;
        // `depth` is the current object/record nesting level. A composite
        // value at the field belongs to that level; its children increment it
        // in their own parser routines.
        let value = self.parse_value(depth)?;
        let semi = self.expect_kind(TokenKind::Semi, "after field value")?;
        Some(Field {
            name,
            span: Span::new(name_token.span.start, semi.span.end),
            name_span: name_token.span,
            value,
        })
    }

    fn parse_value(&mut self, depth: usize) -> Option<Value> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Number => {
                self.cursor += 1;
                let raw = token.span.slice(self.source);
                let number = match parse_rational_with_limit(raw, self.options.max_rational_bits) {
                    Ok(number) => number,
                    Err(error) => {
                        self.push_rational_error(error, token.span);
                        return None;
                    }
                };
                if let Some(TokenKind::Ident) = self.peek_kind(0) {
                    let unit_text = self.current().span.slice(self.source);
                    if let Some(unit) = Unit::parse(unit_text) {
                        let unit_token = self.current().clone();
                        self.cursor += 1;
                        return Some(Value::new(
                            ValueKind::Quantity {
                                value: number,
                                unit,
                            },
                            Span::new(token.span.start, unit_token.span.end),
                        ));
                    }
                }
                Some(Value::new(ValueKind::Number(number), token.span))
            }
            TokenKind::String => {
                self.cursor += 1;
                match decode_string(token.span.slice(self.source)) {
                    Ok(value) => Some(Value::new(ValueKind::String(value), token.span)),
                    Err(message) => {
                        self.push_error(DiagnosticCode::Syntax, message, Some(token.span));
                        None
                    }
                }
            }
            TokenKind::Ident | TokenKind::Pitch => {
                self.cursor += 1;
                let text = token.span.slice(self.source).to_owned();
                if token.kind == TokenKind::Ident && text == "true" {
                    Some(Value::new(ValueKind::Boolean(true), token.span))
                } else if token.kind == TokenKind::Ident && text == "false" {
                    Some(Value::new(ValueKind::Boolean(false), token.span))
                } else if token.kind == TokenKind::Ident && self.at(TokenKind::LParen) {
                    self.parse_call(text, token.span, depth)
                } else {
                    Some(Value::new(ValueKind::Symbol(text), token.span))
                }
            }
            TokenKind::Ampersand => self.parse_reference(),
            TokenKind::LBracket => self.parse_list(depth),
            TokenKind::LParen => self.parse_tuple(depth),
            TokenKind::LBrace => self.parse_record(depth),
            _ => {
                self.push_error(DiagnosticCode::Syntax, "expected a value", Some(token.span));
                None
            }
        }
    }

    fn parse_reference(&mut self) -> Option<Value> {
        let start = self
            .expect_kind(TokenKind::Ampersand, "reference marker")?
            .span
            .start;
        let first = self.expect_identifier("reference path")?;
        let mut path = vec![first.span.slice(self.source).to_owned()];
        let mut end = first.span.end;
        while self.at(TokenKind::Dot) {
            self.cursor += 1;
            let segment = self.expect_identifier("reference path segment")?;
            end = segment.span.end;
            path.push(segment.span.slice(self.source).to_owned());
        }
        let port = if self.at(TokenKind::Colon) {
            self.cursor += 1;
            let port = self.expect_identifier("reference port")?;
            end = port.span.end;
            Some(port.span.slice(self.source).to_owned())
        } else {
            None
        };
        Some(Value::new(
            ValueKind::Reference(Reference { path, port }),
            Span::new(start, end),
        ))
    }

    fn parse_call(&mut self, function: String, start: Span, depth: usize) -> Option<Value> {
        self.ensure_nesting(depth, start)?;
        self.cursor += 1; // '('
        let mut args = Vec::new();
        if !self.at(TokenKind::RParen) {
            loop {
                args.push(self.parse_value(depth + 1)?);
                if !self.at(TokenKind::Comma) {
                    break;
                }
                self.cursor += 1;
                if self.at(TokenKind::RParen) {
                    break;
                }
            }
        }
        let close = self.expect_kind(TokenKind::RParen, "to close constructor")?;
        Some(Value::new(
            ValueKind::Call { function, args },
            Span::new(start.start, close.span.end),
        ))
    }

    fn parse_list(&mut self, depth: usize) -> Option<Value> {
        let open = self.expect_kind(TokenKind::LBracket, "list")?;
        self.ensure_nesting(depth, open.span)?;
        let mut items = Vec::new();
        if !self.at(TokenKind::RBracket) {
            loop {
                items.push(self.parse_value(depth + 1)?);
                if !self.at(TokenKind::Comma) {
                    break;
                }
                self.cursor += 1;
                if self.at(TokenKind::RBracket) {
                    break;
                }
            }
        }
        let close = self.expect_kind(TokenKind::RBracket, "to close list")?;
        Some(Value::new(
            ValueKind::List(items),
            Span::new(open.span.start, close.span.end),
        ))
    }

    fn parse_tuple(&mut self, depth: usize) -> Option<Value> {
        let open = self.expect_kind(TokenKind::LParen, "tuple")?;
        self.ensure_nesting(depth, open.span)?;
        let first = self.parse_value(depth + 1)?;
        self.expect_kind(TokenKind::Comma, "between tuple items")?;
        let second = self.parse_value(depth + 1)?;
        let mut items = vec![first, second];
        while self.at(TokenKind::Comma) {
            self.cursor += 1;
            if self.at(TokenKind::RParen) {
                break;
            }
            items.push(self.parse_value(depth + 1)?);
        }
        let close = self.expect_kind(TokenKind::RParen, "to close tuple")?;
        Some(Value::new(
            ValueKind::Tuple(items),
            Span::new(open.span.start, close.span.end),
        ))
    }

    fn parse_record(&mut self, depth: usize) -> Option<Value> {
        let open = self.expect_kind(TokenKind::LBrace, "record")?;
        self.ensure_nesting(depth, open.span)?;
        let mut fields = BTreeMap::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let Some(field) = self.parse_field(depth) else {
                self.recover_member();
                continue;
            };
            if fields.contains_key(&field.name) {
                self.push_error(
                    DiagnosticCode::DuplicateField,
                    format!("duplicate record field `{}`", field.name),
                    Some(field.name_span),
                );
            } else {
                fields.insert(field.name.clone(), field);
            }
        }
        let close = self.expect_kind(TokenKind::RBrace, "to close record")?;
        Some(Value::new(
            ValueKind::Record(fields),
            Span::new(open.span.start, close.span.end),
        ))
    }

    fn ensure_nesting(&mut self, depth: usize, span: Span) -> Option<()> {
        if depth > self.options.max_nesting_depth {
            self.push_error(
                DiagnosticCode::ResourceLimit,
                format!("nesting exceeds {} levels", self.options.max_nesting_depth),
                Some(span),
            );
            None
        } else {
            Some(())
        }
    }

    fn recover_member(&mut self) {
        if self.at(TokenKind::RBrace) {
            // A stray closing brace at document level (or after a rejected
            // deeply nested child) must not leave top-level recovery parked on
            // the same token forever.
            self.cursor += 1;
            return;
        }
        while !self.at(TokenKind::Eof) && !self.at(TokenKind::RBrace) && !self.at(TokenKind::Semi) {
            self.cursor += 1;
        }
        if self.at(TokenKind::Semi) {
            self.cursor += 1;
        }
    }

    fn current(&self) -> &Token {
        &self.tokens[self.cursor.min(self.tokens.len().saturating_sub(1))]
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.current().kind == kind
    }

    fn peek_kind(&self, offset: usize) -> Option<TokenKind> {
        self.tokens
            .get(self.cursor + offset)
            .map(|token| token.kind.clone())
    }

    fn current_is_identifier(&self) -> bool {
        self.peek_is_identifier(0)
    }

    fn peek_is_identifier(&self, offset: usize) -> bool {
        let Some(token) = self.tokens.get(self.cursor + offset) else {
            return false;
        };
        if token.kind == TokenKind::Ident {
            return true;
        }
        token.kind == TokenKind::Pitch
            && token
                .span
                .slice(self.source)
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }

    fn expect_identifier(&mut self, context: &str) -> Option<Token> {
        if self.current_is_identifier() {
            let token = self.current().clone();
            self.cursor += 1;
            Some(token)
        } else {
            self.push_error(
                DiagnosticCode::Syntax,
                format!("expected {context}, found {}", self.describe_current()),
                Some(self.current().span),
            );
            None
        }
    }

    fn expect_kind(&mut self, kind: TokenKind, context: &str) -> Option<Token> {
        if self.at(kind.clone()) {
            let token = self.current().clone();
            self.cursor += 1;
            Some(token)
        } else {
            self.push_error(
                DiagnosticCode::Syntax,
                format!("expected {context}, found {}", self.describe_current()),
                Some(self.current().span),
            );
            None
        }
    }

    fn expect_text(&mut self, text: &str, context: &str) -> bool {
        if self.at(TokenKind::Ident) && self.current().span.slice(self.source) == text {
            self.cursor += 1;
            true
        } else {
            self.push_error(
                DiagnosticCode::Syntax,
                format!("expected {context} `{text}`"),
                Some(self.current().span),
            );
            false
        }
    }

    fn describe_current(&self) -> String {
        if self.at(TokenKind::Eof) {
            "end of input".to_owned()
        } else {
            format!("`{}`", self.current().span.slice(self.source))
        }
    }

    fn push_error(&mut self, code: DiagnosticCode, message: impl Into<String>, span: Option<Span>) {
        self.diagnostics
            .push(Diagnostic::error(code, message, span).object_path(self.object_path.clone()));
    }

    fn push_rational_error(&mut self, error: RationalError, span: Span) {
        let (code, message) = match error {
            RationalError::InvalidSyntax => (DiagnosticCode::Syntax, "invalid rational literal"),
            RationalError::ZeroDenominator => (
                DiagnosticCode::Syntax,
                "rational denominator must be positive",
            ),
            RationalError::ResourceLimit => (
                DiagnosticCode::ResourceLimit,
                "rational exceeds the configured bit limit",
            ),
        };
        self.push_error(code, message, Some(span));
    }
}

/// Parse source with foundation defaults.
pub fn parse(source: &str) -> Result<Document, Diagnostics> {
    parse_with_options(source, ParseOptions::default())
}

pub fn parse_with_options(source: &str, options: ParseOptions) -> Result<Document, Diagnostics> {
    if source.len() > options.max_source_bytes {
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::ResourceLimit,
            format!("source exceeds {} bytes", options.max_source_bytes),
            Some(Span::new(options.max_source_bytes, source.len())),
        ));
        return Err(diagnostics);
    }
    let (tokens, diagnostics) = Lexer::new(source, &options).lex();
    let parser = Parser::new(source, options, tokens, diagnostics);
    parser.parse_document()
}

/// Convenience API for callers that own filesystem access. The parser itself
/// remains deterministic and receives only UTF-8 source bytes.
pub fn parse_file(path: impl AsRef<Path>) -> Result<Document, ParseFileError> {
    parse_file_with_options(path, ParseOptions::default())
}

pub fn parse_file_with_options(
    path: impl AsRef<Path>,
    options: ParseOptions,
) -> Result<Document, ParseFileError> {
    let path = path.as_ref();
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(ParseFileError::Io)?;
    let mut bytes = Vec::new();
    file.take(options.max_source_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ParseFileError::Io)?;
    if bytes.len() > options.max_source_bytes {
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::ResourceLimit,
            format!("source exceeds {} bytes", options.max_source_bytes),
            Some(Span::new(options.max_source_bytes, bytes.len())),
        ));
        return Err(ParseFileError::Diagnostics(diagnostics));
    }
    let source = String::from_utf8(bytes).map_err(|error| ParseFileError::InvalidUtf8 {
        offset: error.utf8_error().valid_up_to(),
    })?;
    parse_with_options(&source, options).map_err(ParseFileError::Diagnostics)
}

#[derive(Debug)]
pub enum ParseFileError {
    Io(std::io::Error),
    InvalidUtf8 { offset: usize },
    Diagnostics(Diagnostics),
}

impl fmt::Display for ParseFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cannot read ScoreIR source: {error}"),
            Self::InvalidUtf8 { offset } => {
                write!(f, "ScoreIR source is not UTF-8 at byte {offset}")
            }
            Self::Diagnostics(diagnostics) => diagnostics.fmt(f),
        }
    }
}

impl std::error::Error for ParseFileError {}
