//! The File menu (owner request 2026-11-23): export of the current view
//! or of the whole workspace as PDF, Save, Save as, and the commands
//! that pass while Explore is open. SVG and PNG export are EDIT-7's and
//! tested there. Also the Settings menu: the open view's crossing
//! budget.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::files::{self, FileEvent, UiState};
use crate::state::{Command, Dialog, Effect, Event, State};
use crate::testutil::{SHOP, TempDir};
use crate::worker::{self, ExportFormat};

/// The shell without a window: every effect at once, dialogs recorded,
/// writes counted.
pub(crate) struct Shell {
    pub(crate) s: State,
    dialogs: Vec<Dialog>,
    writes: usize,
}

impl Shell {
    fn open(path: &Path) -> Shell {
        let mut shell = Shell {
            s: State::new(UiState::default()),
            dialogs: Vec::new(),
            writes: 0,
        };
        shell.cmd(Command::Open(path.to_path_buf()));
        shell
    }

    fn cmd(&mut self, command: Command) {
        let effects = self.s.handle(Event::Command(command));
        self.run(effects);
    }

    pub(crate) fn run(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            let next = match effect {
                Effect::Read(path) => {
                    let (text, hash) = files::read(&path).expect("read fixture");
                    self.s.handle(Event::File(FileEvent::Loaded { path, text, hash }))
                }
                Effect::Write(request) => {
                    self.writes += 1;
                    self.s.handle(Event::File(files::write_checked(&request)))
                }
                Effect::Work(request) => match worker::serve(request, &AtomicBool::new(false)) {
                    Some(reply) => self.s.handle(Event::Worker(reply)),
                    None => Vec::new(),
                },
                Effect::Dialog(dialog) => {
                    self.dialogs.push(dialog);
                    Vec::new()
                }
                Effect::Watch(_) | Effect::SaveUiState(_) | Effect::Fullscreen(_) | Effect::OpenUrl(_) => Vec::new(),
            };
            self.run(next);
        }
    }

    fn notice(&self) -> String {
        self.s.status().notice.unwrap_or_default()
    }
}

pub(crate) fn shop(tag: &str) -> (TempDir, std::path::PathBuf, Shell) {
    let dir = TempDir::new(tag);
    let path = dir.join("shop.dsl");
    std::fs::write(&path, SHOP).expect("written");
    let shell = Shell::open(&path);
    assert!(shell.s.status().problem.is_none(), "{:?}", shell.s.status().problem);
    (dir, path, shell)
}

/// The number of pages of a PDF this crate wrote.
fn pages(bytes: &[u8]) -> usize {
    String::from_utf8_lossy(bytes).matches("/Type /Page ").count()
}

#[test]
fn file_export_current_view_as_pdf_is_one_page() {
    let (dir, _, mut h) = shop("file-pdf-view");
    let key = h.s.view.as_ref().expect("a view").key();
    h.cmd(Command::ExportView(ExportFormat::Pdf));
    let Some(Dialog::ExportView { view, name, format, .. }) = h.dialogs.last().cloned() else {
        panic!("{:?}", h.dialogs.last())
    };
    assert_eq!(name, format!("{key}.pdf"));
    assert_eq!(format, ExportFormat::Pdf);
    let out = dir.join(&name);
    h.cmd(Command::Export { targets: vec![(view, out.clone())], format });
    assert!(h.notice().starts_with("exported"), "{}", h.notice());
    let bytes = std::fs::read(&out).expect("the PDF");
    assert!(bytes.starts_with(b"%PDF-"));
    assert_eq!(pages(&bytes), 1);
}

