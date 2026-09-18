use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Stack, StackTransitionType};

use crate::analyze;
use crate::data::{
    default_db_path, empty_library, ensure_library_files, load_library, save_index, Library,
    Settings,
};
use crate::sync::{self, settings_username, SyncReport};
use crate::theme::ThemeHandle;
use crate::ui::playbench::SyncUiState;
use crate::ui::{LibraryPage, Playbench, ProgressPage};

fn analyze_progress_text(cur: usize, total: usize, msg: &str) -> String {
    if msg.starts_with("Downloading") {
        msg.to_string()
    } else if total > 0 {
        format!("Analyzing game · {cur}/{total}")
    } else {
        "Analyzing game…".to_string()
    }
}

enum BgMsg {
    LibraryReady(Library),
    NeedUsername(Library),
    SyncProgress(Library),
    SyncFinished {
        library: Library,
        report: SyncReport,
    },
    /// Background backfill of endFen/endPly/endUci for list previews.
    EndFenReady(Library),
    AnalyzeProgress(String),
    AnalyzeFinished {
        id: String,
        game_type: String,
        /// Games whose `hasAnalysis` should flip true in memory (no index reload).
        analyzed_ids: Vec<(String, String)>,
        message: String,
        /// True when the user-triggered Analyze button held the UI busy.
        blocking: bool,
        /// Mark as reviewed when finishing while on the playbench for this game.
        /// False for Analyze all / list-only analyze.
        count_as_review: bool,
        ok: bool,
    },
}

