//! A small RFC 8259 JSON reader for `import` (P2-504): recursive
//! descent into `Value`, std only, no serde (no new dependency).

use crate::text::{self, Position};

/// One JSON value. Objects keep their key order.
// DECISION: numbers are kept as `f64`; positions and sizes are rounded to
// `i32` on use (`Value::as_i32`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The member `key` of an object; `None` for a missing key or a
    /// value that is not an object.
    // DECISION: a duplicate key takes its last value, as common JSON
    // readers do.
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The elements of an array; empty for anything else.
    pub(crate) fn items(&self) -> &[Value] {
        match self {
            Value::Array(items) => items,
            _ => &[],
        }
    }

    /// A string's text.
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// A number rounded to the nearest `i32`, `None` for anything else
    /// or a value outside `i32`.
    pub(crate) fn as_i32(&self) -> Option<i32> {
        match self {
            #[allow(clippy::cast_possible_truncation)] // range checked just before
            Value::Number(n) if n.is_finite() && n.abs() <= f64::from(i32::MAX) => Some(n.round() as i32),
            _ => None,
        }
    }
}

/// A syntax error: where, and what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyntaxError {
    pub(crate) position: Position,
    pub(crate) message: String,
}

/// Parses `source` as one JSON text.
pub(crate) fn parse(source: &str) -> Result<Value, SyntaxError> {
    let mut reader = Reader { src: source, bytes: source.as_bytes(), at: 0, depth: 0 };
    reader.skip_ws();
    let value = reader.value()?;
    reader.skip_ws();
    if reader.at < reader.bytes.len() {
        return Err(reader.error("text after the JSON value — remove it"));
    }
    Ok(value)
}

/// How deep arrays and objects may nest before the reader gives up
/// instead of overflowing the stack.
const MAX_DEPTH: u32 = 512;

struct Reader<'a> {
    src: &'a str,
    bytes: &'a [u8],
    at: usize,
    depth: u32,
}

impl Reader<'_> {
    fn error(&self, message: &str) -> SyntaxError {
        let offset = u32::try_from(self.at).unwrap_or(u32::MAX);
        SyntaxError { position: text::position(self.src, offset), message: message.to_owned() }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn value(&mut self) -> Result<Value, SyntaxError> {
        match self.peek() {
            None => Err(self.error("the JSON text ends where a value is expected — complete it")),
            Some(b'{') => self.nested(Self::object),
            Some(b'[') => self.nested(Self::array),
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => self.word("true", Value::Bool(true)),
            Some(b'f') => self.word("false", Value::Bool(false)),
            Some(b'n') => self.word("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("unexpected character where a value is expected — write a JSON value")),
        }
    }

    fn nested(&mut self, f: fn(&mut Self) -> Result<Value, SyntaxError>) -> Result<Value, SyntaxError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("arrays and objects nest too deep — flatten the file"));
        }
        let value = f(self);
        self.depth -= 1;
        value
    }

    fn word(&mut self, word: &str, value: Value) -> Result<Value, SyntaxError> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("unknown word where a value is expected — write true, false or null"))
        }
    }

    fn object(&mut self) -> Result<Value, SyntaxError> {
        self.at += 1; // `{`
        let mut members = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Value::Object(members));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.error("an object member must start with a quoted key — add `\"key\":`"));
            }
            let key = self.string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.error("missing `:` after an object key — add it"));
            }
            self.at += 1;
            self.skip_ws();
            let value = self.value()?;
            members.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(members));
                }
                None => return Err(self.error("the object is not closed — add `}`")),
                Some(_) => return Err(self.error("expected `,` or `}` after an object member — add one")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, SyntaxError> {
        self.at += 1; // `[`
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                None => return Err(self.error("the array is not closed — add `]`")),
                Some(_) => return Err(self.error("expected `,` or `]` after an array element — add one")),
            }
        }
    }

    fn string(&mut self) -> Result<String, SyntaxError> {
        self.at += 1; // opening quote
        let mut out = String::new();
        loop {
            let start = self.at;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.at += 1;
            }
            out.push_str(&self.src[start..self.at]);
            match self.peek() {
                None => return Err(self.error("the string is not closed — add `\"`")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let c = self.escape()?;
                    out.push(c);
                }
                Some(_) => return Err(self.error("a control character inside a string — escape it")),
            }
        }
    }

    fn escape(&mut self) -> Result<char, SyntaxError> {
        let Some(b) = self.peek() else {
            return Err(self.error("the string is not closed — add `\"`"));
        };
        self.at += 1;
        Ok(match b {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.unicode(),
            _ => {
                self.at -= 1;
                return Err(self.error("unknown escape — use \\\" \\\\ \\/ \\b \\f \\n \\r \\t or \\uXXXX"));
            }
        })
    }

    fn hex4(&mut self) -> Result<u32, SyntaxError> {
        let digits = self.bytes.get(self.at..self.at + 4).filter(|d| d.iter().all(u8::is_ascii_hexdigit));
        let Some(digits) = digits else {
            return Err(self.error("`\\u` needs four hex digits — write them"));
        };
        let value = digits.iter().fold(0, |acc, &d| acc * 16 + char::from(d).to_digit(16).unwrap_or(0));
        self.at += 4;
        Ok(value)
    }

    fn unicode(&mut self) -> Result<char, SyntaxError> {
        let first = self.hex4()?;
        let code = if (0xD800..0xDC00).contains(&first) {
            if !self.bytes[self.at..].starts_with(b"\\u") {
                return Err(self.error("a high surrogate needs a following `\\uDC00`..`\\uDFFF` — add it"));
            }
            self.at += 2;
            let second = self.hex4()?;
            if !(0xDC00..0xE000).contains(&second) {
                return Err(self.error("a high surrogate needs a following `\\uDC00`..`\\uDFFF` — add it"));
            }
            0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
        } else {
            first
        };
        char::from_u32(code).ok_or_else(|| self.error("a lone low surrogate — pair it with a high surrogate"))
    }

    fn number(&mut self) -> Result<Value, SyntaxError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.error("a number needs a digit — write one")),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("a fraction needs a digit after `.` — write one"));
            }
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("an exponent needs a digit — write one"));
            }
            self.digits();
        }
        self.src[start..self.at]
            .parse::<f64>()
            .map(Value::Number)
            .map_err(|_| self.error("not a number — check it"))
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
    }
}