#[test]
fn file_export_whole_workspace_as_pdf_is_one_document() {
    let (dir, _, mut h) = shop("file-pdf-all");
    h.cmd(Command::ExportAll(ExportFormat::Pdf));
    let Some(Dialog::ExportDocument { name, .. }) = h.dialogs.last().cloned() else {
        panic!("{:?}", h.dialogs.last())
    };
    assert_eq!(name, "shop.pdf");
    let out = dir.join(&name);
    let targets = crate::app::export_document_targets(&h.s, &out);
    let views = crate::app::export_all_targets(&h.s, dir.path(), ExportFormat::Svg).len();
    assert!(views >= 2, "the fixture has several views");
    assert_eq!(targets.len(), views);
    assert!(targets.iter().all(|(_, p)| *p == out));
    h.cmd(Command::Export { targets, format: ExportFormat::Pdf });
    assert!(h.notice().starts_with("exported"), "{}", h.notice());
    let bytes = std::fs::read(&out).expect("the PDF");
    assert_eq!(pages(&bytes), views, "one page per view");
    // Nothing else was written beside it.
    let files: Vec<_> = std::fs::read_dir(dir.path()).expect("dir").flatten().map(|e| e.file_name()).collect();
    assert_eq!(files.len(), 2, "{files:?}");
}

#[test]
fn file_export_whole_workspace_as_svg_still_asks_a_folder() {
    let (_dir, _, mut h) = shop("file-svg-all");
    h.cmd(Command::ExportAll(ExportFormat::Svg));
    assert!(matches!(h.dialogs.last(), Some(Dialog::ExportAll { format: ExportFormat::Svg, .. })));
    h.cmd(Command::ExportAll(ExportFormat::Png));
    assert!(matches!(h.dialogs.last(), Some(Dialog::ExportAll { format: ExportFormat::Png, .. })));
}

#[test]
fn file_save_writes_nothing_and_says_so() {
    let (_dir, path, mut h) = shop("file-save");
    let before = std::fs::read(&path).expect("file");
    h.cmd(Command::Save);
    assert_eq!(h.writes, 0);
    assert!(h.dialogs.is_empty());
    assert_eq!(std::fs::read(&path).expect("file"), before);
    assert!(h.notice().starts_with("saved"), "{}", h.notice());
    assert!(h.notice().contains("shop.dsl"), "{}", h.notice());
}

#[test]
fn file_save_as_offers_the_open_text_and_opens_the_copy() {
    let (dir, path, mut h) = shop("file-save-as");
    h.cmd(Command::ShowSaveAsDialog);
    let Some(Dialog::SaveAs { dir: start, name, text }) = h.dialogs.last().cloned() else {
        panic!("{:?}", h.dialogs.last())
    };
    assert_eq!(start, dir.path());
    assert_eq!(name, "shop.dsl");
    assert_eq!(text, std::fs::read_to_string(&path).expect("file"));
    // The shell's answer: the text written to the chosen file, then opened.
    let copy = dir.join("copy.dsl");
    files::write_atomic(&copy, &text).expect("written");
    h.cmd(Command::Open(copy.clone()));
    assert_eq!(h.s.doc.as_ref().map(|d| d.path.clone()), Some(copy));
    assert!(h.s.status().problem.is_none(), "{:?}", h.s.status().problem);
}

#[test]
fn file_commands_without_a_file_do_nothing() {
    let mut s = State::new(UiState::default());
    for command in [Command::Save, Command::ShowSaveAsDialog, Command::ExportAll(ExportFormat::Pdf), Command::ExportView(ExportFormat::Pdf)] {
        assert!(s.handle(Event::Command(command.clone())).is_empty(), "{command:?}");
    }
}

#[test]
fn file_commands_pass_while_explore_is_open() {
    let events = vec![
        Event::Command(Command::Save),
        Event::Command(Command::ShowSaveAsDialog),
        Event::Command(Command::ExportView(ExportFormat::Pdf)),
        Event::Command(Command::ExportAll(ExportFormat::Svg)),
        Event::Command(Command::ShowOpenDialog),
    ];
    let (kept, leave) = crate::app::explore_gate(events);
    assert_eq!(kept.len(), 5);
    assert!(!leave, "Explore stays open");
}

