//! Lossless lexer and syntax tree for the diagram file. Every byte,
//! comments and blank lines included, is kept with its span, so printing
//! the tree gives the input back (DSL-11). Every span carries a
//! source-file id (DSL-16). Parse errors carry line and column (DSL-4).
//!
//! Depends on: nothing.

// The tree is generic on purpose: a statement is "the tokens of one
// line, optionally followed by a `{ ... }` block". Keywords (`person`,
// `views`, `at`, `deploymentNode`, ...) are plain `Word` tokens; `model`
// gives them meaning and rejects what phase 1 does not know (DSL-4), so
// phase 2 parses the full Structurizr grammar (P2-1) without changing
// this API.
//
// DECISION: comments open only as the first non-trivia on their line
// (`//`, `#`, `/*`), as the Structurizr DSL parser does: it treats a line
// as a comment only when it starts with one. Opening them mid-line would
// clash with values the DSL writes unquoted: `#0b3c6f` (a `#rrggbb`
// colour) and `https://x` (a URL) stay `Word`s.
// DECISION: escapes are `\"` and `\\` only (SRS 4.1); any other `\x` in
// a string is `ParseErrorKind::InvalidEscape` (DSL-4: nothing outside
// the table passes silently).
// DECISION: `\n` and `\r\n` end a line and count once for positions; a
// lone `\r` is whitespace.
// DECISION: SRS 5.1 wants errors to carry the file. `text` has no I/O
// and never sees a path: spans carry a `FileId` (DSL-16) and the caller
// maps it to a path when printing.
//
// Line continuations and `"""` text blocks: see the SRS 4.1 parity table.

use std::fmt;
use std::ops::Range;

pub(crate) mod json;

/// Which source file a span points into (DSL-16). In phase 1 every span
/// is in the root file, `FileId::ROOT`; `!include` (P2-5) numbers the
/// included files from 1 in the order they are first read.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct FileId(pub u32);

impl FileId {
    /// The file the user opened. The only file in phase 1 (DSL-16).
    pub const ROOT: FileId = FileId(0);
}

/// A half-open byte range `start..end` in one source file (DSL-16).
///
/// Byte offsets, not characters, so `edit` can splice byte patches by
/// span (DSL-9, DSL-10, DSL-13) and printing stays exact (DSL-11).
// DECISION: u32 offsets keep `Span` at 12 bytes; files over 4 GiB are a
// parse error (`ParseErrorKind::FileTooLarge`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Span {
    /// The file the bytes are in (DSL-16).
    pub file: FileId,
    /// First byte, inclusive.
    pub start: u32,
    /// One past the last byte.
    pub end: u32,
}

impl Span {
    /// The span `start..end` in `file`. Serves DSL-11, DSL-16.
    pub const fn new(file: FileId, start: u32, end: u32) -> Span {
        Span { file, start, end }
    }

    /// The byte range as `usize`, for slicing that file's source:
    /// `&source[span.range()]`. Serves DSL-9, DSL-10, DSL-11.
    pub const fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }

    /// Length in bytes. Serves DSL-11.
    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// True when the span covers no byte. Serves DSL-11.
    pub const fn is_empty(self) -> bool {
        self.end <= self.start
    }
}

/// A 1-based line and column. Columns count characters, not bytes, so
/// non-ASCII text reports the column a person sees (DSL-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Position {
    /// Line number, from 1. `\n` ends a line; `\r\n` counts once.
    pub line: u32,
    /// Column number in characters, from 1.
    pub column: u32,
}

/// The line and column of byte `offset` in `source`. Used by `text`
/// for its own errors and by `model` for its errors (DSL-4, DSL-5).
pub fn position(source: &str, offset: u32) -> Position {
    // DECISION: an offset past the end clamps to the end, and one inside a
    // multi-byte character counts that character as started: work on bytes
    // so no slicing can panic.
    let bytes = &source.as_bytes()[..(offset as usize).min(source.len())];
    let line_start = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    // Pieces between newlines: one more than the newlines, i.e. the line.
    let line = bytes.split(|&b| b == b'\n').count();
    // UTF-8 continuation bytes are 0b10xx_xxxx; every other byte starts a char.
    let chars = bytes[line_start..]
        .iter()
        .filter(|&&b| b & 0xC0 != 0x80)
        .count();
    Position {
        line: to_u32(line),
        column: to_u32(chars + 1),
    }
}

/// `usize` to `u32`, saturating. Offsets are checked against `u32::MAX`
/// by `lex`, so saturation never happens on a lexed source.
fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// What a token is. Trivia (whitespace, newlines, comments, line
/// continuations) are tokens too, so the token list covers every byte
/// of the input (DSL-11). Lexing is the same in every phase; which
/// tokens are allowed where is `model`'s job (DSL-4, P2-1).
#[non_exhaustive]
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenKind {
    /// Spaces, tabs and a lone `\r`.
    Whitespace,
    /// `\n` or `\r\n`, kept as written.
    Newline,
    /// `// ...` or `# ...` up to, not including, the newline. Only when
    /// `//` or `#` is the first non-trivia on its line; elsewhere they
    /// are part of a `Word` (`#rrggbb`, `https://...`).
    LineComment,
    /// `/* ... */`, possibly spanning lines. Opens only as the first
    /// non-trivia on its line, like `LineComment`.
    BlockComment,
    /// `\` immediately before a newline: the statement goes on on the
    /// next line (Structurizr syntax; phase 2 per the SRS 4.1 parity table).
    /// The backslash only; the newline is its own token.
    LineContinuation,
    /// A bare run of characters other than whitespace, `"`, `{` and
    /// `}`: keyword, identifier, dotted path, number, `*`, hex colour,
    /// expression (`element.tag==x`), arrow variant (`--x->`, `-/>`).
    Word,
    /// A double-quoted string on one line, quotes and escapes included
    /// as written.
    String,
    /// A `"""` text block, possibly spanning lines, delimiters included
    /// (phase 2 per the SRS 4.1 parity table).
    TextBlock,
    /// `!name`. Directives of later phases (e.g. `!include`) lex fine;
    /// rejecting them is `model`'s job (DSL-4).
    Directive,
    /// The body of a `!script` block, verbatim and not lexed: from after
    /// the newline that follows `{` up to the line holding only `}`
    /// (P2-1; scripts are parsed with no effect, parity table).
    RawText,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `->` as a whole word.
    Arrow,
    /// `=` as a whole word.
    Equals,
}

impl TokenKind {
    /// True for whitespace, newlines, comments and line continuations.
    /// Serves DSL-11.
    pub const fn is_trivia(self) -> bool {
        matches!(
            self,
            TokenKind::Whitespace
                | TokenKind::Newline
                | TokenKind::LineComment
                | TokenKind::BlockComment
                | TokenKind::LineContinuation
        )
    }
}