pub fn build_ui(app: &Application) {
    let t_ui = Instant::now();
    let theme = ThemeHandle::install();
    eprintln!("reprise: theme install {:.2?}", t_ui.elapsed());

    let path = default_db_path();
    let library = Rc::new(RefCell::new(empty_library(path.clone())));

    let t_bench = Instant::now();
    let playbench = Rc::new(Playbench::new(library.clone(), theme.colors.clone()));
    let library_page = Rc::new(LibraryPage::new(library.clone(), theme.colors.clone()));
    let progress_page = Rc::new(ProgressPage::new(library.clone(), theme.colors.clone()));
    eprintln!("reprise: playbench construct {:.2?}", t_bench.elapsed());
    playbench.set_sync_state(SyncUiState::Syncing);

    let stack = Stack::new();
    stack.set_transition_type(StackTransitionType::Crossfade);
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.add_named(&playbench.widget, Some("playbench"));
    stack.add_named(&library_page.widget, Some("library"));
    stack.add_named(&progress_page.widget, Some("progress"));
    stack.set_visible_child_name("playbench");

    let show_playbench = {
        let stack = stack.clone();
        let playbench = playbench.clone();
        Rc::new(move || {
            stack.set_visible_child_name("playbench");
            playbench.grab_focus();
        })
    };
    let show_library = {
        let stack = stack.clone();
        let library_page = library_page.clone();
        Rc::new(move || {
            library_page.refresh();
            stack.set_visible_child_name("library");
            library_page.grab_focus();
        })
    };
    let show_progress = {
        let stack = stack.clone();
        let progress_page = progress_page.clone();
        Rc::new(move || {
            progress_page.refresh();
            stack.set_visible_child_name("progress");
            progress_page.grab_focus();
        })
    };

    playbench.on_open_library({
        let show_library = show_library.clone();
        move || show_library()
    });
    playbench.on_open_progress({
        let show_progress = show_progress.clone();
        move || show_progress()
    });
    library_page.on_back({
        let show_playbench = show_playbench.clone();
        move || show_playbench()
    });
    progress_page.on_back({
        let show_playbench = show_playbench.clone();
        move || show_playbench()
    });
    library_page.on_open({
        let playbench = playbench.clone();
        let show_playbench = show_playbench.clone();
        move |game| {
            playbench.open_game(&game.id, &game.game_type);
            show_playbench();
            playbench.mark_current_reviewed();
        }
    });
    library_page.on_play({
        let playbench = playbench.clone();
        let show_playbench = show_playbench.clone();
        move |game| {
            let needs_analyze = !game.is_analyzed();
            playbench.open_game(&game.id, &game.game_type);
            show_playbench();
            if needs_analyze {
                playbench.request_analyze();
            } else {
                playbench.mark_current_reviewed();
            }
        }
    });
    library_page.on_analyze({
        let playbench = playbench.clone();
        move |game| {
            // Stay on the list — set current for analyze only (does not mark reviewed).
            playbench.open_game(&game.id, &game.game_type);
            playbench.request_analyze_now();
        }
    });
    library_page.on_sync({
        let playbench = playbench.clone();
        move || playbench.request_sync()
    });

    theme.on_change({
        let repaint = playbench.board_repaint_callback();
        let lib_repaint = library_page.board_repaint_callback();
        let progress_repaint = progress_page.board_repaint_callback();
        move || {
            repaint();
            lib_repaint();
            progress_repaint();
        }
    });

    let (tx, rx) = async_channel::unbounded::<BgMsg>();
    let auto_analyzing = Arc::new(AtomicBool::new(false));

    let try_auto_analyze = {
        let playbench = playbench.clone();
        let library_page = library_page.clone();
        let tx = tx.clone();
        let path = path.clone();
        let auto_analyzing = auto_analyzing.clone();
        Rc::new(move || {
            if auto_analyzing
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return;
            }
            let Some((id, game_type, pgn)) = playbench.newest_unanalyzed() else {
                auto_analyzing.store(false, Ordering::SeqCst);
                return;
            };
            eprintln!("reprise: auto-analyze newest game {id}");
            // Open on playbench so finishing analysis counts as a review.
            playbench.open_game(&id, &game_type);
            playbench.set_analyzing(Some((id.clone(), game_type.clone())));
            library_page.set_analyzing(Some((id.clone(), game_type.clone())));
            playbench.set_analyze_progress("Analyzing game…");
            let tx = tx.clone();
            let path = path.clone();
            let auto_analyzing = auto_analyzing.clone();
            std::thread::spawn(move || {
                let tx_prog = tx.clone();
                let report = analyze::analyze_and_save(
                    &path,
                    &id,
                    &game_type,
                    &pgn,
                    18,
                    3,
                    |cur, total, msg| {
                        let text = analyze_progress_text(cur, total, msg);
                        let _ = tx_prog.send_blocking(BgMsg::AnalyzeProgress(text));
                    },
                )
                .unwrap_or_else(|e| analyze::AnalyzeReport {
                    ok: false,
                    message: e.to_string(),
                    analyzed: 0,
                    failed: 1,
                });
                auto_analyzing.store(false, Ordering::SeqCst);
                let _ = tx.send_blocking(BgMsg::AnalyzeFinished {
                    analyzed_ids: if report.ok {
                        vec![(id.clone(), game_type.clone())]
                    } else {
                        Vec::new()
                    },
                    id,
                    game_type,
                    message: report.message,
                    blocking: false,
                    count_as_review: true,
                    ok: report.ok,
                });
            });
        })
    };

    let spawn_sync = {
        let tx = tx.clone();
        let path = path.clone();
        Rc::new(move || {
            let tx = tx.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let t_load = Instant::now();
                let lib = match load_library(&path) {
                    Ok(lib) => {
                        eprintln!(
                            "reprise: load library {} games in {:.2?}",
                            lib.games.len(),
                            t_load.elapsed()
                        );
                        lib
                    }
                    Err(err) => {
                        eprintln!("reprise: failed to load {}: {err:#}", path.display());
                        empty_library(path)
                    }
                };
                let _ = tx.send_blocking(BgMsg::LibraryReady(lib.clone()));

                // Backfill end positions for list preview (off UI thread).
                {
                    let mut lib = lib.clone();
                    let tx_bf = tx.clone();
                    std::thread::spawn(move || {
                        let t0 = Instant::now();
                        let n = lib.backfill_end_positions();
                        if n > 0 {
                            if let Err(err) = save_index(&lib) {
                                eprintln!("reprise: endFen backfill save failed: {err:#}");
                            } else {
                                eprintln!(
                                    "reprise: endFen backfill {n} games in {:.2?}",
                                    t0.elapsed()
                                );
                                let _ = tx_bf.send_blocking(BgMsg::EndFenReady(lib));
                            }
                        }
                    });
                }

                if settings_username(&lib.settings).is_none() {
                    let _ = tx.send_blocking(BgMsg::NeedUsername(lib));
                    return;
                }

                run_sync(lib, tx);
            });
        })
    };

    spawn_sync();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Reprise")
        .default_width(900)
        .default_height(820)
        .child(&stack)
        .build();

    {
        let playbench = playbench.clone();
        let library_page = library_page.clone();
        let progress_page = progress_page.clone();
        let stack = stack.clone();
        let spawn_sync = spawn_sync.clone();
        let try_auto_analyze = try_auto_analyze.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                let page = stack.visible_child_name();
                let on_library = page.as_deref() == Some("library");
                let on_progress = page.as_deref() == Some("progress");
                match msg {
                    BgMsg::LibraryReady(lib) => {
                        playbench.replace_library(lib);
                        playbench.set_sync_state(SyncUiState::Syncing);
                        // Do not auto-analyze here — wait until sync imports
                        // something new, otherwise a stale index re-analyzes
                        // every unanalyzed-looking newest game on launch.
                        if on_library {
                            library_page.refresh();
                        }
                        if on_progress {
                            progress_page.refresh();
                        }
                    }
                    BgMsg::EndFenReady(lib) => {
                        playbench.update_library_only(lib);
                        if on_progress {
                            progress_page.refresh();
                        }
                    }
                    BgMsg::NeedUsername(lib) => {
                        playbench.set_sync_state(SyncUiState::NeedsRefresh);
                        let pb = playbench.clone();
                        let spawn_sync = spawn_sync.clone();
                        playbench.prompt_username(move |username| {
                            let mut lib = lib.clone();
                            lib.settings = Settings {
                                my_username: Some(username),
                            };
                            if let Err(err) = ensure_library_files(&lib) {
                                eprintln!("reprise: failed to save username: {err:#}");
                                pb.set_sync_state(SyncUiState::NeedsRefresh);
                                return;
                            }
                            pb.replace_library(lib);
                            pb.set_sync_state(SyncUiState::Syncing);
                            spawn_sync();
                        });
                    }
                    BgMsg::SyncProgress(library) => {
                        // Early batch of newly imported games — analyze newest now.
                        playbench.replace_library(library);
                        playbench.set_status("New games found…");
                        playbench.refresh_analyze_cluster();
                        if on_library {
                            library_page.refresh();
                        }
                        if on_progress {
                            progress_page.refresh();
                        }
                        try_auto_analyze();
                    }
                    BgMsg::SyncFinished { library, report } => {
                        if report.imported > 0 {
                            playbench.replace_library(library);
                        } else {
                            playbench.update_library_only(library);
                        }
                        if report.offline {
                            playbench.set_sync_state(SyncUiState::NeedsRefresh);
                        } else {
                            playbench.set_sync_state(SyncUiState::UpToDate);
                            // Don't clobber an in-flight analyze badge.
                            if !playbench.is_analyzing() {
                                if report.imported > 0 {
                                    playbench.note_new_games(report.imported);
                                } else {
                                    playbench.note_up_to_date();
                                }
                            }
                        }
                        playbench.refresh_analyze_cluster();
                        if on_library {
                            library_page.refresh();
                        }
                        if on_progress {
                            progress_page.refresh();
                        }
                        if report.imported > 0 {
                            try_auto_analyze();
                        }
                    }
                    BgMsg::AnalyzeProgress(text) => {
                        playbench.set_analyze_progress(&text);
                    }
                    BgMsg::AnalyzeFinished {
                        id,
                        game_type,
                        analyzed_ids,
                        message,
                        blocking,
                        count_as_review,
                        ok,
                    } => {
                        let t0 = Instant::now();
                        eprintln!("reprise: {message}");
                        playbench.set_analyzing(None);
                        library_page.set_analyzing(None);
                        playbench.clear_status_or_restore_sync();
                        if ok {
                            playbench.mark_analyzed(&analyzed_ids);
                        }
                        let viewing = playbench.current_game_key()
                            == Some((id.clone(), game_type.clone()));

                        if blocking {
                            playbench.set_busy(false);
                        }

                        if !ok {
                            playbench.refresh_analyze_cluster();
                            if on_library {
                                library_page.refresh();
                            }
                            if !on_library {
                                playbench.notify_analysis_failed(&message);
                            }
                        } else if on_library {
                            // Analyzed from the list — refresh markers, no playbench modal.
                            playbench.refresh_analyze_cluster();
                            library_page.refresh();
                            try_auto_analyze();
                        } else if viewing {
                            // Keep the current board until the user acknowledges.
                            if count_as_review {
                                playbench.mark_current_reviewed();
                            }
                            playbench.refresh_analyze_cluster();
                            let pb = playbench.clone();
                            let try_auto_analyze = try_auto_analyze.clone();
                            playbench.notify_analysis_finished(
                                "The board will move to the first pivotal moment.",
                                move || {
                                    pb.refresh_board();
                                    try_auto_analyze();
                                },
                            );
                        } else {
                            playbench.refresh_analyze_cluster();
                            try_auto_analyze();
                        }
                        if ok {
                            progress_page.refresh();
                        }
                        eprintln!(
                            "reprise: AnalyzeFinished UI {:.2?} ({} games, ok={ok})",
                            t0.elapsed(),
                            analyzed_ids.len()
                        );
                    }
                }
            }
        });
    }

    playbench.on_sync({
        let playbench = playbench.clone();
        let spawn_sync = spawn_sync.clone();
        move || {
            // Only block sync while an analyze job holds the busy flag.
            if playbench.is_busy() {
                return;
            }
            if settings_username(&playbench.library_snapshot().settings).is_none() {
                let pb = playbench.clone();
                let spawn_sync = spawn_sync.clone();
                playbench.prompt_username(move |username| {
                    let mut lib = empty_library(default_db_path());
                    lib.settings = Settings {
                        my_username: Some(username),
                    };
                    if let Err(err) = ensure_library_files(&lib) {
                        eprintln!("reprise: failed to save username: {err:#}");
                        return;
                    }
                    pb.replace_library(lib);
                    pb.set_sync_state(SyncUiState::Syncing);
                    spawn_sync();
                });
                return;
            }
            playbench.set_sync_state(SyncUiState::Syncing);
            spawn_sync();
        }
    });

    playbench.on_analyze({
        let playbench = playbench.clone();
        let library_page = library_page.clone();
        let tx = tx.clone();
        let path = path.clone();
        move || {
            if playbench.is_busy() {
                return;
            }
            let Some((id, game_type)) = playbench.current_game_key() else {
                return;
            };
            let Some(pgn) = playbench.current_game_pgn() else {
                return;
            };
            playbench.set_busy(true);
            playbench.set_analyzing(Some((id.clone(), game_type.clone())));
            library_page.set_analyzing(Some((id.clone(), game_type.clone())));
            playbench.set_analyze_progress("Analyzing game…");
            let tx = tx.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let tx_prog = tx.clone();
                let report = analyze::analyze_and_save(
                    &path,
                    &id,
                    &game_type,
                    &pgn,
                    18,
                    3,
                    |cur, total, msg| {
                        let text = analyze_progress_text(cur, total, msg);
                        let _ = tx_prog.send_blocking(BgMsg::AnalyzeProgress(text));
                    },
                )
                .unwrap_or_else(|e| analyze::AnalyzeReport {
                    ok: false,
                    message: e.to_string(),
                    analyzed: 0,
                    failed: 1,
                });
                let _ = tx.send_blocking(BgMsg::AnalyzeFinished {
                    analyzed_ids: if report.ok {
                        vec![(id.clone(), game_type.clone())]
                    } else {
                        Vec::new()
                    },
                    id,
                    game_type,
                    message: report.message,
                    blocking: true,
                    count_as_review: true,
                    ok: report.ok,
                });
            });
        }
    });

    playbench.on_analyze_all({
        let playbench = playbench.clone();
        let library_page = library_page.clone();
        let tx = tx.clone();
        let path = path.clone();
        let auto_analyzing = auto_analyzing.clone();
        move || {
            if playbench.is_busy() {
                return;
            }
            let jobs = playbench.unanalyzed_jobs();
            if jobs.is_empty() {
                return;
            }
            let watched = playbench
                .current_game_key()
                .filter(|k| jobs.iter().any(|(id, gt, _)| id == &k.0 && gt == &k.1))
                .unwrap_or_else(|| (jobs[0].0.clone(), jobs[0].1.clone()));
            let analyzed_ids: Vec<(String, String)> =
                jobs.iter().map(|(id, gt, _)| (id.clone(), gt.clone())).collect();
            let total = jobs.len();
            playbench.set_busy(true);
            playbench.set_analyzing(Some(watched.clone()));
            library_page.set_analyzing(Some(watched.clone()));
            playbench.set_analyze_progress(&format!("Analyzing game · 0/{total}"));
            auto_analyzing.store(true, Ordering::SeqCst);
            let tx = tx.clone();
            let path = path.clone();
            let auto_analyzing = auto_analyzing.clone();
            std::thread::spawn(move || {
                let tx_prog = tx.clone();
                let report = analyze::analyze_many_and_save(
                    &path,
                    &jobs,
                    18,
                    3,
                    |cur, tot, msg| {
                        let text = analyze_progress_text(cur, tot, msg);
                        let _ = tx_prog.send_blocking(BgMsg::AnalyzeProgress(text));
                    },
                )
                .unwrap_or_else(|e| analyze::AnalyzeReport {
                    ok: false,
                    message: e.to_string(),
                    analyzed: 0,
                    failed: 1,
                });
                auto_analyzing.store(false, Ordering::SeqCst);
                let _ = tx.send_blocking(BgMsg::AnalyzeFinished {
                    analyzed_ids: if report.ok {
                        analyzed_ids
                    } else {
                        // Partial success: mark only completed ones by re-reading is awkward;
                        // leave markers alone and let the next refresh reflect disk.
                        Vec::new()
                    },
                    id: watched.0,
                    game_type: watched.1,
                    message: report.message,
                    blocking: true,
                    count_as_review: false,
                    ok: report.ok,
                });
            });
        }
    });

    playbench.grab_focus();

    window.connect_destroy({
        let _theme = theme;
        move |_| {}
    });

    window.connect_unrealize({
        let _playbench = playbench;
        move |_| {}
    });

    eprintln!("reprise: build_ui total {:.2?} — presenting", t_ui.elapsed());
    window.present();
}

fn run_sync(mut lib: Library, tx: async_channel::Sender<BgMsg>) {
    let tx_early = tx.clone();
    match sync::sync_new_games(&mut lib, |partial| {
        eprintln!(
            "reprise: early sync ready ({} games)",
            partial.games.len().min(sync::EARLY_SYNC_BATCH)
        );
        let _ = tx_early.send_blocking(BgMsg::SyncProgress(partial.clone()));
    }) {
        Ok(report) => {
            eprintln!(
                "reprise: sync {} in {:.2?} (imported={})",
                report.message, report.elapsed, report.imported
            );
            let _ = tx.send_blocking(BgMsg::SyncFinished {
                library: lib,
                report,
            });
        }
        Err(err) => {
            let _ = tx.send_blocking(BgMsg::SyncFinished {
                library: lib,
                report: SyncReport {
                    imported: 0,
                    skipped: 0,
                    fetched: 0,
                    username: String::new(),
                    elapsed: std::time::Duration::ZERO,
                    offline: true,
                    message: format!("Sync skipped: {err}"),
                },
            });
        }
    }
}