// ------------------------------------------------------------- settings

/// Owner request 2026-11-23, the Settings menu: "Max crossings" writes
/// the open view's `maxCrossings` line as one undo step, rewrites it in
/// place, and 0 (the default) removes it.
#[test]
fn settings_max_crossings_writes_the_view_line() {
    let (_dir, path, mut h) = shop("settings-max-crossings");
    let text = || std::fs::read_to_string(&path).expect("read back");
    assert_eq!(h.s.max_crossings(), Some(0));
    h.cmd(Command::SetMaxCrossings(2));
    assert_eq!(h.writes, 1);
    assert_eq!(text().matches("maxCrossings 2").count(), 1, "{}", text());
    assert_eq!(h.s.max_crossings(), Some(2));
    // The same budget again writes nothing.
    h.cmd(Command::SetMaxCrossings(2));
    assert_eq!(h.writes, 1);
    // Rewritten in place.
    h.cmd(Command::SetMaxCrossings(3));
    assert!(text().contains("maxCrossings 3") && !text().contains("maxCrossings 2"), "{}", text());
    // Undo steps back one change at a time.
    h.cmd(Command::Undo);
    assert_eq!(h.s.max_crossings(), Some(2));
    h.cmd(Command::Undo);
    assert_eq!(text(), SHOP);
    // 0 removes the line.
    h.cmd(Command::SetMaxCrossings(1));
    h.cmd(Command::SetMaxCrossings(0));
    assert_eq!(text(), SHOP);
    assert_eq!(h.s.max_crossings(), Some(0));
}

/// Without a file there is no view to set: nothing is written.
#[test]
fn settings_without_a_file_do_nothing() {
    let mut s = State::new(UiState::default());
    assert_eq!(s.max_crossings(), None);
    assert!(s.handle(Event::Command(Command::SetMaxCrossings(2))).is_empty());
}

/// P5-309: the Settings menu's "Line colours" writes the open view's
/// `lineColours` line as one undo step, the same mode again writes
/// nothing, undo gives the file back byte for byte, and plain (the
/// default) removes the line. The scene follows.
#[test]
fn p5_309_settings_line_colours_writes_the_view_line() {
    use c4rizer_core::model::LineColours;
    let (_dir, path, mut h) = shop("settings-line-colours");
    let text = || std::fs::read_to_string(&path).expect("read back");
    assert_eq!(h.s.line_colours(), Some(LineColours::Plain));
    h.cmd(Command::SetLineColours(LineColours::Box));
    assert_eq!(h.writes, 1);
    assert_eq!(text().matches("lineColours box").count(), 1, "{}", text());
    assert_eq!(h.s.line_colours(), Some(LineColours::Box));
    assert_eq!(h.s.scene().expect("scene").options.line_colours, LineColours::Box);
    // The same mode again writes nothing.
    h.cmd(Command::SetLineColours(LineColours::Box));
    assert_eq!(h.writes, 1);
    // One undo step gives the file back.
    h.cmd(Command::Undo);
    assert_eq!(text(), SHOP);
    assert_eq!(h.s.line_colours(), Some(LineColours::Plain));
    // Plain removes the line.
    h.cmd(Command::SetLineColours(LineColours::Box));
    let boxed = text();
    h.cmd(Command::SetLineColours(LineColours::Plain));
    assert_eq!(text(), SHOP);
    assert_eq!(h.s.scene().expect("scene").options.line_colours, LineColours::Plain);
    // And its undo brings the line back where it was.
    h.cmd(Command::Undo);
    assert_eq!(text(), boxed);
}

/// P5-309: without a file there is no view to set: nothing is written.
#[test]
fn p5_309_line_colours_without_a_file_do_nothing() {
    use c4rizer_core::model::LineColours;
    let mut s = State::new(UiState::default());
    assert_eq!(s.line_colours(), None);
    assert!(s.handle(Event::Command(Command::SetLineColours(LineColours::Box))).is_empty());
}