/// One token: its kind and the bytes it covers in its tree's file.
/// Its text is `&source[token.range()]`; tokens in order tile the whole
/// input (DSL-11). The file is the tree's (`SyntaxTree::file`), so a
/// token stays small; `SyntaxTree::span` gives the full `Span`
/// (DSL-16).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token {
    /// First byte, inclusive.
    pub start: u32,
    /// One past the last byte.
    pub end: u32,
    /// What the token is.
    pub kind: TokenKind,
}

impl Token {
    /// The byte range as `usize`, for slicing the source. Serves DSL-11.
    pub const fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// Splits `source`, the text of `file`, into tokens, trivia included.
/// Concatenating the token texts gives `source` back (DSL-11).
///
/// # Errors
///
/// An unterminated string, text block or block comment, with the span
/// and position of its opening byte; an escape other than `\"` or `\\`;
/// a source over `u32::MAX` bytes (DSL-4).
pub fn lex(file: FileId, source: &str) -> Result<Vec<Token>, ParseError> {
    if u32::try_from(source.len()).is_err() {
        return Err(ParseError {
            kind: ParseErrorKind::FileTooLarge,
            span: Span::new(file, 0, 0),
            position: Position { line: 1, column: 1 },
        });
    }
    Lexer {
        file,
        src: source,
        pos: 0,
        line_start: true,
        script_pending: false,
        tokens: Vec::new(),
    }
    .run()
}

/// State of one `lex` call.
struct Lexer<'a> {
    file: FileId,
    src: &'a str,
    /// Byte offset of the next unread byte.
    pos: usize,
    /// No non-trivia token yet on the current line: a comment may open.
    line_start: bool,
    /// A `!script` directive was seen on this logical line; its `{`
    /// opens a raw body.
    script_pending: bool,
    tokens: Vec<Token>,
}

/// Whitespace other than a newline.
// DECISION: any Unicode white space but `\n` is `Whitespace` (a lone `\r`,
// a no-break space, ...), so an invisible character never glues two words.
fn is_space(c: char) -> bool {
    c != '\n' && c.is_whitespace()
}

/// Length of the newline at the start of `s`: 1 for `\n`, 2 for `\r\n`,
/// 0 when `s` does not start with one.
fn newline_len(s: &str) -> usize {
    if s.starts_with('\n') {
        1
    } else if s.starts_with("\r\n") {
        2
    } else {
        0
    }
}

impl Lexer<'_> {
    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    fn push(&mut self, kind: TokenKind, end: usize) {
        self.tokens.push(Token {
            start: to_u32(self.pos),
            end: to_u32(end),
            kind,
        });
        self.pos = end;
        if !kind.is_trivia() {
            self.line_start = false;
        }
    }

    fn error(&self, kind: ParseErrorKind, start: usize, end: usize) -> ParseError {
        ParseError {
            kind,
            span: Span::new(self.file, to_u32(start), to_u32(end)),
            position: position(self.src, to_u32(start)),
        }
    }

    fn run(mut self) -> Result<Vec<Token>, ParseError> {
        while let Some(c) = self.rest().chars().next() {
            let rest = self.rest();
            let nl = newline_len(rest);
            if nl > 0 {
                let continued = self
                    .tokens
                    .last()
                    .is_some_and(|t| t.kind == TokenKind::LineContinuation);
                self.push(TokenKind::Newline, self.pos + nl);
                // DECISION: the line after a continuation is the same
                // statement, so a `#` or `//` opening it is not a comment.
                if !continued {
                    self.line_start = true;
                    self.script_pending = false;
                }
            } else if c == '\u{feff}' && self.pos == 0 {
                // DECISION: a UTF-8 byte-order mark opening the file is
                // white space (Structurizr 6.2.4 opens corpus
                // workspace-with-bom.dsl); kept as a token, so printing
                // gives it back (DSL-11). Elsewhere it is part of a word.
                self.push(TokenKind::Whitespace, self.pos + c.len_utf8());
            } else if is_space(c) {
                let len = rest.find(|c: char| !is_space(c));
                let mut end = len.map_or(self.src.len(), |n| self.pos + n);
                // Leave the `\r` of a `\r\n` to the newline.
                if self.src[self.pos..end].ends_with('\r') && self.src[end..].starts_with('\n') {
                    end -= 1;
                }
                self.push(TokenKind::Whitespace, end);
            } else if self.line_start && (rest.starts_with("//") || c == '#') {
                let len = rest.find('\n').unwrap_or(rest.len());
                let len = if rest[..len].ends_with('\r') && len < rest.len() {
                    len - 1
                } else {
                    len
                };
                self.push(TokenKind::LineComment, self.pos + len);
            } else if self.line_start && rest.starts_with("/*") {
                let Some(close) = rest[2..].find("*/") else {
                    return Err(self.error(
                        ParseErrorKind::UnterminatedBlockComment,
                        self.pos,
                        self.pos + 2,
                    ));
                };
                self.push(TokenKind::BlockComment, self.pos + 2 + close + 2);
            } else if c == '\\' && newline_len(&rest[1..]) > 0 {
                self.push(TokenKind::LineContinuation, self.pos + 1);
            } else if let Some(body) = rest.strip_prefix("\"\"\"") {
                let Some(close) = body.find("\"\"\"") else {
                    return Err(self.error(
                        ParseErrorKind::UnterminatedTextBlock,
                        self.pos,
                        self.pos + 3,
                    ));
                };
                self.push(TokenKind::TextBlock, self.pos + 3 + close + 3);
            } else if c == '"' {
                let end = self.string_end()?;
                self.push(TokenKind::String, end);
            } else if c == '{' {
                self.push(TokenKind::LBrace, self.pos + 1);
                if std::mem::take(&mut self.script_pending) {
                    self.script_body()?;
                }
            } else if c == '}' {
                self.push(TokenKind::RBrace, self.pos + 1);
            } else {
                let end = self.word_end();
                let text = &self.src[self.pos..end];
                let kind = match text {
                    "->" => TokenKind::Arrow,
                    "=" => TokenKind::Equals,
                    _ if text.len() > 1 && text.starts_with('!') => TokenKind::Directive,
                    _ => TokenKind::Word,
                };
                if kind == TokenKind::Directive && text.eq_ignore_ascii_case("!script") {
                    self.script_pending = true;
                }
                self.push(kind, end);
            }
        }
        Ok(self.tokens)
    }

    /// End of the word at `pos`: up to whitespace, `"`, `{`, `}` or a
    /// line continuation. A `${NAME}` reference (P2-150) is part of the
    /// word, braces included.
    fn word_end(&self) -> usize {
        let rest = self.rest();
        let mut chars = rest.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '$' && rest[i + 1..].starts_with('{') {
                let name = &rest[i + 2..];
                let len = name
                    .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
                    .unwrap_or(name.len());
                if len > 0 && name[len..].starts_with('}') {
                    // Skip `{NAME}`; the loop resumes after the `}`.
                    let end = i + 2 + len + 1;
                    while chars.peek().is_some_and(|&(j, _)| j < end) {
                        chars.next();
                    }
                    continue;
                }
            }
            if c.is_whitespace() || matches!(c, '"' | '{' | '}') {
                return self.pos + i;
            }
            if c == '\\' && newline_len(&rest[i + 1..]) > 0 {
                return self.pos + i;
            }
        }
        self.src.len()
    }

    /// End of the string opening at `pos`, checking escapes.
    fn string_end(&self) -> Result<usize, ParseError> {
        let unterminated =
            || self.error(ParseErrorKind::UnterminatedString, self.pos, self.pos + 1);
        let mut chars = self.rest().char_indices().skip(1);
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => return Ok(self.pos + i + 1),
                '\n' => return Err(unterminated()),
                '\\' => match chars.next() {
                    // P2-147: a `\` ending a line inside a string continues
                    // the string on the next line (Structurizr 6.2.4).
                    Some((_, '"' | '\\' | '\n')) => {}
                    // DECISION: a backslash ending the file is an
                    // unterminated string, not an escape error: the closing
                    // quote is what is missing.
                    None => return Err(unterminated()),
                    Some((_, '\r')) if self.rest()[i + 1..].starts_with("\r\n") => {
                        chars.next();
                    }
                    Some((j, other)) => {
                        let at = self.pos + i;
                        return Err(self.error(
                            ParseErrorKind::InvalidEscape(other),
                            at,
                            self.pos + j + other.len_utf8(),
                        ));
                    }
                },
                _ => {}
            }
        }
        Err(unterminated())
    }

    /// After the `{` of `!script ... {`: when only whitespace follows on
    /// its line, the lines up to the next line holding only `}` are one
    /// `RawText` token.
    // DECISION: a `{` followed by more text on its line (or by end of file)
    // gets no raw body; the rest lexes normally.
    fn script_body(&mut self) -> Result<(), ParseError> {
        let open = self.pos - 1;
        let rest = self.rest();
        let spaces = rest.find(|c: char| !is_space(c)).unwrap_or(rest.len());
        let mut spaces_end = spaces;
        if rest[..spaces].ends_with('\r') && rest[spaces..].starts_with('\n') {
            spaces_end -= 1;
        }
        let nl = newline_len(&rest[spaces_end..]);
        if nl == 0 {
            return Ok(());
        }
        if spaces_end > 0 {
            self.push(TokenKind::Whitespace, self.pos + spaces_end);
        }
        self.push(TokenKind::Newline, self.pos + nl);
        self.line_start = true;
        let body_start = self.pos;
        let mut line = body_start;
        while line < self.src.len() {
            let text = &self.src[line..];
            let len = text.find('\n').map_or(text.len(), |n| n + 1);
            if text[..len].trim() == "}" {
                if line > body_start {
                    self.push(TokenKind::RawText, line);
                }
                return Ok(());
            }
            line += len;
        }
        Err(self.error(ParseErrorKind::UnclosedBrace, open, open + 1))
    }
}

