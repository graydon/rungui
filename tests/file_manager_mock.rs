//! Builds the file manager example's UI against the headless mock backend and drives it:
//! navigation, sorting, preview, copy / move / rename / delete, the context menu.
#![cfg(feature = "mock")]
#![allow(dead_code)]

#[path = "../examples/file_manager/app.rs"]
mod app;
#[path = "../examples/file_manager/fsmodel.rs"]
mod fsmodel;

use rungui::backend::mock;
use rungui::*;
use std::fs;
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0); // tests run in parallel threads
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("rungui-fm-mock-{}-{n}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn names(t: Table) -> Vec<String> {
    t.rows().iter().map(|r| r[0].trim().trim_start_matches('\u{25B8}').trim().to_string()).collect()
}

fn row_of(t: Table, name: &str) -> usize {
    names(t).iter().position(|n| n == name).unwrap_or_else(|| panic!("{name} not in {:?}", names(t)))
}

fn setup() -> (std::rc::Rc<app::Fm>, PathBuf, PathBuf) {
    let _ = App::new("fm-test");
    let a = tmp("a");
    let b = tmp("b");
    fs::write(a.join("one.txt"), "first file\n").unwrap();
    fs::write(a.join("big.bin"), vec![0u8, 1, 2, 3, 255, 0, 9]).unwrap();
    fs::write(a.join("日本語.txt"), "unicode name").unwrap();
    fs::write(a.join(".secret"), "hidden").unwrap();
    fs::create_dir_all(a.join("sub/deeper")).unwrap();
    fs::write(a.join("sub/inner.txt"), "inner").unwrap();
    let fm = app::build(app::Options { dirs: [a.clone(), b.clone()] });
    (fm, a, b)
}

#[test]
fn navigate_sort_preview() {
    let (fm, a, _b) = setup();
    let u = &fm.ui;
    let [ta, tb] = u.tables;
    // folders first, ".." on top, hidden files filtered out
    assert_eq!(names(ta), ["..", "sub", "big.bin", "one.txt", "日本語.txt"]);
    assert_eq!(names(tb), [".."]);
    assert_eq!(ta.sort_indicator(), Some((0, true)));
    assert_eq!(u.path.text(), a.display().to_string());
    assert!(u.win.title().contains("[A]"));

    // text preview
    mock::user_select_row(ta.id(), Some(row_of(ta, "one.txt")));
    assert_eq!(u.preview.text(), "first file\n");
    assert!(u.status.text().contains("one.txt"));
    // binary preview is a hex dump
    mock::user_select_row(ta.id(), Some(row_of(ta, "big.bin")));
    assert!(u.preview.text().contains("00 01 02 03 ff 00 09"), "{}", u.preview.text());
    // directory preview is a summary
    mock::user_select_row(ta.id(), Some(row_of(ta, "sub")));
    assert!(u.preview.text().contains("2 items"), "{}", u.preview.text());

    // sort by size descending: click the Size column twice
    mock::user_click_column(ta.id(), 1);
    mock::user_click_column(ta.id(), 1);
    assert_eq!(ta.sort_indicator(), Some((1, false)));
    assert_eq!(names(ta)[..2], ["..", "sub"]);
    assert_eq!(names(ta)[2..], ["日本語.txt", "one.txt", "big.bin"]);
    // the selection survives a re-sort; activating a folder navigates, ".." goes back up
    mock::user_activate_row(ta.id(), row_of(ta, "sub"));
    assert_eq!(fm.path_of(0), a.join("sub"));
    assert_eq!(names(ta), ["..", "deeper", "inner.txt"]);
    mock::user_activate_row(ta.id(), 0);
    assert_eq!(fm.path_of(0), a);
    assert_eq!(names(ta)[ta.selected().unwrap()], "sub"); // came from "sub": selected again

    // typing a path navigates the active pane; a bad path does nothing
    mock::user_text(u.path.id(), &a.join("sub/deeper").display().to_string());
    assert_eq!(fm.path_of(0), a.join("sub/deeper"));
    mock::user_text(u.path.id(), "/definitely/not/here");
    assert_eq!(fm.path_of(0), a.join("sub/deeper"));
    mock::user_click(u.up.id());
    mock::user_click(u.up.id());
    assert_eq!(fm.path_of(0), a);

    // Show hidden files
    u.items.hidden.set_checked(true);
    mock::user(u.items.hidden.id(), Event::Toggled(true));
    assert!(names(ta).contains(&".secret".to_string()));
    mock::user(u.items.hidden.id(), Event::Toggled(false));
    assert!(!names(ta).contains(&".secret".to_string()));
}

#[test]
fn tree_is_lazy_and_navigates() {
    let (fm, a, _b) = setup();
    let u = &fm.ui;
    // the active pane's folder is revealed and selected in the tree
    let sel = u.tree.selected().expect("a folder is selected");
    assert_eq!(fm.node_path(sel), Some(a.clone()));
    // clicking another folder navigates the active pane
    let parent = u.tree.parent(sel).expect("nested under its parent");
    mock::user_tree_select(u.tree.id(), Some(parent.0));
    assert_eq!(fm.path_of(0), a.parent().unwrap());
    // expanding a node loads its children once
    let kids = u.tree.children(Some(parent)).len();
    mock::user_tree_expand(u.tree.id(), parent.0, true);
    assert_eq!(u.tree.children(Some(parent)).len(), kids);
}

