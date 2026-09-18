//! Progress dashboard — Elo + blunder MA charts per time control, top motifs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, DrawingArea, Label, Orientation, Overlay, PolicyType, ScrolledWindow};

use crate::data::{
    analysis_progress_stats, elo_by_time_control, merge_progress_series, AnalysisProgressStats,
    BlunderCategoryCount, Library, TimeControlProgress,
};
use crate::theme::ThemeColors;
use crate::ui::keys_help;

struct StatsLoadMsg {
    generation: u64,
    analysis: Result<AnalysisProgressStats, String>,
}

pub struct ProgressPage {
    pub widget: Overlay,
    content: gtk4::Box,
    charts_box: gtk4::Box,
    series: Rc<RefCell<Vec<TimeControlProgress>>>,
    chart_areas: Rc<RefCell<Vec<DrawingArea>>>,
    errors_box: gtk4::Box,
    errors_status: Label,
    keys_help: gtk4::Box,
    library: Rc<RefCell<Library>>,
    colors: Rc<RefCell<ThemeColors>>,
    on_back: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    load_gen: Rc<Cell<u64>>,
    stats_tx: async_channel::Sender<StatsLoadMsg>,
}

impl ProgressPage {
    pub fn new(library: Rc<RefCell<Library>>, colors: Rc<RefCell<ThemeColors>>) -> Self {
        let overlay = Overlay::new();
        overlay.add_css_class("progress");
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);

        let content = gtk4::Box::new(Orientation::Vertical, 18);
        content.add_css_class("page");
        content.add_css_class("progress-page");
        content.set_margin_top(20);
        content.set_margin_bottom(24);
        content.set_margin_start(28);
        content.set_margin_end(28);
        content.set_hexpand(true);
        content.set_vexpand(true);

        let title = Label::new(Some("Progress"));
        title.add_css_class("title");
        title.set_halign(Align::Start);
        content.append(&title);

        let blurb = Label::new(Some(
            "Rating and blunder rate by time control, plus the motifs analysis keeps finding.",
        ));
        blurb.add_css_class("blurb");
        blurb.set_halign(Align::Start);
        blurb.set_xalign(0.0);
        content.append(&blurb);

        let charts_box = gtk4::Box::new(Orientation::Vertical, 18);
        charts_box.set_halign(Align::Fill);
        content.append(&charts_box);

        let errors_head = Label::new(Some("Top blunder types"));
        errors_head.add_css_class("progress-section");
        errors_head.set_halign(Align::Start);
        content.append(&errors_head);

        let errors_status = Label::new(Some("Loading analysis…"));
        errors_status.add_css_class("muted");
        errors_status.set_halign(Align::Start);
        errors_status.set_xalign(0.0);
        content.append(&errors_status);

        let errors_box = gtk4::Box::new(Orientation::Vertical, 8);
        errors_box.add_css_class("error-types");
        errors_box.set_halign(Align::Fill);
        content.append(&errors_box);

        let hint = Label::new(Some("Esc back · ? keys"));
        hint.add_css_class("hint");
        hint.set_halign(Align::Start);
        content.append(&hint);

        let scroll = ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .build();

        let keys_help = keys_help::build_overlay_child("Progress", keys_help::PROGRESS);
        overlay.set_child(Some(&scroll));
        overlay.add_overlay(&keys_help);

        let series: Rc<RefCell<Vec<TimeControlProgress>>> = Rc::new(RefCell::new(Vec::new()));
        let chart_areas: Rc<RefCell<Vec<DrawingArea>>> = Rc::new(RefCell::new(Vec::new()));

        let on_back: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let load_gen = Rc::new(Cell::new(0u64));
        let (stats_tx, stats_rx) = async_channel::unbounded::<StatsLoadMsg>();