/// The value of a `String` token's text: quotes removed, `\"` and `\\`
/// resolved. The text has already been checked by `lex`. Serves DSL-1,
/// DSL-2 (names, descriptions read by `model`).
pub fn unquote(raw: &str) -> String {
    let inner = raw.strip_prefix('"').unwrap_or(raw);
    let inner = inner.strip_suffix('"').unwrap_or(inner);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // DECISION: on text `lex` did not check, an unknown escape is
            // kept as written rather than panicking.
            match chars.next() {
                Some(e @ ('"' | '\\')) => out.push(e),
                Some(e @ ('\n' | '\r')) => {
                    if e == '\r' && chars.clone().next() == Some('\n') {
                        chars.next();
                    }
                    continue_line(&mut chars);
                }
                Some(e) => {
                    out.push('\\');
                    out.push(e);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// After a `\` and newline inside a string (P2-147): skips the leading
/// white space of the next line unless that line is itself continued.
// DECISION: Structurizr 6.2.4 probed: `"A\⏎      B  \⏎  C"` is
// `A      B  C`, so only the line that ends the string loses its indent.
fn continue_line(chars: &mut std::str::Chars<'_>) {
    let rest = chars.as_str();
    let line = rest.find('\n').map_or(rest, |n| &rest[..n]);
    if line.trim_end_matches('\r').ends_with('\\') {
        return;
    }
    let indent = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    *chars = rest[indent..].chars();
}

/// A string literal whose `unquote` is `value`: quotes added, `"` and
/// `\` escaped. `value` holds no newline: `edit` refuses those first
/// (DSL-10).
pub fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// One statement: the tokens of one line up to the newline, or up to a
/// `{` that opens its block. Line continuations join lines into one
/// head. Example: `app = container "Server" "..." "Rust"` or
/// `container system "c2" "Title" { ... }`. Serves DSL-11 and gives
/// `model` spans to keep (DSL-9, DSL-10, DSL-13).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Statement {
    /// From the first head token to the end of the head, or to the
    /// closing `}` when there is a block. Leading and trailing trivia
    /// excluded. Starts at `{` when the head is empty.
    pub span: Span,
    /// Indices into `SyntaxTree::tokens` of the head: first to last
    /// non-trivia token, trivia between them included, `{` excluded.
    /// Empty for a block with no head, as in a dynamic view's parallel
    /// sequence (P2-1, P3-3).
    // DECISION: token indices are usize: they index a Vec and statements
    // are few, so compactness does not matter here.
    pub head: Range<usize>,
    /// The `{ ... }` block that ends the statement, if any.
    pub block: Option<Block>,
}

/// A `{ ... }` block and the statements inside it. Serves DSL-11.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Block {
    /// Index into `SyntaxTree::tokens` of the `{`.
    pub open: usize,
    /// Index into `SyntaxTree::tokens` of the matching `}`.
    pub close: usize,
    /// Statements inside, in source order. Empty when the body is one
    /// `RawText` token (`!script`).
    pub statements: Vec<Statement>,
}

/// One parsed file: its id, source, tokens and a tree of statements over
/// them. Printing gives the source back byte for byte (DSL-11).
// DECISION: fields private so the invariant "tokens tile the source
// exactly" cannot be broken from outside; read through the accessors.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SyntaxTree {
    file: FileId,
    source: String,
    tokens: Vec<Token>,
    statements: Vec<Statement>,
}

impl SyntaxTree {
    /// The file this tree was parsed from (DSL-16).
    pub const fn file(&self) -> FileId {
        self.file
    }

