//! `App::post` from other threads on the Win32 backend, with the wake-up queue under load. It is
//! the only test of its process: posts go to the thread that initialised the toolkit last, so
//! another test starting its own toolkit meanwhile (as in `win32_native.rs`) would take them.
#![cfg(windows)]

use rungui::*;

#[test]
fn closures_posted_from_other_threads_run_on_the_ui_thread() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let app = App::new("win32-post").expect("init");
    let win = Window::new("t");
    let label = Label::new(VBox::new(win), "before");
    let ran = Arc::new(AtomicUsize::new(0));
    let ui = std::thread::current().id();
    const POSTS: usize = 2_000;
    let r = ran.clone();
    let poster = std::thread::spawn(move || {
        for i in 0..POSTS {
            let r = r.clone();
            App::post(move || {
                assert_eq!(std::thread::current().id(), ui);
                r.fetch_add(1, Ordering::SeqCst);
                if i == POSTS - 1 {
                    label.set_text("after");
                }
            });
        }
    });
    // keep the loop's queue busy meanwhile: the wake-up of the posts must not get lost behind it
    let t = Timer::every(1, move || {
        if ran.load(Ordering::SeqCst) == POSTS {
            App::quit();
        }
    });
    app.run();
    t.stop();
    poster.join().unwrap();
    assert_eq!(label.text(), "after");
}