        let page = Self {
            widget: overlay,
            content,
            charts_box,
            series,
            chart_areas,
            errors_box,
            errors_status,
            keys_help,
            library,
            colors,
            on_back,
            load_gen,
            stats_tx,
        };
        page.attach_keys();
        page.attach_stats_loader(stats_rx);
        page.refresh();
        page
    }

    fn attach_keys(&self) {
        let keys = gtk4::EventControllerKey::new();
        keys.connect_key_pressed({
            let keys_help = self.keys_help.clone();
            let content = self.content.clone();
            let on_back = self.on_back.clone();
            let overlay = self.widget.clone();
            move |_, key, _, _| {
                use gtk4::gdk::Key;
                if keys_help.is_visible() {
                    if key == Key::Escape || key == Key::question {
                        keys_help.set_visible(false);
                        content.set_sensitive(true);
                        overlay.grab_focus();
                    }
                    return glib::Propagation::Stop;
                }
                match key {
                    Key::question => {
                        keys_help.set_visible(true);
                        content.set_sensitive(false);
                        keys_help.grab_focus();
                        glib::Propagation::Stop
                    }
                    Key::Escape => {
                        if let Some(f) = on_back.borrow().clone() {
                            f();
                        }
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            }
        });
        self.widget.add_controller(keys);
        self.widget.set_focusable(true);
    }

    fn attach_stats_loader(&self, rx: async_channel::Receiver<StatsLoadMsg>) {
        let load_gen = self.load_gen.clone();
        let errors_box = self.errors_box.clone();
        let errors_status = self.errors_status.clone();
        let series = self.series.clone();
        let charts_box = self.charts_box.clone();
        let chart_areas = self.chart_areas.clone();
        let colors = self.colors.clone();
        let library = self.library.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                if msg.generation != load_gen.get() {
                    continue;
                }
                clear_box(&errors_box);
                match msg.analysis {
                    Ok(stats) => {
                        let elo = elo_by_time_control(&library.borrow());
                        let merged = merge_progress_series(elo, stats.by_time_control);
                        rebuild_charts(
                            &charts_box,
                            &series,
                            &chart_areas,
                            &merged,
                            &colors,
                        );
                        render_categories(&errors_box, &errors_status, &stats.categories);
                    }
                    Err(err) => {
                        // Keep Elo-only charts from the last refresh; clear blunder overlays.
                        {
                            let mut series = series.borrow_mut();
                            for s in series.iter_mut() {
                                s.blunder_ma.clear();
                            }
                        }
                        for area in chart_areas.borrow().iter() {
                            area.queue_draw();
                        }
                        errors_status.set_text(&format!("Could not load blunder stats: {err}"));
                        errors_status.set_visible(true);
                    }
                }
            }
        });
    }

    pub fn refresh(&self) {
        let elo = elo_by_time_control(&self.library.borrow());
        rebuild_charts(
            &self.charts_box,
            &self.series,
            &self.chart_areas,
            &elo,
            &self.colors,
        );

        let generation = self.load_gen.get().wrapping_add(1);
        self.load_gen.set(generation);
        self.errors_status.set_text("Loading analysis…");
        self.errors_status.set_visible(true);
        clear_box(&self.errors_box);

        let path = self.library.borrow().path.clone();
        let tx = self.stats_tx.clone();
        std::thread::spawn(move || {
            let analysis = analysis_progress_stats(&path, 5, 3).map_err(|e| e.to_string());
            let _ = tx.send_blocking(StatsLoadMsg {
                generation,
                analysis,
            });
        });
    }

    pub fn on_back<F: Fn() + 'static>(&self, f: F) {
        *self.on_back.borrow_mut() = Some(Rc::new(f));
    }

    pub fn grab_focus(&self) {
        self.widget.grab_focus();
    }

    pub fn board_repaint_callback(&self) -> Rc<dyn Fn()> {
        let chart_areas = self.chart_areas.clone();
        Rc::new(move || {
            for area in chart_areas.borrow().iter() {
                area.queue_draw();
            }
        })
    }
}

fn rebuild_charts(
    charts_box: &gtk4::Box,
    series_slot: &Rc<RefCell<Vec<TimeControlProgress>>>,
    chart_areas: &Rc<RefCell<Vec<DrawingArea>>>,
    series: &[TimeControlProgress],
    colors: &Rc<RefCell<ThemeColors>>,
) {
    clear_box(charts_box);
    chart_areas.borrow_mut().clear();

    // Chart every time control that has at least one rated/analyzed game.
    let series: Vec<TimeControlProgress> = series
        .iter()
        .filter(|s| !s.elo.is_empty() || !s.blunder_ma.is_empty())
        .cloned()
        .collect();
    *series_slot.borrow_mut() = series.clone();

    if series.is_empty() {
        let empty = Label::new(Some("No rated games yet"));
        empty.add_css_class("muted");
        empty.set_halign(Align::Start);
        charts_box.append(&empty);
        return;
    }

    for (idx, tc) in series.iter().enumerate() {
        let head = Label::new(Some(&format!(
            "{} · Elo · blunders (5-game avg)",
            tc.label
        )));
        head.add_css_class("progress-section");
        head.set_halign(Align::Start);
        charts_box.append(&head);

        let area = DrawingArea::new();
        area.add_css_class("elo-chart");
        area.set_content_width(720);
        area.set_content_height(240);
        area.set_hexpand(true);
        area.set_vexpand(false);
        area.set_size_request(480, 220);

        {
            let series_slot = series_slot.clone();
            let colors = colors.clone();
            area.set_draw_func(move |_area, cr, w, h| {
                let series = series_slot.borrow();
                let Some(tc) = series.get(idx) else {
                    return;
                };
                draw_progress_chart(cr, w, h, &tc.elo, &tc.blunder_ma, &colors.borrow());
            });
        }
        charts_box.append(&area);
        chart_areas.borrow_mut().push(area);
    }
}