    /// The source text, unchanged. Serves DSL-11.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Every token in order, trivia included. Serves DSL-11.
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    /// Top-level statements in source order. Empty for an empty file or
    /// a file of only comments. Serves DSL-11.
    pub fn statements(&self) -> &[Statement] {
        &self.statements
    }

    /// The text of token `index`. Serves DSL-1, DSL-2 (model reads
    /// words and strings through it).
    pub fn token_text(&self, index: usize) -> &str {
        &self.source[self.tokens[index].range()]
    }

    /// The span of token `index`, with this tree's file (DSL-16), for
    /// `model` to keep (DSL-9, DSL-10, DSL-13).
    pub fn span(&self, index: usize) -> Span {
        let t = self.tokens[index];
        Span::new(self.file, t.start, t.end)
    }

    /// The concatenated token texts: byte-identical to the input
    /// (DSL-11).
    pub fn print(&self) -> String {
        self.tokens
            .iter()
            .map(|t| &self.source[t.range()])
            .collect()
    }
}

/// Lexes `source`, the text of `file`, and builds the tree. Phase 1
/// passes `FileId::ROOT` (DSL-16).
///
/// # Errors
///
/// The first lexical error (see `lex`), or an unbalanced brace, with the
/// span and position of the opening token, not of the end of file
/// (DSL-4).
// DECISION: first error only; the file is small and the user fixes one
// thing at a time.
pub fn parse(file: FileId, source: &str) -> Result<SyntaxTree, ParseError> {
    let tokens = lex(file, source)?;
    let statements = build(file, source, &tokens)?;
    Ok(SyntaxTree {
        file,
        source: source.to_owned(),
        tokens,
        statements,
    })
}

/// A block being filled while its `}` has not been reached yet.
struct OpenBlock {
    /// Index of the `{`.
    open: usize,
    /// Head of the statement the block belongs to.
    head: Range<usize>,
    /// Start byte of that statement.
    start: u32,
    statements: Vec<Statement>,
}

/// Groups `tokens` into statements and blocks.
// DECISION: an explicit stack of open blocks instead of recursion, so a
// deeply nested file cannot overflow the call stack.
fn build(file: FileId, source: &str, tokens: &[Token]) -> Result<Vec<Statement>, ParseError> {
    let error = |kind, t: Token| ParseError {
        kind,
        span: Span::new(file, t.start, t.end),
        position: position(source, t.start),
    };
    let mut top = Vec::new();
    let mut stack: Vec<OpenBlock> = Vec::new();
    let mut i = 0;
    loop {
        // Skip what lies between statements. A `RawText` body belongs to
        // no statement (`Block::statements` is empty for `!script`).
        while i < tokens.len()
            && (tokens[i].kind.is_trivia() || tokens[i].kind == TokenKind::RawText)
        {
            i += 1;
        }
        let Some(&tok) = tokens.get(i) else {
            // DECISION: with several unclosed blocks, the innermost is
            // reported: it is the one the end of file was reached in.
            return match stack.last() {
                Some(b) => Err(error(ParseErrorKind::UnclosedBrace, tokens[b.open])),
                None => Ok(top),
            };
        };
        if tok.kind == TokenKind::RBrace {
            let Some(b) = stack.pop() else {
                return Err(error(ParseErrorKind::UnmatchedCloseBrace, tok));
            };
            let statement = Statement {
                span: Span::new(file, b.start, tok.end),
                head: b.head,
                block: Some(Block {
                    open: b.open,
                    close: i,
                    statements: b.statements,
                }),
            };
            stack
                .last_mut()
                .map_or(&mut top, |p| &mut p.statements)
                .push(statement);
            i += 1;
            continue;
        }
        // A head: up to a newline not after a line continuation, or a brace.
        let first = i;
        let mut last = None;
        while let Some(t) = tokens.get(i) {
            match t.kind {
                TokenKind::Newline
                    if i > 0 && tokens[i - 1].kind == TokenKind::LineContinuation => {}
                TokenKind::Newline | TokenKind::LBrace | TokenKind::RBrace | TokenKind::RawText => {
                    break;
                }
                k if k.is_trivia() => {}
                _ => last = Some(i),
            }
            i += 1;
        }
        // DECISION: a block with no head records the empty range `{..{`.
        let head = last.map_or(first..first, |l| first..l + 1);
        if tokens.get(i).is_some_and(|t| t.kind == TokenKind::LBrace) {
            stack.push(OpenBlock {
                open: i,
                head,
                start: tokens[first].start,
                statements: Vec::new(),
            });
            i += 1;
        } else if let Some(l) = last {
            let statement = Statement {
                span: Span::new(file, tokens[first].start, tokens[l].end),
                head,
                block: None,
            };
            stack
                .last_mut()
                .map_or(&mut top, |p| &mut p.statements)
                .push(statement);
        }
    }
}

/// Why the text could not be lexed or parsed (DSL-4).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParseErrorKind {
    /// A `"` with no closing `"` before the end of its line.
    UnterminatedString,
    /// A `"""` with no closing `"""`.
    UnterminatedTextBlock,
    /// A `\` followed by a character other than `"` or `\`.
    InvalidEscape(char),
    /// A `/*` with no closing `*/`.
    UnterminatedBlockComment,
    /// A `{` with no matching `}`.
    UnclosedBrace,
    /// A `}` with no `{` to close.
    UnmatchedCloseBrace,
    /// The source is longer than `u32::MAX` bytes.
    FileTooLarge,
}

/// A lex or parse error: what, where (span of the offending or opening
/// token, with its file) and its line and column (DSL-4, DSL-16).
/// `Display` gives one sentence saying what to change (SRS 5.1).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParseError {
    /// What went wrong.
    pub kind: ParseErrorKind,
    /// The offending token, or the opening token for an unterminated
    /// construct.
    pub span: Span,
    /// Line and column of `span.start`.
    pub position: Position,
}

impl fmt::Display for ParseError {
    /// `line:column: <what> — <what to change>` (DSL-4, SRS 5.1). The
    /// caller prefixes the path of `span.file`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Position { line, column } = self.position;
        write!(f, "{line}:{column}: ")?;
        match self.kind {
            ParseErrorKind::UnterminatedString => {
                f.write_str("string is not closed — add a `\"` before the end of the line")
            }
            ParseErrorKind::UnterminatedTextBlock => {
                f.write_str("text block is not closed — add a closing `\"\"\"`")
            }
            ParseErrorKind::InvalidEscape(c) => write!(
                f,
                "invalid escape `\\{c}` in string — only `\\\"` and `\\\\` are allowed; write `\\\\` for a backslash"
            ),
            ParseErrorKind::UnterminatedBlockComment => {
                f.write_str("block comment is not closed — add a closing `*/`")
            }
            ParseErrorKind::UnclosedBrace => {
                f.write_str("`{` is never closed — add a matching `}`")
            }
            ParseErrorKind::UnmatchedCloseBrace => {
                f.write_str("`}` closes nothing — remove it or add the `{` it closes")
            }
            ParseErrorKind::FileTooLarge => {
                f.write_str("file is larger than 4 GiB — split it into smaller files")
            }
        }
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Non-trivia tokens of `source` as (kind, text) pairs.
    fn significant(source: &str) -> Vec<(TokenKind, &str)> {
        lex(FileId::ROOT, source)
            .expect("lexes")
            .into_iter()
            .filter(|t| !t.kind.is_trivia())
            .map(|t| (t.kind, &source[t.range()]))
            .collect()
    }

