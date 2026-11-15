//! Lossless lexer and syntax tree for the diagram file. Every byte,
//! comments and blank lines included, is kept with its span, so printing
//! the tree gives the input back (DSL-11). Parse errors carry line and
//! column (DSL-4).
//!
//! Depends on: nothing.