#[test]
fn file_operations() {
    let (fm, a, b) = setup();
    let u = &fm.ui;
    let [ta, tb] = u.tables;

    // copy to the other pane (F5 / button)
    mock::user_select_row(ta.id(), Some(row_of(ta, "one.txt")));
    mock::user_click(u.copy_btn.id());
    assert_eq!(fs::read_to_string(b.join("one.txt")).unwrap(), "first file\n");
    assert!(a.join("one.txt").exists());
    assert!(names(tb).contains(&"one.txt".to_string()));
    // copying again asks before overwriting; "No" leaves things alone
    fs::write(a.join("one.txt"), "changed").unwrap();
    mock::queue_answer(Answer::No);
    mock::user_select_row(ta.id(), Some(row_of(ta, "one.txt")));
    mock::user_click(u.copy_btn.id());
    assert_eq!(mock::last_message().unwrap().title, "Overwrite");
    assert_eq!(fs::read_to_string(b.join("one.txt")).unwrap(), "first file\n");
    mock::queue_answer(Answer::Yes);
    mock::user_click(u.copy_btn.id());
    assert_eq!(fs::read_to_string(b.join("one.txt")).unwrap(), "changed");

    // move a folder
    mock::user_select_row(ta.id(), Some(row_of(ta, "sub")));
    mock::user_click(u.move_btn.id());
    assert!(!a.join("sub").exists() && b.join("sub/deeper").is_dir());
    assert!(!names(ta).contains(&"sub".to_string()) && names(tb).contains(&"sub".to_string()));

    // a folder cannot be copied into itself
    mock::user_select_row(tb.id(), Some(row_of(tb, "sub")));
    assert_eq!(fm.active(), 1);
    fm.navigate_for_test(0, b.join("sub"));
    mock::user_select_row(tb.id(), Some(row_of(tb, "sub")));
    mock::user_click(u.copy_btn.id());
    assert!(u.status.text().contains("itself") || u.status.text().contains("same"), "{}", u.status.text());

    // rename through the dialog
    fm.navigate_for_test(0, a.clone());
    mock::user_select_row(ta.id(), Some(row_of(ta, "big.bin")));
    mock::user_click(u.items.rename.id());
    let (win, input, ok, _cancel) = fm.dialog_ui.get().expect("dialog opened");
    assert!(win.is_alive());
    assert_eq!(input.text(), "big.bin");
    mock::user_text(input.id(), "a/b"); // invalid: dialog stays open with an error
    mock::user_click(ok.id());
    assert!(win.is_alive() && a.join("big.bin").exists());
    mock::user_text(input.id(), "renamed.bin");
    mock::user_click(ok.id());
    assert!(!win.is_alive());
    assert!(a.join("renamed.bin").exists() && !a.join("big.bin").exists());
    assert_eq!(names(ta)[ta.selected().unwrap()], "renamed.bin");

    // new folder through the dialog; Cancel changes nothing
    mock::user_click(u.items.new_folder.id());
    let (win, input, _ok, cancel) = fm.dialog_ui.get().unwrap();
    mock::user_click(cancel.id());
    assert!(!win.is_alive());
    mock::user_click(u.items.new_folder.id());
    let (win, input2, ok, _) = fm.dialog_ui.get().unwrap();
    let _ = input;
    mock::user_text(input2.id(), "Fresh");
    mock::user_click(ok.id());
    assert!(!win.is_alive() && a.join("Fresh").is_dir());
    assert!(names(ta).contains(&"Fresh".to_string()));

    // delete asks first
    mock::user_select_row(ta.id(), Some(row_of(ta, "Fresh")));
    mock::queue_answer(Answer::No);
    mock::user_click(u.items.delete.id());
    assert!(a.join("Fresh").exists());
    mock::queue_answer(Answer::Yes);
    mock::user_click(u.items.delete.id());
    assert!(!a.join("Fresh").exists());
    assert!(!names(ta).contains(&"Fresh".to_string()));
    assert!(u.status.text().contains("Fresh"));
}

#[test]
fn context_menu_and_errors() {
    let (fm, a, _b) = setup();
    let u = &fm.ui;
    let ta = u.tables[0];
    // nothing real selected: the item-specific entries are disabled
    mock::user_select_row(ta.id(), Some(0)); // ".."
    mock::user_context_menu(ta.id(), 5, 5);
    let rec = mock::last_popup().unwrap();
    let enabled = |label: &str| rec.items.iter().find(|i| i.1.starts_with(label)).map(|i| i.2).unwrap();
    assert!(enabled("Open") && !enabled("Rename") && !enabled("Delete") && !enabled("Copy"));
    assert!(enabled("New Folder"));
    mock::user_select_row(ta.id(), Some(row_of(ta, "one.txt")));
    mock::user_context_menu(ta.id(), 5, 5);
    assert!(mock::last_popup().unwrap().items.iter().all(|i| i.2 || i.0 == Kind::MenuSeparator));
    mock::queue_popup_choice(Some(u.items.ctx_delete.id()));
    // right-clicking the other table makes that pane active
    mock::user_context_menu(u.tables[1].id(), 1, 1);
    assert_eq!(fm.active(), 1);

    // errors go to the status bar, never panic
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = a.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&locked).is_err() {
            // (skipped when running as root, which can read anything)
            mock::user_activate_row(ta.id(), 0); // make sure pane 0 is where we think
            fm.navigate_for_test(0, a.clone());
            mock::user_activate_row(ta.id(), row_of(ta, "locked"));
            assert!(u.status.text().starts_with("Cannot open"), "{}", u.status.text());
            assert_eq!(fm.path_of(0), a);
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    }
    mock::queue_answer(Answer::Yes);
    let before = names(ta);
    fs::remove_file(a.join("one.txt")).unwrap(); // vanished behind our back
    mock::user_select_row(ta.id(), Some(row_of(ta, "日本語.txt")));
    fm.navigate_for_test(0, a.clone());
    assert!(!names(ta).contains(&"one.txt".to_string()) && before.contains(&"one.txt".to_string()));
}
