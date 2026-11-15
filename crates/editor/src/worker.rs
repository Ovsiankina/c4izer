//! Pipeline thread: takes open, file-changed, drag and release requests
//! with a generation number, drops stale ones, sends scenes back. The UI
//! thread never routes (EDIT-25).
//!
//! Depends on: `c4rizer-core` (pipeline).