    /// Every token as (kind, text), trivia included.
    fn all_tokens(source: &str) -> Vec<(TokenKind, &str)> {
        lex(FileId::ROOT, source)
            .expect("lexes")
            .into_iter()
            .map(|t| (t.kind, &source[t.range()]))
            .collect()
    }

    fn concat(source: &str, tokens: &[Token]) -> String {
        tokens.iter().map(|t| &source[t.range()]).collect()
    }

    /// Tokens tile the input: contiguous, no gap, no overlap, no empty token.
    fn assert_tiles(source: &str, tokens: &[Token]) {
        let mut at = 0u32;
        for t in tokens {
            assert_eq!(t.start, at, "gap or overlap before {t:?}");
            assert!(t.end > t.start, "empty token {t:?}");
            at = t.end;
        }
        assert_eq!(at as usize, source.len());
        assert_eq!(concat(source, tokens), source);
    }

    /// Every `.dsl` file under `tests/data/`, plus `docs/workspace.dsl`,
    /// sorted by path.
    fn fixtures() -> Vec<(std::path::PathBuf, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                // DECISION: the pinned Structurizr corpus holds files that
                // Structurizr itself rejects; it has its own tests
                // (crates/core/tests/compat.rs, P2-14), so it is no
                // fixture here.
                if path.is_dir() && path.ends_with("tests/data/structurizr") {
                    continue;
                }
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "dsl") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut paths = Vec::new();
        walk(&root.join("tests/data"), &mut paths);
        paths.push(root.join("docs/workspace.dsl"));
        paths.sort();
        let files: Vec<_> = paths
            .into_iter()
            .filter_map(|p| std::fs::read_to_string(&p).ok().map(|s| (p, s)))
            .collect();
        assert!(!files.is_empty(), "no .dsl fixture found");
        files
    }

    /// LF, CRLF, with and without a final newline, of one text.
    fn line_ending_variants(source: &str) -> Vec<String> {
        let lf = source.replace("\r\n", "\n");
        let bare = lf.trim_end_matches('\n').to_string();
        let crlf = lf.replace('\n', "\r\n");
        let crlf_bare = bare.replace('\n', "\r\n");
        vec![lf, bare, crlf, crlf_bare]
    }

    fn head_words(tree: &SyntaxTree, s: &Statement) -> Vec<String> {
        s.head
            .clone()
            .filter(|&i| !tree.tokens()[i].kind.is_trivia())
            .map(|i| tree.token_text(i).to_string())
            .collect()
    }

    fn parse_err(source: &str) -> ParseError {
        parse(FileId::ROOT, source).expect_err("must fail")
    }

    // Layout pins for the per-token structs.

    #[test]
    fn file_id_size_is_pinned() {
        assert_eq!(std::mem::size_of::<FileId>(), 4);
    }

    #[test]
    fn span_size_is_pinned() {
        assert_eq!(std::mem::size_of::<Span>(), 12);
    }

    #[test]
    fn token_size_is_pinned() {
        assert_eq!(std::mem::size_of::<Token>(), 12);
    }

    #[test]
    fn span_helpers() {
        let s = Span::new(FileId(2), 3, 7);
        assert_eq!(
            s,
            Span {
                file: FileId(2),
                start: 3,
                end: 7
            }
        );
        assert_eq!(s.range(), 3..7);
        assert_eq!(s.len(), 4);
        assert!(!s.is_empty());
        assert!(Span::new(FileId::ROOT, 5, 5).is_empty());
        assert_eq!(Span::new(FileId::ROOT, 5, 5).len(), 0);
        assert_eq!(
            Token {
                start: 1,
                end: 4,
                kind: TokenKind::Word
            }
            .range(),
            1..4
        );
    }

    #[test]
    fn position_counts_lines_and_chars() {
        let src = "ab\ncd\r\néf\rg";
        assert_eq!(position(src, 0), Position { line: 1, column: 1 });
        assert_eq!(position(src, 2), Position { line: 1, column: 3 });
        assert_eq!(position(src, 3), Position { line: 2, column: 1 });
        assert_eq!(position(src, 7), Position { line: 3, column: 1 });
        // `é` is two bytes, one column.
        assert_eq!(position(src, 9), Position { line: 3, column: 2 });
        // A lone `\r` does not end a line.
        assert_eq!(position(src, 11), Position { line: 3, column: 4 });
        // Past the end clamps to the end.
        assert_eq!(position(src, 99), Position { line: 3, column: 5 });
    }

    #[test]
    fn trivia_kinds() {
        use TokenKind::*;
        for k in [
            Whitespace,
            Newline,
            LineComment,
            BlockComment,
            LineContinuation,
        ] {
            assert!(k.is_trivia(), "{k:?}");
        }
        for k in [
            Word, String, TextBlock, Directive, RawText, LBrace, RBrace, Arrow, Equals,
        ] {
            assert!(!k.is_trivia(), "{k:?}");
        }
    }

    // One test per requirement.

    #[test]
    fn dsl_4_error_names_line_and_column() {
        let src = "workspace {\n    model {\n        x = person \"open\n    }\n}\n";
        let err = parse_err(src);
        assert_eq!(err.kind, ParseErrorKind::UnterminatedString);
        assert_eq!(
            err.position,
            Position {
                line: 3,
                column: 20
            }
        );
        assert_eq!(err.position, position(src, err.span.start));
        let text = err.to_string();
        assert!(text.starts_with("3:20: "), "{text}");
        // One sentence saying what is wrong and what to change (SRS 5.1).
        assert!(text.contains("string"), "{text}");
        assert!(text.contains('—'), "{text}");

        let err = parse_err("a {\n  b\n");
        assert!(err.to_string().starts_with("1:3: "), "{err}");
    }

    #[test]
    fn dsl_11_round_trip_byte_identical() {
        for (path, text) in fixtures() {
            for source in line_ending_variants(&text) {
                let tree = parse(FileId::ROOT, &source)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert_eq!(tree.print(), source, "{}", path.display());
                assert_eq!(tree.source(), source);
                assert!(!tree.statements().is_empty(), "{}", path.display());
            }
        }
    }

    #[test]
    fn dsl_16_spans_carry_file_id() {
        let file = FileId(3);
        let tree = parse(file, "a = b {\n  c\n}\n").unwrap();
        assert_eq!(tree.file(), file);
        for i in 0..tree.tokens().len() {
            assert_eq!(tree.span(i).file, file);
        }
        let top = &tree.statements()[0];
        assert_eq!(top.span.file, file);
        assert_eq!(top.block.as_ref().unwrap().statements[0].span.file, file);
        let err = parse(file, "a {\n").unwrap_err();
        assert_eq!(err.span.file, file);
        let err = lex(file, "\"x").unwrap_err();
        assert_eq!(err.span.file, file);
        // Phase 1 passes the root file.
        let tree = parse(FileId::ROOT, "x\n").unwrap();
        assert_eq!(tree.statements()[0].span.file, FileId::ROOT);
        assert_eq!(FileId::ROOT, FileId(0));
    }

    // text-01: lexer

    #[test]
    fn lexer_keeps_every_byte() {
        // DSL-11 at token level.
        let src = "  // c\n#c\n/* a\nb */ workspace \"n \\\" \\\\\" {\r\n\tx = y -> z \\\n  !include f\n\"\"\"\nt\n\"\"\"\r}";
        let tokens = lex(FileId::ROOT, src).unwrap();
        assert_tiles(src, &tokens);
        for (path, text) in fixtures() {
            for source in line_ending_variants(&text) {
                let tokens = lex(FileId::ROOT, &source)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert_tiles(&source, &tokens);
            }
        }
    }

    #[test]
    fn lexer_string_escapes() {
        use TokenKind::*;
        let src = r#"a "plain" "say \"hi\"" "back\\slash" "" b"#;
        assert_eq!(
            significant(src),
            vec![
                (Word, "a"),
                (String, r#""plain""#),
                (String, r#""say \"hi\"""#),
                (String, r#""back\\slash""#),
                (String, r#""""#),
                (Word, "b"),
            ]
        );
        assert_eq!(unquote(r#""say \"hi\"""#), r#"say "hi""#);
        assert_eq!(unquote(r#""back\\slash""#), r"back\slash");
        assert_eq!(unquote(r#""""#), "");
        // An escaped backslash before the closing quote ends the string.
        assert_eq!(
            significant(r#""x\\" y"#),
            vec![(String, r#""x\\""#), (Word, "y")]
        );
        // Strings stick to neighbouring words.
        assert_eq!(
            significant(r#"a"b"c"#),
            vec![(Word, "a"), (String, r#""b""#), (Word, "c")]
        );
    }

    #[test]
    fn lexer_comments_all_three_styles() {
        use TokenKind::*;
        let src = "// one\n  # two\n/* three\nlines */\nx // not a comment\n";
        assert_eq!(
            all_tokens(src),
            vec![
                (LineComment, "// one"),
                (Newline, "\n"),
                (Whitespace, "  "),
                (LineComment, "# two"),
                (Newline, "\n"),
                (BlockComment, "/* three\nlines */"),
                (Newline, "\n"),
                (Word, "x"),
                (Whitespace, " "),
                (Word, "//"),
                (Whitespace, " "),
                (Word, "not"),
                (Whitespace, " "),
                (Word, "a"),
                (Whitespace, " "),
                (Word, "comment"),
                (Newline, "\n"),
            ]
        );
        // The line comment stops before `\r\n`.
        assert_eq!(
            all_tokens("# c\r\n"),
            vec![(LineComment, "# c"), (Newline, "\r\n")]
        );
        // After a block comment the line still has no non-trivia.
        assert_eq!(significant("/* a */ // b\nx"), vec![(Word, "x")]);
    }

    #[test]
    fn lexer_error_has_line_and_column() {
        // DSL-4: parse errors name line and column.
        let src = "a\n  b \"x\n";
        let err = lex(FileId::ROOT, src).unwrap_err();
        assert_eq!(err.kind, ParseErrorKind::UnterminatedString);
        assert_eq!(err.position, Position { line: 2, column: 5 });
        assert_eq!(err.span.start, 6);
        assert!(err.to_string().starts_with("2:5: "));
    }

    // text-02: tree

    #[test]
    fn tree_nests_blocks() {
        let src = "workspace \"W\" {\n    model {\n        user = person \"Operator\"\n        sys = softwareSystem \"S\" {\n            app = container \"A\"\n        }\n    }\n    views {\n    }\n}\n";
        let tree = parse(FileId::ROOT, src).unwrap();
        assert_eq!(tree.statements().len(), 1);
        let ws = &tree.statements()[0];
        assert_eq!(head_words(&tree, ws), ["workspace", "\"W\""]);
        assert_eq!(&src[ws.span.range()], src.trim_end());
        let block = ws.block.as_ref().unwrap();
        assert_eq!(tree.token_text(block.open), "{");
        assert_eq!(tree.token_text(block.close), "}");
        assert_eq!(block.close, tree.tokens().len() - 2);
        let names: Vec<_> = block
            .statements
            .iter()
            .map(|s| tree.token_text(s.head.start))
            .collect();
        assert_eq!(names, ["model", "views"]);
        let model = &block.statements[0];
        let inner = &model.block.as_ref().unwrap().statements;
        assert_eq!(inner.len(), 2);
        assert_eq!(
            head_words(&tree, &inner[0]),
            ["user", "=", "person", "\"Operator\""]
        );
        assert!(inner[0].block.is_none());
        assert_eq!(&src[inner[0].span.range()], "user = person \"Operator\"");
        let sys = &inner[1];
        assert_eq!(
            head_words(&tree, sys),
            ["sys", "=", "softwareSystem", "\"S\""]
        );
        let app = &sys.block.as_ref().unwrap().statements[0];
        assert_eq!(&src[app.span.range()], "app = container \"A\"");
        assert!(sys.span.range().contains(&(app.span.start as usize)));
        let views = &block.statements[1];
        assert!(views.block.as_ref().unwrap().statements.is_empty());
        // `{` is excluded from the head; trailing trivia too.
        let last_head = tree.tokens()[ws.head.end - 1];
        assert_eq!(last_head.kind, TokenKind::String);
    }

    #[test]
    fn tree_statements_share_a_line_with_braces() {
        let tree = parse(FileId::ROOT, "a { b } c\nd { e\n} f").unwrap();
        let tops: Vec<_> = tree
            .statements()
            .iter()
            .map(|s| tree.token_text(s.head.start))
            .collect();
        assert_eq!(tops, ["a", "c", "d", "f"]);
        let a = &tree.statements()[0];
        assert_eq!(
            tree.token_text(a.block.as_ref().unwrap().statements[0].head.start),
            "b"
        );
    }

    #[test]
    fn unbalanced_brace_error_points_at_opener() {
        // DSL-4
        let src = "workspace {\n  model {\n  }\n";
        let err = parse_err(src);
        assert_eq!(err.kind, ParseErrorKind::UnclosedBrace);
        assert_eq!(&src[err.span.range()], "{");
        assert_eq!(err.span.start, 10);
        assert_eq!(
            err.position,
            Position {
                line: 1,
                column: 11
            }
        );

        let src = "a {\n}\n}\n";
        let err = parse_err(src);
        assert_eq!(err.kind, ParseErrorKind::UnmatchedCloseBrace);
        assert_eq!(err.span.start, 6);
        assert_eq!(err.position, Position { line: 3, column: 1 });
    }

    // Review Focus

    #[test]
    fn crlf_and_missing_final_newline_round_trip() {
        // DSL-11
        for src in
            line_ending_variants("// c\nworkspace {\n\n  model {\n    a = person \"R\"\n  }\n}\n")
        {
            let tree = parse(FileId::ROOT, &src).unwrap();
            assert_eq!(tree.print(), src);
            assert_eq!(tree.statements().len(), 1);
        }
        let crlf = "a\r\nb\r\n";
        let tree = parse(FileId::ROOT, crlf).unwrap();
        assert_eq!(tree.statements().len(), 2);
        assert_eq!(&crlf[tree.statements()[0].span.range()], "a");
        assert!(
            tree.tokens()
                .iter()
                .any(|t| t.kind == TokenKind::Newline && t.end - t.start == 2)
        );
    }

    #[test]
    fn crlf_error_position_counts_lines_once() {
        // DSL-4: in a CRLF file, an error on line 3 reports line 3.
        let src = "a {\r\n  b\r\n  c \"x\r\n}\r\n";
        let err = parse_err(src);
        assert_eq!(err.kind, ParseErrorKind::UnterminatedString);
        assert_eq!(err.position, Position { line: 3, column: 5 });
    }

    #[test]
    fn unterminated_string_error_points_at_opener() {
        // DSL-4
        let src = "a \"never closed\nb\nc\n";
        let err = parse_err(src);
        assert_eq!(err.kind, ParseErrorKind::UnterminatedString);
        assert_eq!(err.span.start, 2);
        assert_eq!(err.position, Position { line: 1, column: 3 });
        // At end of file too.
        let err = parse_err("x \"tail");
        assert_eq!(err.position, Position { line: 1, column: 3 });
    }

    #[test]
    fn unterminated_block_comment_error_points_at_opener() {
        // DSL-4
        let src = "a\n  /* open\nb\nc\n";
        let err = parse_err(src);
        assert_eq!(err.kind, ParseErrorKind::UnterminatedBlockComment);
        assert_eq!(err.span.start, 4);
        assert_eq!(err.position, Position { line: 2, column: 3 });
    }

    #[test]
    fn non_ascii_spans_are_bytes_columns_are_chars() {
        // DSL-4, DSL-11
        let src = "é = person \"Café 東京\" \"x\nb";
        let tokens = lex(FileId::ROOT, "é = person \"Café 東京\"").unwrap();
        assert_eq!(
            tokens[0],
            Token {
                start: 0,
                end: 2,
                kind: TokenKind::Word
            }
        );
        let s = tokens.last().unwrap();
        assert_eq!(s.kind, TokenKind::String);
        assert_eq!((s.start, s.end), (12, 12 + 14));
        let err = parse_err(src);
        assert_eq!(err.kind, ParseErrorKind::UnterminatedString);
        assert_eq!(err.span.start, 27);
        assert_eq!(
            err.position,
            Position {
                line: 1,
                column: 22
            }
        );
        let ok = "é = person \"Café 東京\"\n";
        assert_eq!(parse(FileId::ROOT, ok).unwrap().print(), ok);
    }

    #[test]
    fn empty_file_parses_to_empty_tree() {
        let tree = parse(FileId::ROOT, "").unwrap();
        assert!(tree.statements().is_empty());
        assert!(tree.tokens().is_empty());
        assert_eq!(tree.print(), "");
        let tree = parse(FileId::ROOT, "\n  \r\n\t").unwrap();
        assert!(tree.statements().is_empty());
    }

    #[test]
    fn comments_only_file_parses_to_empty_tree() {
        let src = "// a\n# b\n/* c\n { */\n";
        let tree = parse(FileId::ROOT, src).unwrap();
        assert!(tree.statements().is_empty());
        assert_eq!(tree.print(), src);
    }

    #[test]
    fn later_phase_directive_lexes() {
        // DSL-4: `!include` lexes as a Directive; model rejects it.
        use TokenKind::*;
        assert_eq!(
            significant("!include other.dsl\n!identifiers hierarchical"),
            vec![
                (Directive, "!include"),
                (Word, "other.dsl"),
                (Directive, "!identifiers"),
                (Word, "hierarchical"),
            ]
        );
        let tree = parse(FileId::ROOT, "!include other.dsl\n").unwrap();
        assert_eq!(tree.statements().len(), 1);
    }

    #[test]
    fn quote_unquote_round_trip() {
        // DSL-10: edit rewrites literals with `quote`.
        for value in [
            "",
            "plain",
            "say \"hi\"",
            r"back\slash",
            r#"\""#,
            "Café 東京",
            "# not a comment",
        ] {
            let q = quote(value);
            assert!(q.starts_with('"') && q.ends_with('"'));
            let tokens = lex(FileId::ROOT, &q).unwrap();
            assert_eq!(tokens.len(), 1, "{q}");
            assert_eq!(tokens[0].kind, TokenKind::String);
            assert_eq!(unquote(&q), value);
        }
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    // Comment rule and escapes (SRS 4.1 keyword table)

    #[test]
    fn hash_colour_mid_line_is_word() {
        // `background #0b3c6f` lexes `#0b3c6f` as a Word.
        use TokenKind::*;
        assert_eq!(
            significant("    background #0b3c6f\n"),
            vec![(Word, "background"), (Word, "#0b3c6f")]
        );
    }

    #[test]
    fn slashes_mid_line_are_word() {
        // `url https://example.com` lexes the URL as one Word.
        use TokenKind::*;
        assert_eq!(
            significant("url https://example.com /* x */"),
            vec![
                (Word, "url"),
                (Word, "https://example.com"),
                (Word, "/*"),
                (Word, "x"),
                (Word, "*/")
            ]
        );
    }

    #[test]
    fn invalid_escape_is_error() {
        // DSL-4: `"a\nb"` fails with InvalidEscape('n') at the backslash.
        let src = "x \"a\\nb\"";
        let err = lex(FileId::ROOT, src).unwrap_err();
        assert_eq!(err.kind, ParseErrorKind::InvalidEscape('n'));
        assert_eq!(err.span.start, 4);
        assert_eq!(err.position, Position { line: 1, column: 5 });
        assert!(err.to_string().contains("\\n"), "{err}");
        let err = parse_err("\"é\\ü\"");
        assert_eq!(err.kind, ParseErrorKind::InvalidEscape('ü'));
        assert_eq!(err.position, Position { line: 1, column: 3 });
    }

    #[test]
    fn arrow_and_equals_are_whole_words() {
        use TokenKind::*;
        assert_eq!(
            significant("a = b -> c --x-> d -/> e == f a->b"),
            vec![
                (Word, "a"),
                (Equals, "="),
                (Word, "b"),
                (Arrow, "->"),
                (Word, "c"),
                (Word, "--x->"),
                (Word, "d"),
                (Word, "-/>"),
                (Word, "e"),
                (Word, "=="),
                (Word, "f"),
                (Word, "a->b"),
            ]
        );
        assert_eq!(
            significant("a{b}"),
            vec![(Word, "a"), (LBrace, "{"), (Word, "b"), (RBrace, "}")]
        );
    }

    // Full Structurizr grammar shape (P2-1): lexes and parses now,
    // rejected by `model` in phase 1.

    #[test]
    fn headless_block_parses() {
        // A dynamic view's parallel sequence: `{` alone on its line.
        let src = "dynamic * {\n    {\n        a -> b\n    }\n}\n";
        let tree = parse(FileId::ROOT, src).unwrap();
        let outer = tree.statements()[0].block.as_ref().unwrap();
        assert_eq!(outer.statements.len(), 1);
        let par = &outer.statements[0];
        assert!(par.head.is_empty());
        assert_eq!(&src[par.span.range()], "{\n        a -> b\n    }");
        let inner = &par.block.as_ref().unwrap().statements;
        assert_eq!(head_words(&tree, &inner[0]), ["a", "->", "b"]);
    }

    #[test]
    fn line_continuation_joins_head() {
        use TokenKind::*;
        let src = "a = person \\\n    \"Name\" \\\r\n    \"Desc\"\nb\n";
        assert_eq!(
            all_tokens("x \\\ny")[..4],
            [
                (Word, "x"),
                (Whitespace, " "),
                (LineContinuation, "\\"),
                (Newline, "\n")
            ]
        );
        let tree = parse(FileId::ROOT, src).unwrap();
        assert_eq!(tree.statements().len(), 2);
        let s = &tree.statements()[0];
        assert_eq!(
            head_words(&tree, s),
            ["a", "=", "person", "\"Name\"", "\"Desc\""]
        );
        assert_eq!(
            &src[s.span.range()],
            "a = person \\\n    \"Name\" \\\r\n    \"Desc\""
        );
        // A backslash elsewhere stays in its word.
        assert_eq!(significant("a\\b"), vec![(Word, "a\\b")]);
        // A `#` on a continued line is not a comment.
        assert_eq!(significant("a \\\n#b"), vec![(Word, "a"), (Word, "#b")]);
    }

    #[test]
    fn text_block_spans_lines() {
        use TokenKind::*;
        let src = "description \"\"\"\n  First line\n  \"quoted\" second\n  \"\"\" tail\n";
        assert_eq!(
            significant(src),
            vec![
                (Word, "description"),
                (
                    TextBlock,
                    "\"\"\"\n  First line\n  \"quoted\" second\n  \"\"\""
                ),
                (Word, "tail"),
            ]
        );
        let tree = parse(FileId::ROOT, src).unwrap();
        assert_eq!(tree.statements().len(), 1);
        assert_eq!(tree.print(), src);
        let err = parse_err("a\nb \"\"\"\nnever closed\n");
        assert_eq!(err.kind, ParseErrorKind::UnterminatedTextBlock);
        assert_eq!(err.position, Position { line: 2, column: 3 });
    }

    #[test]
    fn script_body_is_raw_text() {
        // `!script groovy {` body with unbalanced quotes and braces.
        use TokenKind::*;
        let body = "    def x = \"open\n    if (x) {\n    // }\n";
        let src = format!("workspace {{\n  !script groovy {{\n{body}  }}\n  model {{\n  }}\n}}\n");
        let tokens = all_tokens(&src);
        assert!(tokens.contains(&(RawText, body)), "{tokens:?}");
        let tree = parse(FileId::ROOT, &src).unwrap();
        assert_eq!(tree.print(), src);
        let ws = tree.statements()[0].block.as_ref().unwrap();
        assert_eq!(ws.statements.len(), 2);
        let script = &ws.statements[0];
        assert_eq!(head_words(&tree, script), ["!script", "groovy"]);
        assert!(script.block.as_ref().unwrap().statements.is_empty());
        assert_eq!(tree.token_text(ws.statements[1].head.start), "model");
        // An unclosed script reports its `{`.
        let err = parse_err("!script groovy {\n  x\n");
        assert_eq!(err.kind, ParseErrorKind::UnclosedBrace);
        assert_eq!(
            err.position,
            Position {
                line: 1,
                column: 16
            }
        );
        // An empty body has no RawText token.
        let src = "!script kotlin {\r\n}\r\n";
        assert!(!all_tokens(src).iter().any(|t| t.0 == RawText));
        assert_eq!(parse(FileId::ROOT, src).unwrap().print(), src);
    }

    #[test]
    fn byte_order_mark_is_trivia() {
        // P2-14 (corpus workspace-with-bom.dsl): a leading U+FEFF is
        // whitespace, and printing keeps it (DSL-11).
        use TokenKind::*;
        let src = "\u{feff}workspace {\n}\n";
        assert_eq!(all_tokens(src)[0], (Whitespace, "\u{feff}"));
        assert_eq!(significant(src)[0], (Word, "workspace"));
        assert_eq!(parse(FileId::ROOT, src).unwrap().print(), src);
    }

    #[test]
    fn line_continuation_inside_a_string() {
        // P2-147 (corpus multi-line.dsl), Structurizr 6.2.4 probed: `\`
        // ending a line inside a string joins the next line; the leading
        // white space of the line that ends the string is dropped, that of
        // a line itself continued is kept.
        use TokenKind::*;
        let src = "a \"Software \\\n      System\" b";
        assert_eq!(
            significant(src),
            vec![(Word, "a"), (String, "\"Software \\\n      System\""), (Word, "b")]
        );
        assert_eq!(unquote("\"Software \\\n      System\""), "Software System");
        assert_eq!(unquote("\"A\\\n      B  \\\r\n  C\""), "A      B  C");
        assert_eq!(unquote("\"X  \\\n\t  Y\""), "X  Y");
        // A backslash ending the file is still an unterminated string.
        assert_eq!(parse_err("a \"x\\").kind, ParseErrorKind::UnterminatedString);
    }
}
