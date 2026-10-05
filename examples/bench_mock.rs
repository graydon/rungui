//! Scaling benchmark on the headless mock backend: times the core's hot paths at doubling sizes and
//! prints the measured growth exponent (1.0 = linear, 2.0 = quadratic). It measures rungui's own
//! bookkeeping (registry, layout, models), not any toolkit.
//!
//!   cargo run --release --features mock --example bench_mock [-- --max-exponent 1.5 [filter]]
//!
//! With `--max-exponent X` the exit status is non-zero if any benchmark grows faster than `n^X`.
use rungui::backend::mock;
use rungui::*;
use std::time::{Duration, Instant};

/// Problem sizes: each benchmark runs at `BASE << i` for `i in 0..STEPS`.
const BASE: usize = 1000;
const STEPS: usize = 4;
/// Timings below this are dominated by noise and are not used for the exponent fit.
const NOISE_FLOOR: Duration = Duration::from_micros(200);

type Bench = (&'static str, fn(usize));

/// A chain of boxes as deep as the nesting limit allows, with `n / depth` labels at every level.
fn deep_chain(n: usize) {
    let win = Window::new("deep");
    let depth = MAX_NESTING - 2;
    let mut parent: WidgetId = win.id();
    for i in 0..depth {
        parent = if i % 2 == 0 {
            VBox::new(parent).id()
        } else {
            HBox::new(parent).id()
        };
        for _ in 0..n / depth {
            Label::new(parent, "x");
        }
    }
    win.show();
    App::update();
    win.destroy();
}

fn window_with<R>(n: usize, f: impl FnOnce(Window, VBox) -> R) -> R {
    let win = Window::new("bench");
    let col = VBox::new(win);
    for i in 0..n {
        Label::new(col, &format!("label {i}"));
    }
    let r = f(win, col);
    win.destroy();
    r
}

const BENCHES: &[Bench] = &[
    ("create labels", |n| {
        window_with(n, |_, _| ());
    }),
    ("layout (n labels)", |n| {
        window_with(n, |win, _| {
            win.show();
            for _ in 0..10 {
                win.set_size(500, 400);
                App::update();
                win.set_size(501, 401);
                App::update();
            }
        })
    }),
    ("destroy children one by one", |n| {
        window_with(n, |_, col| {
            let kids: Vec<WidgetId> = (0..n).map(|_| Label::new(col, "x").id()).collect();
            for k in kids {
                Widget(k).destroy();
            }
        })
    }),
    ("hide/show every child", |n| {
        window_with(n, |_, col| {
            let kids: Vec<WidgetId> = (0..n).map(|_| Label::new(col, "x").id()).collect();
            for k in &kids {
                Widget(*k).set_visible(false);
            }
            for k in &kids {
                Widget(*k).set_visible(true);
            }
        })
    }),
    ("table push_row (unbatched)", |n| {
        window_with(0, |win, col| {
            let _ = win;
            let t = Table::new(col);
            t.set_columns(&[Column::new("a"), Column::new("b")]);
            for i in 0..n / 4 {
                t.push_row(&[i.to_string(), "x".to_string()]);
            }
            App::update(); // the one transfer of the whole model
        })
    }),
    ("table push_row (batched)", |n| {
        window_with(0, |_, col| {
            let t = Table::new(col);
            t.set_columns(&[Column::new("a"), Column::new("b")]);
            t.batch(|t| {
                for i in 0..n {
                    t.push_row(&[i.to_string(), "x".to_string()]);
                }
            });
            for i in 0..n {
                t.set_selected(Some(i % 100));
            }
        })
    }),
    ("table select (n rows)", |n| {
        window_with(0, |_, col| {
            let t = Table::new(col);
            t.set_columns(&[Column::new("a")]);
            let rows: Vec<Vec<String>> = (0..n).map(|i| vec![i.to_string()]).collect();
            t.set_rows(&rows);
            for i in 0..200 {
                mock::user_select_row(t.id(), Some(i % n));
            }
        })
    }),
    ("tree add (unbatched)", |n| {
        window_with(0, |_, col| {
            let t = Tree::new(col);
            let root = t.add(None, "root");
            for i in 0..n / 4 {
                t.add(Some(root), &i.to_string());
            }
            App::update();
        })
    }),
    ("tree add (batched)", |n| {
        window_with(0, |_, col| {
            let t = Tree::new(col);
            t.batch(|t| {
                let root = t.add(None, "root");
                for i in 0..n {
                    t.add(Some(root), &i.to_string());
                }
            });
        })
    }),
    ("tree deep chain", |n| {
        window_with(0, |_, col| {
            let t = Tree::new(col);
            t.batch(|t| {
                let mut p = None;
                for i in 0..n {
                    p = Some(t.add(p, &i.to_string()));
                }
                t.set_selected(p);
            });
        })
    }),
    ("radio toggles (one group)", |n| {
        window_with(0, |_, col| {
            let g = RadioGroup::new();
            let rs: Vec<RadioButton> = (0..n).map(|_| RadioButton::new(col, &g, "r")).collect();
            for r in rs.iter().take(200) {
                r.set_checked(true);
            }
            for r in rs.iter().take(200) {
                mock::user(r.id(), Event::Toggled(true));
            }
        })
    }),
    ("timers start/stop", |n| {
        let ts: Vec<Timer> = (0..n).map(|_| Timer::every(10, || {})).collect();
        for t in ts {
            t.stop();
        }
    }),
    ("post closures", |n| {
        for _ in 0..n {
            App::post(|| {});
        }
        App::update();
    }),
    ("listbox set_items + select", |n| {
        window_with(0, |_, col| {
            let l = ListBox::new(col);
            let items: Vec<String> = (0..n).map(|i| i.to_string()).collect();
            l.set_items(&items);
            for i in 0..100 {
                l.set_selected(Some(i));
            }
        })
    }),
    ("grid cells", |n| {
        let win = Window::new("grid");
        let g = Grid::new(win, 10);
        for i in 0..n {
            Label::new(g, &i.to_string());
        }
        win.show();
        App::update();
        win.destroy();
    }),
    ("deep chain (max nesting)", deep_chain),
];

fn main() {
    let mut args = std::env::args().skip(1);
    let mut max_exp: Option<f64> = None;
    let mut filter: Option<String> = None;
    while let Some(a) = args.next() {
        if a == "--max-exponent" {
            max_exp = args.next().and_then(|v| v.parse().ok());
        } else {
            filter = Some(a);
        }
    }
    let _app = App::new("bench").expect("init");
    let mut worst = 0.0f64;
    let mut bad = vec![];
    println!(
        "{:<34}{}  exponent",
        "benchmark",
        (0..STEPS)
            .map(|i| format!("{:>10}", BASE << i))
            .collect::<String>()
    );
    for (name, f) in BENCHES {
        if filter.as_ref().is_some_and(|p| !name.contains(p.as_str())) {
            continue;
        }
        let mut times = vec![];
        for i in 0..STEPS {
            let n = BASE << i;
            let t0 = Instant::now();
            f(n);
            times.push(t0.elapsed());
            assert_eq!(mock::widget_count(), 0, "{name}: leaked widgets");
            assert_eq!(mock::timer_count(), 0, "{name}: leaked timers");
        }
        // least-squares slope of log(time) against log(n), using the points above the noise floor
        let pts: Vec<(f64, f64)> = times
            .iter()
            .enumerate()
            .filter(|(_, t)| **t >= NOISE_FLOOR)
            .map(|(i, t)| (((BASE << i) as f64).ln(), t.as_secs_f64().ln()))
            .collect();
        let exp = if pts.len() >= 2 {
            let m = pts.len() as f64;
            let (sx, sy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
            let (sxx, sxy) = pts
                .iter()
                .fold((0.0, 0.0), |a, p| (a.0 + p.0 * p.0, a.1 + p.0 * p.1));
            Some((m * sxy - sx * sy) / (m * sxx - sx * sx))
        } else {
            None
        };
        println!(
            "{:<34}{}  {}",
            name,
            times
                .iter()
                .map(|t| format!("{:>8.2}ms", t.as_secs_f64() * 1e3))
                .collect::<String>(),
            exp.map_or("-".to_string(), |e| format!("{e:.2}"))
        );
        if let Some(e) = exp {
            worst = worst.max(e);
            if max_exp.is_some_and(|m| e > m) {
                bad.push(*name);
            }
        }
    }
    println!("worst exponent: {worst:.2}");
    if !bad.is_empty() {
        eprintln!("FAIL: super-linear growth in: {}", bad.join(", "));
        std::process::exit(1);
    }
}
