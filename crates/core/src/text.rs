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
        todo!()
    }

    /// The byte range as `usize`, for slicing that file's source:
    /// `&source[span.range()]`. Serves DSL-9, DSL-10, DSL-11.
    pub const fn range(self) -> Range<usize> {
        todo!()
    }

    /// Length in bytes. Serves DSL-11.
    pub const fn len(self) -> u32 {
        todo!()
    }

    /// True when the span covers no byte. Serves DSL-11.
    pub const fn is_empty(self) -> bool {
        todo!()
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
    todo!()
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
        todo!()
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
        todo!()
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
    todo!()
}

/// The value of a `String` token's text: quotes removed, `\"` and `\\`
/// resolved. The text has already been checked by `lex`. Serves DSL-1,
/// DSL-2 (names, descriptions read by `model`).
pub fn unquote(raw: &str) -> String {
    todo!()
}

/// A string literal whose `unquote` is `value`: quotes added, `"` and
/// `\` escaped. `value` holds no newline: `edit` refuses those first
/// (DSL-10).
pub fn quote(value: &str) -> String {
    todo!()
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
        todo!()
    }

    /// The source text, unchanged. Serves DSL-11.
    pub fn source(&self) -> &str {
        todo!()
    }

    /// Every token in order, trivia included. Serves DSL-11.
    pub fn tokens(&self) -> &[Token] {
        todo!()
    }

    /// Top-level statements in source order. Empty for an empty file or
    /// a file of only comments. Serves DSL-11.
    pub fn statements(&self) -> &[Statement] {
        todo!()
    }

    /// The text of token `index`. Serves DSL-1, DSL-2 (model reads
    /// words and strings through it).
    pub fn token_text(&self, index: usize) -> &str {
        todo!()
    }

    /// The span of token `index`, with this tree's file (DSL-16), for
    /// `model` to keep (DSL-9, DSL-10, DSL-13).
    pub fn span(&self, index: usize) -> Span {
        todo!()
    }

    /// The concatenated token texts: byte-identical to the input
    /// (DSL-11).
    pub fn print(&self) -> String {
        todo!()
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
    todo!()
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
        todo!()
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;

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

    // One test per requirement.

    #[test]
    fn dsl_4_error_names_line_and_column() {
        todo!()
    }

    #[test]
    fn dsl_11_round_trip_byte_identical() {
        todo!()
    }

    #[test]
    fn dsl_16_spans_carry_file_id() {
        // Tree, statement and error spans all carry the file passed to
        // `parse`; phase 1 passes `FileId::ROOT`.
        todo!()
    }

    // text-01: lexer

    #[test]
    fn lexer_keeps_every_byte() {
        // DSL-11 at token level.
        todo!()
    }

    #[test]
    fn lexer_string_escapes() {
        todo!()
    }

    #[test]
    fn lexer_comments_all_three_styles() {
        todo!()
    }

    #[test]
    fn lexer_error_has_line_and_column() {
        // DSL-4: parse errors name line and column.
        todo!()
    }

    // text-02: tree

    #[test]
    fn tree_nests_blocks() {
        todo!()
    }

    #[test]
    fn unbalanced_brace_error_points_at_opener() {
        // DSL-4
        todo!()
    }

    // Review Focus

    #[test]
    fn crlf_and_missing_final_newline_round_trip() {
        // DSL-11
        todo!()
    }

    #[test]
    fn crlf_error_position_counts_lines_once() {
        // DSL-4: in a CRLF file, an error on line 3 reports line 3.
        todo!()
    }

    #[test]
    fn unterminated_string_error_points_at_opener() {
        // DSL-4
        todo!()
    }

    #[test]
    fn unterminated_block_comment_error_points_at_opener() {
        // DSL-4
        todo!()
    }

    #[test]
    fn non_ascii_spans_are_bytes_columns_are_chars() {
        // DSL-4, DSL-11
        todo!()
    }

    #[test]
    fn empty_file_parses_to_empty_tree() {
        todo!()
    }

    #[test]
    fn comments_only_file_parses_to_empty_tree() {
        todo!()
    }

    #[test]
    fn later_phase_directive_lexes() {
        // DSL-4: `!include` lexes as a Directive; model rejects it.
        todo!()
    }

    #[test]
    fn quote_unquote_round_trip() {
        // DSL-10: edit rewrites literals with `quote`.
        todo!()
    }

    // Comment rule and escapes (SRS 4.1 keyword table)

    #[test]
    fn hash_colour_mid_line_is_word() {
        // `background #0b3c6f` lexes `#0b3c6f` as a Word.
        todo!()
    }

    #[test]
    fn slashes_mid_line_are_word() {
        // `url https://example.com` lexes the URL as one Word.
        todo!()
    }

    #[test]
    fn invalid_escape_is_error() {
        // DSL-4: `"a\nb"` fails with InvalidEscape('n') at the backslash.
        todo!()
    }

    // Full Structurizr grammar shape (P2-1): lexes and parses now,
    // rejected by `model` in phase 1.

    #[test]
    fn headless_block_parses() {
        // A dynamic view's parallel sequence: `{` alone on its line.
        todo!()
    }

    #[test]
    fn line_continuation_joins_head() {
        todo!()
    }

    #[test]
    fn text_block_spans_lines() {
        todo!()
    }

    #[test]
    fn script_body_is_raw_text() {
        // `!script groovy {` body with unbalanced quotes and braces.
        todo!()
    }
}