fn render_categories(
    errors_box: &gtk4::Box,
    errors_status: &Label,
    rows: &[BlunderCategoryCount],
) {
    if rows.is_empty() {
        errors_status.set_text("No blunders classified yet — analyze some games.");
        errors_status.set_visible(true);
        return;
    }
    errors_status.set_visible(false);
    for (i, row) in rows.iter().enumerate() {
        let line = Label::new(Some(&format!("{}. {} — {}", i + 1, row.label, row.count)));
        line.add_css_class("error-type-row");
        line.set_halign(Align::Start);
        line.set_xalign(0.0);
        errors_box.append(&line);
    }
}

fn clear_box(box_: &gtk4::Box) {
    while let Some(child) = box_.first_child() {
        box_.remove(&child);
    }
}

const BLUNDER_Y_MAX: f64 = 10.0;

fn draw_progress_chart(
    cr: &gtk4::cairo::Context,
    w: i32,
    h: i32,
    elo: &[crate::data::EloPoint],
    blunders: &[crate::data::BlunderMaPoint],
    colors: &ThemeColors,
) {
    let width = w as f64;
    let height = h as f64;
    let (bg_r, bg_g, bg_b) = ThemeColors::rgb(&colors.dark_background);
    cr.set_source_rgb(bg_r, bg_g, bg_b);
    cr.rectangle(0.0, 0.0, width, height);
    let _ = cr.fill();

    let pad_l = 52.0;
    let pad_r = 44.0;
    let pad_t = 22.0;
    let pad_b = 36.0;
    let plot_w = (width - pad_l - pad_r).max(1.0);
    let plot_h = (height - pad_t - pad_b).max(1.0);

    let (ax_r, ax_g, ax_b) = ThemeColors::rgb(&colors.dark_foreground);
    cr.set_source_rgb(ax_r, ax_g, ax_b);
    cr.set_line_width(1.0);
    cr.move_to(pad_l, pad_t);
    cr.line_to(pad_l, pad_t + plot_h);
    cr.line_to(pad_l + plot_w, pad_t + plot_h);
    cr.line_to(pad_l + plot_w, pad_t);
    let _ = cr.stroke();

    cr.select_font_face(
        "Sans",
        gtk4::cairo::FontSlant::Normal,
        gtk4::cairo::FontWeight::Normal,
    );
    cr.set_font_size(11.0);

    if elo.is_empty() && blunders.is_empty() {
        let (fg_r, fg_g, fg_b) = ThemeColors::rgb(&colors.light_foreground);
        cr.set_source_rgb(fg_r, fg_g, fg_b);
        cr.set_font_size(13.0);
        let _ = cr.move_to(pad_l + 12.0, pad_t + plot_h * 0.5);
        let _ = cr.show_text("No rated / analyzed games yet");
        return;
    }

    // Shared time domain across both series.
    let t_min = elo
        .iter()
        .map(|p| p.at.timestamp())
        .chain(blunders.iter().map(|p| p.at.timestamp()))
        .min()
        .unwrap_or(0);
    let t_max = elo
        .iter()
        .map(|p| p.at.timestamp())
        .chain(blunders.iter().map(|p| p.at.timestamp()))
        .max()
        .unwrap_or(t_min + 1);
    let t0 = t_min as f64;
    let t_span = ((t_max - t_min) as f64).max(1.0);
    let x_at = |t: i64| {
        if t_max == t_min {
            pad_l + plot_w * 0.5
        } else {
            pad_l + ((t as f64 - t0) / t_span) * plot_w
        }
    };

    // Left axis: Elo
    let (tick_r, tick_g, tick_b) = ThemeColors::rgb(&colors.dark_foreground);
    if !elo.is_empty() {
        let min_elo = elo.iter().map(|p| p.elo).min().unwrap_or(0);
        let max_elo = elo.iter().map(|p| p.elo).max().unwrap_or(0);
        let elo_span = (max_elo - min_elo).max(40) as f64;
        let elo_lo = (min_elo as f64) - elo_span * 0.08;
        let elo_hi = (max_elo as f64) + elo_span * 0.08;
        let elo_range = (elo_hi - elo_lo).max(1.0);
        let y_elo = |rating: i32| pad_t + plot_h - ((rating as f64 - elo_lo) / elo_range) * plot_h;

        for frac in [0.0_f64, 0.5, 1.0] {
            let rating = (elo_lo + elo_range * (1.0 - frac)) as i32;
            let y = pad_t + plot_h * frac;
            cr.set_source_rgba(tick_r, tick_g, tick_b, 0.28);
            cr.move_to(pad_l, y);
            cr.line_to(pad_l + plot_w, y);
            let _ = cr.stroke();
            cr.set_source_rgb(tick_r, tick_g, tick_b);
            let _ = cr.move_to(8.0, y + 4.0);
            let _ = cr.show_text(&format!("{rating}"));
        }

        let (line_r, line_g, line_b) = ThemeColors::rgb(&colors.accent);
        cr.set_source_rgb(line_r, line_g, line_b);
        cr.set_line_width(2.0);
        if elo.len() >= 2 {
            for (i, p) in elo.iter().enumerate() {
                let x = x_at(p.at.timestamp());
                let y = y_elo(p.elo);
                if i == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
            let _ = cr.stroke();
        }

        if let Some(last) = elo.last() {
            let x = x_at(last.at.timestamp());
            let y = y_elo(last.elo);
            cr.arc(x, y, 3.5, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            cr.set_font_size(12.0);
            let _ = cr.move_to((x + 8.0).min(width - pad_r - 36.0), y - 6.0);
            let _ = cr.show_text(&format!("{}", last.elo));
        }
    }

    // Right axis: blunders / game (fixed 0..10)
    let y_bl = |avg: f64| {
        let clipped = avg.clamp(0.0, BLUNDER_Y_MAX);
        pad_t + plot_h - (clipped / BLUNDER_Y_MAX) * plot_h
    };
    cr.set_font_size(11.0);
    let (br_r, br_g, br_b) = ThemeColors::rgb(&colors.red);
    for (frac, label) in [(0.0_f64, "10"), (0.5, "5"), (1.0, "0")] {
        let y = pad_t + plot_h * frac;
        cr.set_source_rgb(br_r, br_g, br_b);
        let _ = cr.move_to(pad_l + plot_w + 8.0, y + 4.0);
        let _ = cr.show_text(label);
    }

    if !blunders.is_empty() {
        cr.set_source_rgb(br_r, br_g, br_b);
        cr.set_line_width(2.0);
        if blunders.len() >= 2 {
            for (i, p) in blunders.iter().enumerate() {
                let x = x_at(p.at.timestamp());
                let y = y_bl(p.avg);
                if i == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
            let _ = cr.stroke();
        }

        if let Some(last) = blunders.last() {
            let x = x_at(last.at.timestamp());
            let y = y_bl(last.avg);
            cr.arc(x, y, 3.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            cr.set_font_size(12.0);
            let _ = cr.move_to((x - 36.0).max(pad_l), y - 8.0);
            let _ = cr.show_text(&format!("{:.1}", last.avg));
        }
    }

    // Legend
    cr.set_font_size(11.0);
    let (acc_r, acc_g, acc_b) = ThemeColors::rgb(&colors.accent);
    cr.set_source_rgb(acc_r, acc_g, acc_b);
    cr.rectangle(pad_l, 6.0, 10.0, 3.0);
    let _ = cr.fill();
    let (fg_r, fg_g, fg_b) = ThemeColors::rgb(&colors.light_foreground);
    cr.set_source_rgb(fg_r, fg_g, fg_b);
    let _ = cr.move_to(pad_l + 14.0, 12.0);
    let _ = cr.show_text("Elo");

    cr.set_source_rgb(br_r, br_g, br_b);
    cr.rectangle(pad_l + 48.0, 6.0, 10.0, 3.0);
    let _ = cr.fill();
    cr.set_source_rgb(fg_r, fg_g, fg_b);
    let _ = cr.move_to(pad_l + 62.0, 12.0);
    let _ = cr.show_text("Blunders");

    // X date labels from whichever series has points
    let date_src: Vec<(i64, String)> = if elo.len() >= 2 {
        let mid = elo.len() / 2;
        [0usize, mid, elo.len() - 1]
            .into_iter()
            .map(|i| {
                let p = &elo[i];
                (p.at.timestamp(), p.at.format("%d %b").to_string())
            })
            .collect()
    } else if elo.len() == 1 {
        let p = &elo[0];
        vec![(p.at.timestamp(), p.at.format("%d %b").to_string())]
    } else if blunders.len() >= 2 {
        let mid = blunders.len() / 2;
        [0usize, mid, blunders.len() - 1]
            .into_iter()
            .map(|i| {
                let p = &blunders[i];
                (p.at.timestamp(), p.at.format("%d %b").to_string())
            })
            .collect()
    } else if blunders.len() == 1 {
        let p = &blunders[0];
        vec![(p.at.timestamp(), p.at.format("%d %b").to_string())]
    } else {
        Vec::new()
    };
    cr.set_source_rgb(tick_r, tick_g, tick_b);
    cr.set_font_size(11.0);
    for (i, (ts, label)) in date_src.iter().enumerate() {
        let x = x_at(*ts);
        let offset = match i {
            0 => 0.0,
            n if n + 1 == date_src.len() => -40.0,
            _ => -20.0,
        };
        let _ = cr.move_to(x + offset, pad_t + plot_h + 22.0);
        let _ = cr.show_text(label);
    }
}
