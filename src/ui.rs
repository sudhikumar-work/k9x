use crate::app::{App, InputPurpose, Mode, ViewKind};
use crate::cfg::Theme;
use crate::model::{ColSrc, KindSpec, Sev};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row as TRow, Table},
};

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let big = !app.ui_headless && area.height >= 16 && area.width >= 90;

    let chunks = if big {
        Layout::vertical([
            Constraint::Length(8), // 1 row top padding + logo / cluster info / shortcuts
            Constraint::Min(3),
            Constraint::Length(app_status_h(app)),
        ])
        .split(area)
    } else if app.ui_headless {
        Layout::vertical([Constraint::Min(3), Constraint::Length(app_status_h(app))]).split(area)
    } else {
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(app_status_h(app)),
        ])
        .split(area)
    };

    if big {
        if area.width >= 128 && !app.ui_logoless && !app.ui_crumbsless {
            // middle panel width follows its actual text, then the leftover is
            // split EVENLY between the two gutters — logo/info/hints all get
            // the same horizontal breathing room
            let info_rows = build_info_rows(app);
            let iw = info_rows
                .iter()
                .map(|l| l.width() as u16)
                .max()
                .unwrap_or(58)
                .clamp(58, 68);
            let logo_w: u16 = LOGO_WIDTH + 2;
            let cols = Layout::horizontal([
                Constraint::Length(logo_w), // Logo (33)
                Constraint::Length(2),      // Gutter
                Constraint::Length(iw),     // Info block (58-68)
                Constraint::Length(2),      // Gutter
                Constraint::Min(30),        // Shortcuts grid
            ])
            .split(chunks[0]);

            draw_logo(f, app, cols[0]);
            draw_info_block(f, app, cols[2]);
            draw_shortcuts_grid(f, app, cols[4]);
        } else if app.ui_logoless || app.ui_crumbsless {
            // logo and/or hints hidden: info block takes the full top strip
            let cols = Layout::horizontal([Constraint::Min(
                build_info_rows(app)
                    .iter()
                    .map(|l| l.width() as u16)
                    .max()
                    .unwrap_or(46)
                    + 2,
            )])
            .split(chunks[0]);
            draw_info_block(f, app, cols[0]);
        } else {
            let cols =
                Layout::horizontal([Constraint::Length(LOGO_WIDTH + 2), Constraint::Min(35)])
                    .split(chunks[0]);

            draw_logo(f, app, cols[0]);

            let right = Layout::vertical([
                Constraint::Length(1), // info line
                Constraint::Min(5),    // shortcuts
            ])
            .split(cols[1]);

            draw_info_line(f, app, right[0]);
            draw_shortcuts_grid(f, app, right[1]);
        }
    } else if app.ui_headless {
        // no chrome at all
    } else {
        draw_topbar(f, app, chunks[0]);
        if !app.ui_crumbsless {
            draw_hints(f, app, chunks[1]);
        }
    }

    let mut body_chunk = match (big, app.ui_headless) {
        (true, _) => chunks[1],
        (_, true) => chunks[0],
        _ => chunks[2],
    };
    let status_chunk = if big {
        chunks[2]
    } else if app.ui_headless {
        chunks[1]
    } else {
        chunks[3]
    };

    // the command/filter bar gets its own reserved strip so it never covers the view
    let mut input_rect: Option<Rect> = None;
    if matches!(app.mode, Mode::Cmd { .. } | Mode::Filter { .. }) && body_chunk.height >= 6 {
        let h = 3u16.min(body_chunk.height / 2);
        let parts = Layout::vertical([Constraint::Min(3), Constraint::Length(h)]).split(body_chunk);
        body_chunk = parts[0];
        input_rect = Some(parts[1]);
    }

    app.ui_body = Some(body_chunk);

    match (&app.view, &app.mode) {
        (_, Mode::Logs(st)) => draw_logs(f, app, st, body_chunk),
        (_, Mode::LogExport { logs_state, .. }) => draw_logs(f, app, logs_state, body_chunk),
        (
            _,
            Mode::Confirm {
                action: crate::app::Action::SaveLogs { logs_state, .. },
                ..
            },
        ) => draw_logs(f, app, logs_state, body_chunk),
        (_, Mode::Exec(_)) => draw_exec(f, app, body_chunk),
        (_, Mode::TextPane { .. }) => draw_text_pane(f, app, body_chunk),
        (ViewKind::Table, _) => draw_table(f, app, body_chunk),
        (ViewKind::Pulse, _) => draw_pulse(f, app, body_chunk),
        (ViewKind::Pf, _) => draw_pf(f, app, body_chunk),
    }

    // overlays
    match &app.mode {
        Mode::Menu(_) => draw_menu(f, app),
        Mode::Confirm { prompt, .. } => draw_confirm(f, app, prompt),
        Mode::Notice { title, lines, .. } => draw_notice(f, app, title, lines),
        Mode::ThemeEditor {
            values,
            sel,
            editing,
            buf,
        } => draw_theme_editor(f, app, values, *sel, *editing, buf),
        Mode::LogExport {
            dir_buf,
            file_buf,
            focus,
            suggestions,
            sug_idx,
            sug_scroll,
            ..
        } => draw_log_export(
            f,
            app,
            dir_buf,
            file_buf,
            *focus,
            suggestions,
            *sug_idx,
            *sug_scroll,
        ),
        Mode::PortForward(st) => draw_port_forward(f, app, st),
        Mode::Input { buf, purpose } => draw_input(f, &app.theme, buf, purpose_label(purpose)),
        Mode::Cmd { buf, .. } | Mode::Filter { buf } => {
            draw_input_line(f, app, buf, input_rect.unwrap_or(body_chunk))
        }
        _ => {}
    }

    // record clickable geometry for mouse support on modals
    match &app.mode {
        Mode::Confirm { .. } => {
            let r = centered(64, 8, area);
            let btn = Rect {
                x: r.x + 1,
                y: r.y + 4,
                width: r.width - 2,
                height: 1,
            };
            app.ui_confirm_btn = Some((btn, r.x + r.width / 2));
            app.ui_notice_rect = None;
        }
        Mode::Notice { lines, .. } => {
            let w = 70u16.min(area.width.saturating_sub(4));
            let h = ((lines.len() as u16) + 5)
                .min(area.height.saturating_sub(4))
                .max(6);
            let r = Rect {
                x: area.x + (area.width.saturating_sub(w)) / 2,
                y: area.y + (area.height.saturating_sub(h)) / 2,
                width: w,
                height: h.max(6),
            };
            app.ui_notice_rect = Some(r);
            app.ui_confirm_btn = None;
        }
        Mode::LogExport { .. } => {
            let w = 80u16.min(area.width.saturating_sub(4));
            let r = Rect {
                x: area.x + (area.width.saturating_sub(w)) / 2,
                y: area.y + (area.height.saturating_sub(12)) / 2,
                width: w,
                height: 12,
            };
            app.ui_notice_rect = Some(r);
            app.ui_confirm_btn = None;
        }
        Mode::PortForward(st) => {
            let w = 72u16.min(area.width.saturating_sub(4));
            let extra = if st.ports.len() > 1 {
                st.ports.len().min(4) as u16 + 1
            } else {
                0
            };
            let h = (14 + extra).min(area.height.saturating_sub(2));
            let r = Rect {
                x: area.x + (area.width.saturating_sub(w)) / 2,
                y: area.y + (area.height.saturating_sub(h)) / 2,
                width: w,
                height: h,
            };
            app.ui_notice_rect = Some(r);
            app.ui_confirm_btn = None;
        }
        _ => {
            app.ui_confirm_btn = None;
            app.ui_notice_rect = None;
        }
    }

    if !app.status.is_empty() {
        let style = if app.status.starts_with('!') {
            Style::default().fg(app.theme.bad)
        } else {
            Style::default().fg(app.theme.warn)
        };
        let p = Paragraph::new(Line::from(Span::styled(app.status.clone(), style)));
        f.render_widget(p, status_chunk);
    }
}

fn app_status_h(app: &App) -> u16 {
    if app.status.is_empty() { 0 } else { 1 }
}

fn purpose_label(p: &InputPurpose) -> String {
    match p {
        InputPurpose::Scale { name } => format!("replicas → {name}:"),
        InputPurpose::PfBind { ns, pod, port } => format!("port-forward bind ({ns}/{pod}:{port}):"),
    }
}

const LOGO_LINES: [&str; 6] = [
    "██╗  ██╗    ██████╗    ██╗  ██╗",
    "██║ ██╔╝   ██╔═══██╗   ╚██╗██╔╝",
    "█████╔╝    ╚██████║     ╚███╔╝ ",
    "██╔═██╗     ╚═══██║     ██╔██╗ ",
    "██║  ██╗   ██████╔╝    ██╔╝ ██╗",
    "╚═╝  ╚═╝   ╚═════╝     ╚═╝  ╚═╝",
];
const LOGO_WIDTH: u16 = 31;
const LOGO_HEIGHT: u16 = 6;

type RgbTriplet = (u8, u8, u8);

fn logo_gradient_colors(theme: &Theme) -> (RgbTriplet, RgbTriplet, RgbTriplet) {
    match theme.ok {
        Color::Green | Color::LightGreen | Color::Rgb(0, 230, 60) | Color::Rgb(0, 255, 65) => {
            // Matrix theme cyber green gradient — high luminance, never black
            ((80, 255, 170), (0, 255, 65), (0, 200, 80))
        }
        Color::Rgb(0, 128, 0) => {
            // Light theme blue gradient
            ((0, 160, 240), (0, 100, 190), (0, 50, 130))
        }
        Color::Gray => {
            // Mono theme gray gradient
            ((255, 255, 255), (180, 180, 180), (100, 100, 100))
        }
        _ => {
            // Default cyan/teal gradient
            ((120, 240, 255), (0, 200, 240), (0, 100, 180))
        }
    }
}

fn lerp_rgb(c1: RgbTriplet, c2: RgbTriplet, t: f32) -> RgbTriplet {
    let t = t.clamp(0.0, 1.0);
    (
        (c1.0 as f32 + (c2.0 as f32 - c1.0 as f32) * t) as u8,
        (c1.1 as f32 + (c2.1 as f32 - c1.1 as f32) * t) as u8,
        (c1.2 as f32 + (c2.2 as f32 - c1.2 as f32) * t) as u8,
    )
}

fn logo_char_color(theme: &Theme, char_x: usize, char_y: usize, is_block: bool) -> Style {
    let (start, mid, end) = logo_gradient_colors(theme);
    let factor = (char_x as f32 / 30.0) * 0.65 + (char_y as f32 / 5.0) * 0.35;
    let rgb = if factor < 0.5 {
        lerp_rgb(start, mid, factor * 2.0)
    } else {
        lerp_rgb(mid, end, (factor - 0.5) * 2.0)
    };

    let mut st = Style::default().fg(Color::Rgb(rgb.0, rgb.1, rgb.2));
    if is_block {
        st = st.add_modifier(Modifier::BOLD);
    }
    st
}

fn draw_logo(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let pad_x = (area.width.saturating_sub(LOGO_WIDTH)) / 2;
    let pad_y = (area.height.saturating_sub(LOGO_HEIGHT)) / 2;

    for (row_idx, line) in LOGO_LINES.iter().enumerate() {
        let y = area.y + pad_y + row_idx as u16;
        if y >= area.y + area.height {
            break;
        }

        let mut spans = Vec::with_capacity(32);
        if pad_x > 0 {
            spans.push(Span::raw(" ".repeat(pad_x as usize)));
        }

        for (current_col, (col_idx, ch)) in (pad_x..).zip(line.chars().enumerate()) {
            if current_col >= area.width {
                break;
            }
            if ch == ' ' {
                spans.push(Span::raw(" "));
            } else {
                let is_block = ch == '█';
                let style = logo_char_color(t, col_idx, row_idx, is_block);
                spans.push(Span::styled(ch.to_string(), style));
            }
        }
        f.render_widget(
            Line::from(spans),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
        );
    }
}

fn fmt_gib(bytes: u64) -> String {
    format!("{:.1}G", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
}

/// fixed-width aligned label span (all top-section labels share one width)
fn lbl(s: &str, style: Style) -> Span<'static> {
    Span::styled(format!("{s:<11}"), style)
}

/// severity color by configured warn/crit thresholds
pub fn pct_style(pct: u16, warn: u16, crit: u16, t: &Theme) -> Style {
    if pct >= crit {
        Style::default().fg(t.bad).add_modifier(Modifier::BOLD)
    } else if pct >= warn {
        Style::default().fg(t.warn).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.ok)
    }
}

fn date_style(date: &str, t: &Theme) -> Style {
    if crate::model::k8s_support_expired(date) {
        Style::default().fg(t.bad).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.warn)
    }
}

fn build_info_rows(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let ro = app.ro;
    let ctx = &app.cluster.ctx_name;
    let ns = if app.all_ns { "all" } else { app.ns.as_str() };
    let view = match (&app.view, &app.mode) {
        (_, Mode::Logs(st)) => {
            format!("logs:{}/{}", st.pod, st.container.as_deref().unwrap_or("*"))
        }
        (_, Mode::Exec(ex)) => format!("exec:{}", ex.pod),
        (_, Mode::TextPane { title, .. }) => title.clone(),
        (ViewKind::Table, _) => match &app.view_spec {
            Some(s) => {
                if let Some(dt) = &app.drill_title {
                    format!("{dt} → pods")
                } else {
                    s.plural.clone()
                }
            }
            None => String::new(),
        },
        (ViewKind::Pulse, _) => "pulse".into(),
        (ViewKind::Pf, _) => "port-forwards".into(),
    };
    let count = if matches!(app.view, ViewKind::Table) && app.view_spec.is_some() {
        format!(" ({})", app.filtered_sorted().len())
    } else {
        String::new()
    };

    let label_style = Style::default().fg(t.warn).add_modifier(Modifier::BOLD);
    let val_style = Style::default().fg(t.info).add_modifier(Modifier::BOLD);
    let sub_lbl_style = Style::default().fg(Color::Gray);

    // k8s server version
    let k8s_line = Line::from(match &app.k8s_version {
        Some(ver) => vec![
            lbl("K8s:", label_style),
            Span::styled(ver.clone(), val_style),
        ],
        None => vec![lbl("K8s:", label_style), Span::styled("…", sub_lbl_style)],
    });

    // support windows: AWS/EKS data when available, upstream estimates otherwise (~)
    let sup_line: Line = match &app.sup_dates {
        Some(sd) => Line::from(vec![
            lbl("Support:", label_style),
            Span::styled(format!("std {}", sd.standard), date_style(&sd.standard, t)),
            Span::styled(" \u{00b7} ", sub_lbl_style),
            Span::styled(
                if sd.estimated {
                    format!("ext ~{}", sd.extended)
                } else {
                    format!("ext {}", sd.extended)
                },
                date_style(&sd.extended, t),
            ),
        ]),
        None => Line::from(vec![
            lbl("Support:", label_style),
            Span::styled("-", sub_lbl_style),
        ]),
    };

    // cluster-wide resource usage (metrics-server + node allocatable)
    let res_line: Line = match &app.cluster_res {
        Some(r) => {
            let pct_of = |used: f64, cap: f64| -> String {
                if cap > 0.0 {
                    format!(" ({}%)", (used / cap * 100.0).round() as u64)
                } else {
                    String::new()
                }
            };
            let cpu_raw = r.cpu_used_m;
            let mem_raw = r.mem_used.map(|b| b as f64);
            let mut spans = vec![lbl("Load:", label_style)];
            spans.push(Span::styled("cpu ", sub_lbl_style));
            match (cpu_raw, r.cpu_cap_m > 0.0) {
                (Some(u), true) => {
                    spans.push(Span::styled(
                        format!("{:.0}m/{}m", u, r.cpu_cap_m as u64),
                        val_style,
                    ));
                    spans.push(Span::styled(
                        pct_of(u, r.cpu_cap_m),
                        pct_style(
                            ((u / r.cpu_cap_m) * 100.0) as u16,
                            app.thresholds.0,
                            app.thresholds.1,
                            t,
                        ),
                    ));
                }
                (Some(u), false) => spans.push(Span::styled(format!("{:.0}m", u), val_style)),
                (None, true) => spans.push(Span::styled(
                    format!("n/a/{}m", r.cpu_cap_m as u64),
                    val_style,
                )),
                _ => spans.push(Span::styled("n/a", val_style)),
            }
            spans.push(Span::styled(" \u{00b7} ", sub_lbl_style));
            spans.push(Span::styled("mem ", sub_lbl_style));
            match (mem_raw, r.mem_cap > 0) {
                (Some(u), true) => {
                    spans.push(Span::styled(
                        format!("{}/{}", fmt_gib(u as u64), fmt_gib(r.mem_cap)),
                        val_style,
                    ));
                    spans.push(Span::styled(
                        pct_of(u, r.mem_cap as f64),
                        pct_style(
                            ((u / r.mem_cap as f64) * 100.0) as u16,
                            app.thresholds.2,
                            app.thresholds.3,
                            t,
                        ),
                    ));
                }
                (Some(u), false) => spans.push(Span::styled(fmt_gib(u as u64), val_style)),
                (None, true) => spans.push(Span::styled(
                    format!("n/a/{}", fmt_gib(r.mem_cap)),
                    val_style,
                )),
                _ => spans.push(Span::styled("n/a", val_style)),
            }
            Line::from(spans)
        }
        None => Line::from(vec![
            lbl("Load:", label_style),
            Span::styled("\u{2026}", t.dim),
        ]),
    };

    vec![
        Line::from(vec![
            lbl("Context:", label_style),
            Span::styled(ctx.clone(), val_style),
            Span::raw(" "),
            if ro {
                Span::styled(
                    "[RO]",
                    Style::default().fg(t.bad).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(
                    "[RW]",
                    Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
                )
            },
        ]),
        Line::from(vec![
            lbl("Namespace:", label_style),
            Span::styled(
                ns.to_string(),
                Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            lbl("View:", label_style),
            Span::styled(
                format!("{view}{count}"),
                Style::default().fg(t.title).add_modifier(Modifier::BOLD),
            ),
        ]),
        k8s_line,
        sup_line,
        res_line,
        Line::from(vec![
            lbl("k9x", label_style),
            Span::styled(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(t.dim).add_modifier(Modifier::BOLD),
            ),
        ]),
    ]
}

fn draw_info_block(f: &mut Frame, app: &App, area: Rect) {
    let rows = build_info_rows(app);
    let rows_n = rows.len() as u16;
    // one blank row above the block (top padding) when the panel is tall enough
    let y0 = if area.height > rows_n { 1 } else { 0 };
    for (i, row) in rows.into_iter().enumerate() {
        let y = area.y + y0 + i as u16;
        if y < area.y + area.height {
            f.render_widget(
                row,
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
        }
    }
}

fn draw_info_line(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let ro = app.ro;
    let ctx = &app.cluster.ctx_name;
    let ns = if app.all_ns { "all" } else { app.ns.as_str() };
    let view = match (&app.view, &app.mode) {
        (_, Mode::Logs(st)) => {
            format!("logs:{}/{}", st.pod, st.container.as_deref().unwrap_or("*"))
        }
        (_, Mode::Exec(ex)) => format!("exec:{}", ex.pod),
        (_, Mode::TextPane { title, .. }) => title.clone(),
        (ViewKind::Table, _) => match &app.view_spec {
            Some(s) => s.plural.clone(),
            None => String::new(),
        },
        (ViewKind::Pulse, _) => "pulse".into(),
        (ViewKind::Pf, _) => "port-forwards".into(),
    };
    let count = if matches!(app.view, ViewKind::Table) && app.view_spec.is_some() {
        format!(" ({})", app.filtered_sorted().len())
    } else {
        String::new()
    };
    let mut spans = vec![
        Span::styled("\u{2590}", Style::default().fg(t.accent)),
        Span::styled(
            "k9",
            Style::default()
                .fg(Color::White)
                .bg(t.info)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "x",
            Style::default()
                .fg(Color::Black)
                .bg(t.ok)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{258c}", Style::default().fg(t.accent)),
        Span::raw("  "),
        Span::styled("ctx ", Style::default().fg(t.dim)),
        Span::styled(
            ctx.clone(),
            Style::default().fg(t.info).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  \u{2502}  ", Style::default().fg(t.dim)),
        Span::styled("ns ", Style::default().fg(t.dim)),
        Span::styled(
            ns.to_string(),
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  \u{2502}  ", Style::default().fg(t.dim)),
        Span::styled(
            view + &count,
            Style::default().fg(t.title).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(ver) = &app.k8s_version {
        let mut k8s_spans = vec![
            Span::styled("  \u{2502}  ", Style::default().fg(t.dim)),
            Span::styled(
                ver.clone(),
                Style::default().fg(t.info).add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(sd) = &app.sup_dates {
            k8s_spans.push(Span::raw(" "));
            k8s_spans.push(Span::styled(
                format!("std\u{2192}{}", sd.standard),
                date_style(&sd.standard, t),
            ));
            k8s_spans.push(Span::styled(" \u{00b7} ", Style::default().fg(t.dim)));
            let ext_txt = if sd.estimated {
                format!("ext\u{2192}~{}", sd.extended)
            } else {
                format!("ext\u{2192}{}", sd.extended)
            };
            k8s_spans.push(Span::styled(ext_txt, date_style(&sd.extended, t)));
        }
        spans.extend(k8s_spans);
    }
    if let Some(r) = &app.cluster_res {
        spans.push(Span::styled("  \u{2502}  ", Style::default().fg(t.dim)));
        spans.push(Span::styled("res ", Style::default().fg(t.dim)));
        let dim_st = Style::default().fg(t.dim);
        match r.cpu_used_m {
            Some(u) => {
                spans.push(Span::styled(
                    format!("{:.0}m/{}m", u, r.cpu_cap_m as u64),
                    Style::default().fg(t.info).add_modifier(Modifier::BOLD),
                ));
                if r.cpu_cap_m > 0.0 {
                    spans.push(Span::styled(
                        format!(" ({}%)", (u / r.cpu_cap_m * 100.0).round() as u64),
                        dim_st,
                    ));
                }
            }
            None => spans.push(Span::styled(
                format!("n/a/{}m", r.cpu_cap_m as u64),
                Style::default().fg(t.info).add_modifier(Modifier::BOLD),
            )),
        }
        spans.push(Span::raw(" \u{00b7} "));
        match r.mem_used {
            Some(u) => {
                spans.push(Span::styled(
                    format!("{}/{}", fmt_gib(u), fmt_gib(r.mem_cap)),
                    Style::default().fg(t.info).add_modifier(Modifier::BOLD),
                ));
                if r.mem_cap > 0 {
                    spans.push(Span::styled(
                        format!(
                            " ({}%)",
                            ((u as f64 / r.mem_cap as f64) * 100.0).round() as u64
                        ),
                        dim_st,
                    ));
                }
            }
            None => spans.push(Span::styled(
                format!("n/a/{}", fmt_gib(r.mem_cap)),
                Style::default().fg(t.info).add_modifier(Modifier::BOLD),
            )),
        }
    }
    if ro {
        spans.push(Span::styled(
            "  \u{2502}  ".to_string(),
            Style::default().fg(t.dim),
        ));
        spans.push(Span::styled(
            "\u{1f512} read-only",
            Style::default().fg(t.bad).add_modifier(Modifier::BOLD),
        ));
    }
    f.render_widget(Line::from(spans), area);
}

fn draw_topbar(f: &mut Frame, app: &App, area: Rect) {
    let ro = if app.ro { " \u{2502} \u{1f512}ro" } else { "" };
    let ctx = &app.cluster.ctx_name;
    let ns = if app.all_ns { "all" } else { app.ns.as_str() };
    let view = match (&app.view, &app.mode) {
        (_, Mode::Logs(st)) => {
            format!("logs:{}/{}", st.pod, st.container.as_deref().unwrap_or("*"))
        }
        (_, Mode::Exec(ex)) => format!("exec:{}", ex.pod),
        (_, Mode::TextPane { title, .. }) => title.clone(),
        (ViewKind::Table, _) => match &app.view_spec {
            Some(s) => s.plural.clone(),
            None => "k9x".into(),
        },
        (ViewKind::Pulse, _) => "pulse".into(),
        (ViewKind::Pf, _) => "port-forwards".into(),
    };
    let count = if matches!(app.view, ViewKind::Table) && app.view_spec.is_some() {
        format!(" ({})", app.filtered_sorted().len())
    } else {
        String::new()
    };
    let t = &app.theme;
    // --- logo: two-tone chip + accent mark ---
    let logo = vec![
        Span::styled("\u{2590}", Style::default().fg(t.accent)),
        Span::styled(
            "k9",
            Style::default()
                .fg(Color::White)
                .bg(t.info)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "x",
            Style::default()
                .fg(Color::Black)
                .bg(t.ok)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{258c}", Style::default().fg(t.accent)),
        Span::raw(" "),
    ];
    let mut spans = vec![];
    spans.extend(logo);
    spans.push(Span::styled("ctx ", Style::default().fg(t.dim)));
    spans.push(Span::styled(
        ctx.clone(),
        Style::default().fg(t.info).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled("  \u{2502}  ", Style::default().fg(t.dim)));
    spans.push(Span::styled("ns ", Style::default().fg(t.dim)));
    spans.push(Span::styled(
        ns.to_string(),
        Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled("  \u{2502}  ", Style::default().fg(t.dim)));
    spans.push(Span::styled(
        view + &count,
        Style::default().fg(t.title).add_modifier(Modifier::BOLD),
    ));
    if !ro.is_empty() {
        spans.push(Span::styled(
            "  \u{2502}  ".to_string(),
            Style::default().fg(t.dim),
        ));
        spans.push(Span::styled(
            "\u{1f512} read-only",
            Style::default().fg(t.bad).add_modifier(Modifier::BOLD),
        ));
    }
    f.render_widget(Line::from(spans), area);
}

fn sev_style(t: &Theme, s: &Sev) -> Style {
    match s {
        Sev::Ok => Style::default().fg(t.ok),
        Sev::Warn => Style::default().fg(t.warn),
        Sev::Bad => Style::default().fg(t.bad),
        Sev::Info => Style::default(),
    }
}

fn draw_table(f: &mut Frame, app: &mut App, area: Rect) {
    let spec = match &app.view_spec {
        Some(s) => s.clone(),
        None => return,
    };
    if app.sel_key.is_none() {
        app.sel_top();
    }
    let rows = app.filtered_sorted();
    let widths: Vec<Constraint> = spec
        .cols
        .iter()
        .map(|c| {
            let total: u16 = spec.cols.iter().map(|x| x.weight).sum();
            Constraint::Percentage(((c.weight * 100) / total.max(1)).max(4))
        })
        .collect();
    let header = TRow::new(
        spec.cols
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let arrow = if i == app.sort_col {
                    if app.sort_desc { " ↓" } else { " ↑" }
                } else {
                    ""
                };
                Cell::from(format!("{}{arrow}", c.name)).style(
                    Style::default()
                        .fg(app.theme.header)
                        .add_modifier(Modifier::BOLD),
                )
            })
            .collect::<Vec<_>>(),
    );
    let sel = app.sel_key.clone();
    let keys_in_order: Vec<String> = rows.iter().map(|r| r.key.clone()).collect();
    let body = rows
        .into_iter()
        .map(|r| {
            // whole row takes the severity color: red = failing, orange = degraded/restarts
            let mut sev = sev_style(&app.theme, &r.sev);
            if app.marks.contains(&r.key) {
                // old-school inverse video for marked rows
                sev = sev.add_modifier(Modifier::REVERSED);
            }
            TRow::new(
                r.cells
                    .iter()
                    .enumerate()
                    .map(|(i, cell)| {
                        let is_node_pct = spec.kind == "Node"
                            && matches!(
                                spec.cols.get(i).map(|c| &c.src),
                                Some(ColSrc::NodeCpuPct) | Some(ColSrc::NodeMemPct)
                            );
                        let base = if is_node_pct && cell != "-" {
                            let p: u16 = cell.trim_end_matches('%').parse().unwrap_or(0);
                            let (w, cc) = match spec.cols.get(i).map(|c| c.name) {
                                Some("CPU%") => (app.thresholds.0, app.thresholds.1),
                                _ => (app.thresholds.2, app.thresholds.3),
                            };
                            pct_style(p, w, cc, &app.theme)
                        } else if i == status_col(&spec) {
                            sev.add_modifier(Modifier::BOLD)
                        } else {
                            sev
                        };
                        Cell::from(cell.clone()).style(base)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    let title = format!(" {} ", table_title(app, &spec));
    let table = Table::new(body, widths)
        .header(header)
        .block(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.dim))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(app.theme.title)
                        .add_modifier(Modifier::BOLD),
                )),
        )
        .row_highlight_style(
            Style::default()
                .bg(app.theme.bg_sel)
                .add_modifier(Modifier::BOLD),
        )
        .column_spacing(1);
    let idx = sel
        .as_deref()
        .and_then(|k| keys_in_order.iter().position(|x| x == k));
    let mut ts = ratatui::widgets::TableState::default()
        .with_selected(idx)
        .with_offset(app.ui_toffset);
    f.render_stateful_widget(table, area, &mut ts);
    app.ui_toffset = ts.offset();
    app.ui_body = Some(area);
    app.ui_header = Some(area);
    app.ui_row_keys = keys_in_order;
    let total: u16 = spec
        .cols
        .iter()
        .map(|c| c.weight)
        .fold(0u16, u16::saturating_add)
        .max(1);
    let mut acc = 0u16;
    let mut starts = Vec::with_capacity(spec.cols.len());
    for c in &spec.cols {
        starts.push(acc);
        acc = acc.saturating_add((((c.weight * 100) / total).max(4)).saturating_add(1));
    }
    app.ui_col_starts = starts;
}

fn status_col(spec: &KindSpec) -> usize {
    spec.cols
        .iter()
        .position(|c| c.name == "STATUS")
        .unwrap_or(usize::MAX)
}

fn table_title(app: &App, spec: &KindSpec) -> String {
    let scope = if !spec.namespaced || app.all_ns {
        "*".to_string()
    } else {
        app.ns.clone()
    };
    let filter = if app.filter.is_empty() {
        String::new()
    } else {
        format!(" /{} ", app.filter)
    };
    format!("{scope}{}{filter}", "")
}

type Hint = (&'static str, &'static str); // (key, description)

const HINTS_LOGS: &[Hint] = &[
    ("/", "find"),
    ("n/p", "next/prev"),
    ("o", "per-occurrence"),
    ("<0>", "tail"),
    ("<1>", "head"),
    ("<2>", "1m"),
    ("<3>", "5m"),
    ("<4>", "15m"),
    ("<5>", "30m"),
    ("<6>", "1h"),
    ("P", "previous"),
    ("t", "timestamps"),
    ("w", "wrap"),
    ("s", "save"),
    ("j/k", "scroll"),
    ("pgup/dn", "page"),
    ("esc", "close"),
];
const HINTS_EXEC: &[Hint] = &[("type", "input"), ("ctrl-q", "detach")];
const HINTS_PULSE: &[Hint] = &[("esc", "back")];
const HINTS_PF: &[Hint] = &[
    ("j/k", "select"),
    ("s", "cancel selected"),
    ("X", "cancel ALL"),
    ("q", "back"),
];

fn hint_color(key: &str, t: &Theme) -> Color {
    match key {
        "ctrl-q" | "esc" | "X" | "s" => t.bad,
        "?" | ":" | "/" | "tab" | "C" | "P" | "w" | "o" => t.accent,
        _ => t.ok,
    }
}

const GRID_GAP: usize = 3;

fn normalize_grid(items: &[Hint], cols: usize) -> Vec<Vec<Option<Hint>>> {
    let rows = items.len().div_ceil(cols.max(1));
    let mut grid = vec![vec![None; cols]; rows];
    for (i, h) in items.iter().enumerate() {
        grid[i / cols][i % cols] = Some(*h);
    }
    grid
}

fn draw_shortcuts_grid(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let owned: Vec<Hint>;
    let flat: &[Hint] = match (&app.mode, app.view) {
        (Mode::Logs(st), _) => {
            let mut h = HINTS_LOGS.to_vec();
            if let Some(pos) = h.iter().position(|(k, _)| *k == "o") {
                h[pos] = if st.count_occurrences {
                    ("o", "toggle per-line")
                } else {
                    ("o", "toggle per-occurrence")
                };
            }
            owned = h;
            &owned
        }
        (Mode::Exec(_), _) => {
            owned = HINTS_EXEC.to_vec();
            &owned
        }
        (_, ViewKind::Pulse) => {
            owned = HINTS_PULSE.to_vec();
            &owned
        }
        (_, ViewKind::Pf) => {
            owned = HINTS_PF.to_vec();
            &owned
        }
        _ => {
            owned = app.context_hints();
            &owned
        }
    };
    if flat.is_empty() || area.width == 0 {
        return;
    }

    type HintGrid = (usize, Vec<Vec<Option<Hint>>>, Vec<usize>);

    let mut chosen: Option<HintGrid> = None;
    for cols in [3usize, 2, 1] {
        let grid = normalize_grid(flat, cols);
        let mut widths = vec![0usize; cols];
        for (i, h) in flat.iter().enumerate() {
            let c = i % cols;
            widths[c] = widths[c].max(h.0.len() + 3 + h.1.len());
        }
        let total: usize = widths.iter().sum::<usize>() + GRID_GAP * (cols - 1);
        if total <= area.width as usize {
            chosen = Some((cols, grid, widths));
            break;
        }
    }
    let Some((cols, grid, widths)) = chosen else {
        return;
    };

    let rows_n = grid.len() as u16;
    let pad_y = if area.height > rows_n {
        ((area.height - rows_n) / 2).max(1)
    } else {
        0
    };
    for (r, row) in grid.iter().enumerate() {
        let y = area.y + pad_y + r as u16;
        if y >= area.y + area.height {
            break;
        }
        let mut spans: Vec<Span> = Vec::new();
        for (c, cell) in row.iter().enumerate() {
            match cell {
                Some((k, d)) => {
                    let used = k.len() + 3 + d.len();
                    spans.push(Span::styled(
                        format!("<{k}>"),
                        Style::default()
                            .fg(hint_color(k, t))
                            .add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        d.to_string(),
                        Style::default().fg(Color::Gray),
                    ));
                    spans.push(Span::raw(" ".repeat(widths[c].saturating_sub(used))));
                }
                None => spans.push(Span::raw(" ".repeat(widths[c]))),
            }
            if c + 1 < cols {
                spans.push(Span::raw(" ".repeat(GRID_GAP)));
            }
        }
        f.render_widget(
            Line::from(spans),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
        );
    }
}

fn draw_hints(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;

    let (hints, second): (Vec<Hint>, Option<Vec<Hint>>) = match &app.mode {
        Mode::Logs(st) => {
            let mut h = HINTS_LOGS.to_vec();
            if let Some(pos) = h.iter().position(|(k, _)| *k == "o") {
                h[pos] = if st.count_occurrences {
                    ("o", "toggle per-line")
                } else {
                    ("o", "toggle per-occurrence")
                };
            }
            (h, None)
        }
        Mode::Exec(_) => (HINTS_EXEC.to_vec(), None),
        _ => match app.view {
            ViewKind::Pulse => (HINTS_PULSE.to_vec(), None),
            ViewKind::Pf => (HINTS_PF.to_vec(), None),
            _ => {
                let mut dyn_hints = vec![("enter", "actions")];
                if let Some(spec) = &app.view_spec
                    && spec.kind.as_str() == "Pod"
                {
                    dyn_hints.extend([("l", "logs"), ("s", "shell"), ("p", "pf")]);
                }
                dyn_hints.extend([
                    ("d", "describe"),
                    ("y", "yaml"),
                    ("e", "edit"),
                    ("ctrl-d", "del"),
                ]);
                (
                    dyn_hints,
                    Some(vec![
                        ("R", "restart"),
                        ("S", "scale"),
                        ("C", "contexts"),
                        ("r", "reload"),
                        ("g/G", "top/bot"),
                    ]),
                )
            }
        },
    };

    fn render_hint_row(f: &mut Frame, hints: &[Hint], area: Rect, t: &Theme) {
        let gap = "   ";
        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        let mut used = 1usize;
        for (i, (k, d)) in hints.iter().enumerate() {
            let piece_len = k.len() + d.len() + 2 + gap.len();
            if used + piece_len > area.width as usize {
                break;
            }
            used += piece_len;
            spans.push(Span::styled(
                format!("<{k}>"),
                Style::default()
                    .fg(hint_color(k, t))
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                d.to_string(),
                Style::default().fg(Color::Gray),
            ));
            if i + 1 < hints.len() {
                spans.push(Span::raw(gap));
            }
        }
        f.render_widget(Line::from(spans), area);
    }

    if let Some(sec) = second {
        let h1 = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        let h2 = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: 1,
        };
        render_hint_row(f, &hints, h1, t);
        render_hint_row(f, &sec, h2, t);
    } else {
        render_hint_row(f, &hints, area, t);
    }
}

fn highlight_line(line: &str, q: &str, t: &Theme, active_occ: Option<usize>) -> Line<'static> {
    if q.is_empty() {
        return Line::from(line.to_string());
    }
    let lower_q: Vec<char> = q
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    if lower_q.is_empty() {
        return Line::from(line.to_string());
    }
    let line_chars: Vec<char> = line.chars().collect();
    let lower_line_chars: Vec<char> = line_chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut last_char_idx = 0;
    let mut occ_count = 0usize;
    let q_len = lower_q.len();

    let mut i = 0;
    while i + q_len <= lower_line_chars.len() {
        if lower_line_chars[i..i + q_len] == lower_q[..] {
            if i > last_char_idx {
                let unmatch_str: String = line_chars[last_char_idx..i].iter().collect();
                spans.push(Span::raw(unmatch_str));
            }
            let matched_str: String = line_chars[i..i + q_len].iter().collect();
            let is_current = match active_occ {
                Some(usize::MAX) => true,
                Some(target_occ) => target_occ == occ_count,
                None => false,
            };
            let hl_style = if is_current {
                Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::Rgb(20, 60, 160))
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default()
                    .fg(t.bg_sel)
                    .bg(t.info)
                    .add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled(matched_str, hl_style));
            occ_count += 1;
            last_char_idx = i + q_len;
            i += q_len;
        } else {
            i += 1;
        }
    }
    if last_char_idx < line_chars.len() {
        let tail_str: String = line_chars[last_char_idx..].iter().collect();
        spans.push(Span::raw(tail_str));
    }
    Line::from(spans)
}

fn draw_logs(f: &mut Frame, app: &App, st: &crate::app::LogsState, area: Rect) {
    let wrap = ratatui::widgets::Wrap { trim: false };
    let q_active = !st.query.is_empty();
    let searching = st.search;
    let query = st.query.clone();
    let filtered: Vec<String> = if q_active {
        let ql = query.to_lowercase();
        st.lines
            .iter()
            .filter(|l| l.to_lowercase().contains(&ql))
            .cloned()
            .collect()
    } else {
        st.lines.clone()
    };
    let total = filtered.len();
    let inner_h = area.height.saturating_sub(2) as usize;
    let scroll = st.scroll_from_end.min(total.saturating_sub(1));
    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(inner_h);

    let matches = if q_active {
        crate::app::compute_log_matches(&st.lines, &query, st.count_occurrences)
    } else {
        Vec::new()
    };

    let content: Vec<Line> = if q_active {
        let t = app.theme.clone();
        filtered[start..end]
            .iter()
            .enumerate()
            .map(|(rel_idx, l)| {
                let filtered_line_idx = start + rel_idx;
                let active_occ_for_line = if let Some(cur_idx) = st.match_idx {
                    if let Some(&(m_line, m_occ)) = matches.get(cur_idx) {
                        if m_line == filtered_line_idx {
                            Some(m_occ)
                        } else {
                            Some(usize::MAX - 1)
                        }
                    } else {
                        None
                    }
                } else if !matches.is_empty() {
                    if let Some(&(m_line, m_occ)) = matches.first() {
                        if m_line == filtered_line_idx {
                            Some(m_occ)
                        } else {
                            Some(usize::MAX - 1)
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                highlight_line(l, &query, &t, active_occ_for_line)
            })
            .collect()
    } else {
        filtered[start..end]
            .iter()
            .map(|l| Line::from(l.as_str()))
            .collect()
    };
    let win_tag = match st.window {
        crate::k8s::LogWindow::Tail(n) => format!("[tail:{n}] "),
        crate::k8s::LogWindow::Head => "[head] ".to_string(),
        crate::k8s::LogWindow::Since(s) => format!("[since:{}m] ", s / 60),
    };
    let occ_flag = if st.count_occurrences {
        "per-occurrence[o]"
    } else {
        "per-line[o]"
    };
    let flags = format!(
        "{}[follow] prev:{} ts:{} wrap:{} {} {}",
        win_tag, st.previous, st.timestamps, st.wrap, occ_flag, st.status
    );
    let match_note = if q_active && st.match_total > 0 {
        let cur = st.match_idx.map(|i| i + 1).unwrap_or(0);
        let (mode_tag, next_mode) = if st.count_occurrences {
            ("per-occurrence", "per-line")
        } else {
            ("per-line", "per-occurrence")
        };
        format!(
            "  match [{}/{}] ({mode_tag}) (n=next p=prev o={next_mode} esc=clear)",
            cur, st.match_total
        )
    } else {
        String::new()
    };
    let search_note = if searching {
        format!("  \u{2318}find: {}█ (enter=apply, esc=cancel)", st.query)
    } else if q_active {
        format!("  /'{}' {} lines (esc clears)", st.query, total)
    } else {
        String::new()
    };
    let title = format!(
        " logs {}/{} {}{}{} ",
        st.ns,
        st.pod,
        st.container.as_deref().unwrap_or("(all)"),
        if q_active { " \u{00b7} filtered" } else { "" },
        if q_active {
            if st.count_occurrences {
                " \u{00b7} per-occurrence"
            } else {
                " \u{00b7} per-line"
            }
        } else {
            ""
        }
    );
    let mut para = Paragraph::new(content).block(
        Block::new()
            .borders(Borders::ALL)
            .title(Span::styled(title, Style::default().fg(app.theme.ok))),
    );
    if st.wrap {
        para = para.wrap(wrap);
    }
    f.render_widget(para, area);
    let foot = Rect {
        y: area.y + area.height - 1,
        x: area.x,
        width: area.width,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{flags}{search_note}{match_note}"),
            Style::default().fg(if searching || q_active {
                Color::Yellow
            } else {
                Color::DarkGray
            }),
        ))),
        foot,
    );
}

fn draw_exec(f: &mut Frame, app: &mut App, area: Rect) {
    let Mode::Exec(ex) = &mut app.mode else {
        return;
    };
    let text = String::from_utf8_lossy(&ex.buffer).to_string();
    let title = format!(" exec:{} [{}] ", ex.pod, ex.status);
    let lines: Vec<Line> = text.split('\n').map(Line::from).collect();
    let h = area.height.saturating_sub(2) as usize;
    let start = lines.len().saturating_sub(h);
    let para = Paragraph::new(lines[start..].to_vec())
        .wrap(ratatui::widgets::Wrap { trim: false })
        .block(
            Block::new()
                .borders(Borders::ALL)
                .title(Span::styled(title, Style::default().fg(app.theme.info))),
        );
    f.render_widget(para, area);
}

fn draw_text_pane(f: &mut Frame, app: &mut App, area: Rect) {
    let Mode::TextPane {
        title,
        lines,
        pos,
        wrap,
    } = &mut app.mode
    else {
        return;
    };
    let inner_h = area.height.saturating_sub(2) as usize;
    let max_pos = lines.len().saturating_sub(inner_h);
    *pos = (*pos).min(max_pos);
    let slice: Vec<Line> = lines[*pos..(*pos + inner_h).min(lines.len())]
        .iter()
        .map(|l| Line::from(l.clone()))
        .collect();
    let w = ratatui::widgets::Wrap { trim: false };
    let mut para = Paragraph::new(slice).block(Block::new().borders(Borders::ALL).title(
        Span::styled(format!(" {title} "), Style::default().fg(app.theme.title)),
    ));
    if *wrap {
        para = para.wrap(w);
    }
    f.render_widget(para, area);
}

// ---- pulse dashboard (k9s parity) ----

/// 3×3 dial digits — exact port of derailed/k9s internal/tchart To3x3Char:
/// heavy box-drawing glyphs so every digit shares one identical footprint.
const DIAL_DIGITS: [[&str; 3]; 10] = [
    ["┏━┓", "┃ ┃", "┗━┛"], // 0
    [" ╻ ", " ┃ ", " ╹ "], // 1
    ["╺━┓", "┏━┛", "┗━╸"], // 2
    ["━━┓", "╺━┫", "━━┛"], // 3
    ["╻ ╻", "┗━┫", "  ╹"], // 4
    ["┏━╸", "┗━┓", "╺━┛"], // 5
    ["┏━╸", "┣━┓", "┗━┛"], // 6
    ["━━┓", "  ┃", "  ╹"], // 7
    ["┏━┓", "┣━┫", "┗━┛"], // 8
    ["┏━┓", "┗━┫", "╺━┛"], // 9
];

/// braille tick k9s places between the ok/fault counters (U+2814)
const DIAL_SEP: &str = "\u{2814}";

/// number rendered in the k9s dial font; one style for the whole number,
/// digits packed flush exactly like k9s (3 cols per digit, no extra gap).
pub fn seven_seg(n: u64, style: Style) -> Vec<Line<'static>> {
    let s = n.to_string();
    let mut rows: [Vec<Span>; 3] = [vec![], vec![], vec![]];
    for ch in s.bytes() {
        let g = &DIAL_DIGITS[(ch - b'0') as usize % 10];
        for (ri, cell) in g.iter().enumerate() {
            rows[ri].push(Span::styled((*cell).to_string(), style));
        }
    }
    rows.into_iter().map(Line::from).collect()
}

fn mem_bar_char(pct: u64) -> &'static str {
    match pct {
        0..=12 => "\u{2581}",
        13..=25 => "\u{2582}",
        26..=37 => "\u{2583}",
        38..=50 => "\u{2584}",
        51..=62 => "\u{2585}",
        63..=75 => "\u{2586}",
        76..=88 => "\u{2587}",
        _ => "\u{2588}",
    }
}

/// ok/fault counters in the k9s gauge style: [ok digits] ⠔ [fault digits],
/// separator on the middle row. zero values are expected to arrive dimmed.
fn counter_pair_lines(
    ok_n: u64,
    bad_n: u64,
    ok_st: Style,
    bad_st: Style,
    sep_st: Style,
) -> Vec<Line<'static>> {
    let left = seven_seg(ok_n, ok_st);
    let right = seven_seg(bad_n, bad_st);
    let mut out = Vec::with_capacity(3);
    for i in 0..3usize {
        let mut spans: Vec<Span> =
            Vec::with_capacity(left[i].spans.len() + right[i].spans.len() + 1);
        spans.extend(left[i].spans.iter().cloned());
        spans.push(if i == 1 {
            Span::styled(DIAL_SEP.to_string(), sep_st)
        } else {
            Span::raw(" ")
        });
        spans.extend(right[i].spans.iter().cloned());
        out.push(Line::from(spans));
    }
    out
}

fn draw_pulse(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let ns_label = if app.all_ns {
        "all".to_string()
    } else {
        app.ns.clone()
    };
    let outer = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.header))
        .title(Span::styled(
            format!(" Pulses({ns_label}) "),
            Style::default().fg(t.title).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(outer, area);
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let counts = app.pulse_counts.lock().unwrap().clone();
    let cards_h = inner.height.min(18); // 3 rows x 6 (border+pill+digits+hint)
    let cards_area = Rect {
        height: cards_h,
        ..inner
    };
    let chart_area = Rect {
        y: inner.y + cards_h,
        height: inner.height.saturating_sub(cards_h),
        ..inner
    };

    let cols = 4u16;
    let rows_n = 3u16;
    let cw = cards_area.width / cols;
    let ch = cards_area.height / rows_n;
    for idx in 0..crate::app::PULSE_CARDS.len() {
        let (label, _) = crate::app::PULSE_CARDS[idx];
        let (total, healthy) = counts.get(label).copied().unwrap_or((0, 0));
        let col = (idx as u16) % cols;
        let row = (idx as u16) / cols;
        let cell_x = cards_area.x + col * cw;
        let cell_y = cards_area.y + row * ch;
        // individual bordered card with 1-col gap between cards
        let card = Rect {
            x: cell_x,
            y: cell_y,
            width: cw.saturating_sub(1),
            height: ch,
        };
        if card.width < 8 || card.height < 5 {
            continue;
        }
        let selected = idx == app.pulse_sel;
        let _degraded = healthy < total;

        // bordered enclosure
        let border_color = if selected { t.accent } else { t.dim };
        f.render_widget(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
            card,
        );
        let ic = Rect {
            x: card.x + 1,
            y: card.y + 1,
            width: card.width.saturating_sub(2),
            height: card.height.saturating_sub(2),
        };

        // k9s gauge parity: ok count left, fault count right, ⠔ between.
        // zero-valued numbers render dimmed (k9s "dimmed" style); no bold anywhere.
        let bad_n = total - healthy;
        let ok_st = if healthy > 0 {
            Style::default().fg(t.ok)
        } else {
            Style::default().fg(t.dim)
        };
        let bad_st = if bad_n > 0 {
            Style::default().fg(t.bad)
        } else {
            Style::default().fg(t.dim)
        };
        let sep_st = Style::default().fg(t.warn).add_modifier(Modifier::BOLD);
        let seg_lines: Vec<Line> =
            counter_pair_lines(healthy as u64, bad_n as u64, ok_st, bad_st, sep_st);

        let seg_block_h = seg_lines.len() as u16; // always 3
        let digits_y = ic.y + (ic.height.saturating_sub(seg_block_h)) / 2;
        let digit_w = seg_lines.first().map(|l| l.width() as u16).unwrap_or(7);
        let dx = ic.x + (ic.width.saturating_sub(digit_w)) / 2;
        for (i, line) in seg_lines.into_iter().take(ic.height as usize).enumerate() {
            f.render_widget(
                line,
                Rect {
                    x: dx.max(ic.x),
                    y: digits_y + i as u16,
                    width: ic.width.saturating_sub(dx.saturating_sub(ic.x)),
                    height: 1,
                },
            );
        }

        // resource title centered on the BOTTOM border; selected = inverted pill
        let title_style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(255, 165, 0))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.header)
        };
        let title_txt = format!(" {label} ");
        let tx_pos = card.x + (card.width.saturating_sub(title_txt.chars().count() as u16)) / 2;
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(title_txt.clone(), title_style))),
            Rect {
                x: tx_pos,
                y: card.y + card.height - 1,
                width: (title_txt.chars().count() as u16).min(card.width),
                height: 1,
            },
        );
    }

    // ---- charts ----
    if chart_area.height < 5 || chart_area.width < 20 {
        return;
    }
    let halves = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chart_area);

    // chronology: ring pushes newest at back — natural iter() is oldest→newest (L→R)
    let hist: Vec<crate::app::PulseSample> = {
        let h = app.pulse_hist.lock().unwrap();
        h.iter().cloned().collect()
    };

    for (side, is_cpu) in [(halves[0], true), (halves[1], false)] {
        let name = if is_cpu { " Cpu " } else { " Memory " };
        // CPU cyan/blue border · Memory magenta/pink border (per spec)
        let accent_border = if is_cpu {
            Color::Cyan
        } else {
            Color::Rgb(255, 105, 180)
        };
        let blk = Block::new()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(accent_border))
            .title(Span::styled(
                name,
                Style::default()
                    .fg(accent_border)
                    .add_modifier(Modifier::BOLD),
            ));
        f.render_widget(blk, side);
        let ib = Rect {
            x: side.x + 1,
            y: side.y + 1,
            width: side.width.saturating_sub(2),
            height: side.height.saturating_sub(2),
        };

        // scoping: ns-scoped pod sums when namespace focused; cluster node totals in all-ns
        let (vals, unit_now): (Vec<u64>, String) = if is_cpu {
            if !app.all_ns {
                (
                    hist.iter()
                        .map(|h| h.ns_cpu_m.unwrap_or(0.0) as u64)
                        .collect(),
                    match hist.last().and_then(|h| h.ns_cpu_m) {
                        Some(u) => format!("Cpu {u:.0}m (ns {ns_label})"),
                        None => "Cpu n/a".into(),
                    },
                )
            } else {
                (
                    hist.iter()
                        .map(|h| match (h.cpu_m, h.cpu_cap_m > 0.0) {
                            (Some(u), true) => (u / h.cpu_cap_m * 100.0) as u64,
                            _ => 0,
                        })
                        .collect(),
                    match hist.last().map(|h| (h.cpu_m, h.cpu_cap_m)) {
                        Some((Some(u), cap)) => {
                            format!("Cpu {:.0}%({:.0}m/{:.0}m)", u / cap * 100.0, u, cap)
                        }
                        _ => "Cpu n/a".into(),
                    },
                )
            }
        } else if !app.all_ns {
            (
                hist.iter()
                    .map(|h| h.ns_mem_b.unwrap_or(0) / (1024 * 1024))
                    .collect(),
                match hist.last().and_then(|h| h.ns_mem_b) {
                    Some(u) => format!("Memory {}Mi (ns {ns_label})", u / (1024 * 1024)),
                    None => "Memory n/a".into(),
                },
            )
        } else {
            (
                hist.iter()
                    .map(|h| match (h.mem_b, h.mem_cap_b > 0) {
                        (Some(u), true) => (u as f64 / h.mem_cap_b as f64 * 100.0) as u64,
                        _ => 0,
                    })
                    .collect(),
                match hist.last() {
                    Some(h) if h.mem_b.is_some() && h.mem_cap_b > 0 => {
                        let u = h.mem_b.unwrap();
                        let cap = h.mem_cap_b;
                        format!(
                            "Memory {:.0}%({}/{}Mi)",
                            u as f64 / cap as f64 * 100.0,
                            u / (1024 * 1024),
                            cap / (1024 * 1024)
                        )
                    }
                    _ => "Memory n/a".into(),
                },
            )
        };

        if vals.is_empty() {
            continue;
        }

        let data_h = ib.height.saturating_sub(2).max(1) as usize;
        let spark_data: Vec<u64> = vals[vals.len().saturating_sub(data_h)..].to_vec();

        if is_cpu {
            let spark = ratatui::widgets::Sparkline::default()
                .data(&spark_data)
                .style(Style::default().fg(Color::Cyan));
            f.render_widget(
                spark,
                Rect {
                    height: data_h as u16,
                    ..ib
                },
            );
        } else {
            let bar_area = Rect {
                height: data_h as u16,
                ..ib
            };
            let width = bar_area.width as usize;
            let samples: Vec<u64> = vals[vals.len().saturating_sub(width)..].to_vec();
            let mut spans: Vec<Span> = samples
                .iter()
                .map(|&p| {
                    let color = if p >= 85 {
                        t.bad
                    } else if p >= 70 {
                        t.warn
                    } else {
                        t.ok
                    };
                    Span::styled(mem_bar_char(p), Style::default().fg(color))
                })
                .collect();
            while spans.len() < width {
                spans.insert(0, Span::raw("\u{2581}"));
            }
            f.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect {
                    height: 1,
                    y: bar_area.y + bar_area.height.saturating_sub(1),
                    width: bar_area.width,
                    ..bar_area
                },
            );
        }

        // x-axis timestamps: oldest left → newest right
        let times: Vec<String> = hist
            .iter()
            .map(|h| {
                h.ts.with_timezone(&chrono::Local)
                    .format("%H:%M")
                    .to_string()
            })
            .collect();
        let axis_times: Vec<String> = if times.len() <= 5 {
            times.clone()
        } else {
            let step = (times.len() / 5).max(1);
            (0..times.len())
                .step_by(step)
                .map(|i| times[i].clone())
                .take(5)
                .collect()
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                axis_times.join("      "),
                Style::default().fg(t.dim),
            ))),
            Rect {
                y: ib.y + data_h as u16,
                height: 1,
                width: ib.width,
                ..ib
            },
        );

        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                unit_now,
                Style::default()
                    .fg(accent_border)
                    .add_modifier(Modifier::BOLD),
            ))),
            Rect {
                y: ib.y + data_h as u16 + 1,
                height: 1,
                width: ib.width,
                ..ib
            },
        );
    }
}

fn draw_pf(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .pfs
        .iter()
        .map(|e| {
            ListItem::new(Line::from(vec![
                Span::raw(format!("#{:<3}", e.id)),
                Span::styled(
                    format!("127.0.0.1:{:<6}", e.local_port),
                    Style::default().fg(app.theme.ok),
                ),
                Span::raw(" → "),
                Span::styled(e.target.clone(), Style::default().fg(app.theme.info)),
                Span::raw(format!(
                    "  conns:{}",
                    e.conns.load(std::sync::atomic::Ordering::Relaxed)
                )),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(Block::new().borders(Borders::ALL).title(Span::styled(
            " port-forwards ",
            Style::default().fg(Color::White),
        )))
        .highlight_style(
            Style::default()
                .bg(app.theme.bg_sel)
                .add_modifier(Modifier::BOLD),
        );
    let sel = if app.pfs.is_empty() {
        None
    } else {
        Some(app.pf_sel.min(app.pfs.len() - 1))
    };
    f.render_stateful_widget(
        list,
        area,
        &mut ratatui::widgets::ListState::default().with_selected(sel),
    );
}

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let vw = width.min(area.width);
    let vh = height.min(area.height);
    Rect {
        x: area.x + (area.width - vw) / 2,
        y: area.y + (area.height - vh) / 2,
        width: vw,
        height: vh,
    }
}

fn draw_menu(f: &mut Frame, app: &App) {
    let Mode::Menu(m) = &app.mode else { return };
    let w = m.items.iter().map(|i| i.label.len()).max().unwrap_or(10) as u16 + 6;
    let h = m.items.len() as u16 + 2;
    let area = centered(w.max(30), h.min(24), f.area());
    let items: Vec<ListItem> = m
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let marker = if i == m.sel { "▶ " } else { "  " };
            ListItem::new(Line::from(Span::raw(format!("{marker}{}", it.label))))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::new().borders(Borders::ALL).title(Span::styled(
                format!(" {} ", m.title),
                Style::default()
                    .fg(Color::Black)
                    .bg(app.theme.accent)
                    .bold(),
            )),
        )
        .highlight_style(Style::default().bg(app.theme.bg_sel));
    f.render_widget(Clear, area);
    f.render_stateful_widget(
        list,
        area,
        &mut ratatui::widgets::ListState::default().with_selected(Some(m.sel)),
    );
}

fn draw_confirm(f: &mut Frame, app: &App, prompt: &str) {
    let sel_yes = matches!(&app.mode, Mode::Confirm { sel_yes: true, .. });
    let prompt_owned;
    let prompt = if let Some(dl) = app.theme_deadline {
        let left = dl
            .saturating_duration_since(std::time::Instant::now())
            .as_secs();
        prompt_owned = format!("{prompt}  [auto-revert in {left}s]");
        prompt_owned.as_str()
    } else {
        prompt
    };
    let area = centered(64, 8, f.area());
    f.render_widget(Clear, area);

    let yes_style = if sel_yes {
        Style::default()
            .fg(Color::Black)
            .bg(app.theme.ok)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.dim)
    };
    let no_style = if !sel_yes {
        Style::default()
            .fg(Color::Black)
            .bg(app.theme.bad)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.dim)
    };

    let nav = Line::from(Span::styled(
        "\u{2190}\u{2192}/tab select \u{2502} enter run \u{2502} y/n shortcut \u{2502} esc cancel",
        Style::default().fg(app.theme.dim),
    ));
    let txt = vec![
        Line::from(""),
        Line::from(Span::styled(
            prompt.to_string(),
            Style::default()
                .fg(app.theme.warn)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  [ Yes ]  ", yes_style),
            Span::raw("    "),
            Span::styled("  [ No ]  ", no_style),
        ]),
        Line::from(""),
        nav,
    ];
    f.render_widget(
        Paragraph::new(txt).alignment(Alignment::Center).block(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if sel_yes {
                    app.theme.ok
                } else {
                    app.theme.dim
                })),
        ),
        area,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_log_export(
    f: &mut Frame,
    app: &App,
    dir_buf: &str,
    file_buf: &str,
    focus: crate::app::SaveFocus,
    suggestions: &[String],
    sug_idx: Option<usize>,
    sug_scroll: usize,
) {
    let t = &app.theme;
    let area = f.area();
    let w = 78u16.min(area.width.saturating_sub(4));
    let has_sug = !suggestions.is_empty();
    const MAX_VISIBLE: usize = 5;
    let visible_count = suggestions.len().min(MAX_VISIBLE);
    let sug_height = if has_sug {
        (visible_count as u16) + 1
    } else {
        0
    };
    let h = (15 + sug_height).min(area.height.saturating_sub(2));
    let r = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, r);

    let dir_focused = focus == crate::app::SaveFocus::Directory;
    let file_focused = focus == crate::app::SaveFocus::Filename;
    let ok_focused = focus == crate::app::SaveFocus::OkBtn;
    let cancel_focused = focus == crate::app::SaveFocus::CancelBtn;

    let lbl_style = |focused: bool| {
        if focused {
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.info)
        }
    };

    let mut txt = vec![
        Line::from(Span::styled(
            " <Save Logs> ",
            Style::default().fg(t.title).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // Directory Field
    let mut dir_line = vec![Span::styled("Directory: ", lbl_style(dir_focused))];
    if dir_buf.is_empty() {
        if dir_focused {
            dir_line.push(Span::styled(
                "Type folder path e.g. /tmp, ./, ~/█",
                Style::default().fg(t.dim).add_modifier(Modifier::ITALIC),
            ));
        } else {
            dir_line.push(Span::styled("/tmp", Style::default().fg(t.dim)));
        }
    } else {
        dir_line.push(Span::styled(
            format!(
                "{}{}",
                dir_buf,
                if dir_focused && sug_idx.is_none() {
                    "█"
                } else {
                    ""
                }
            ),
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
        ));
    }
    txt.push(Line::from(dir_line));

    // Suggestions list (max 5 visible, with scroll offset & scrollbar indicator)
    if has_sug {
        let total = suggestions.len();
        let start = sug_scroll.min(total.saturating_sub(1));
        let end = (start + MAX_VISIBLE).min(total);
        let header_info = if total > MAX_VISIBLE {
            format!("  Folders ({}-{} of {}):", start + 1, end, total)
        } else {
            format!("  Folders ({}):", total)
        };
        txt.push(Line::from(Span::styled(
            header_info,
            Style::default()
                .fg(t.dim)
                .add_modifier(Modifier::UNDERLINED),
        )));

        for (slot_idx, i) in (start..end).enumerate() {
            let sug = &suggestions[i];
            let is_sel = sug_idx == Some(i);
            let style = if is_sel {
                Style::default()
                    .fg(t.bg_sel)
                    .bg(t.ok)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.dim)
            };

            // Scrollbar glyph for the current slot
            let bar_glyph = if total > MAX_VISIBLE {
                let bar_pos = (sug_scroll * MAX_VISIBLE) / total;
                if slot_idx == bar_pos {
                    " █ "
                } else {
                    " │ "
                }
            } else {
                ""
            };

            let max_name_len = (w as usize).saturating_sub(20);
            let display_sug = if sug.chars().count() > max_name_len {
                let cutoff = max_name_len.saturating_sub(2);
                let truncated: String = sug.chars().take(cutoff).collect();
                format!("{truncated}…/")
            } else {
                sug.clone()
            };

            txt.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    if is_sel {
                        format!(" ▶ {display_sug:<max_name_len$} ")
                    } else {
                        format!("   {display_sug:<max_name_len$} ")
                    },
                    style,
                ),
                Span::styled(bar_glyph, Style::default().fg(t.dim)),
            ]));
        }
    }

    txt.push(Line::from(""));

    // Filename Field
    let mut file_line = vec![Span::styled("Filename:  ", lbl_style(file_focused))];
    if file_buf.is_empty() {
        if file_focused {
            file_line.push(Span::styled(
                "Enter filename█",
                Style::default().fg(t.dim).add_modifier(Modifier::ITALIC),
            ));
        } else {
            file_line.push(Span::styled("logs", Style::default().fg(t.dim)));
        }
    } else {
        file_line.push(Span::styled(
            format!("{}{}", file_buf, if file_focused { "█" } else { "" }),
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
        ));
    }
    file_line.push(Span::styled(".txt", Style::default().fg(t.dim)));
    txt.push(Line::from(file_line));

    txt.push(Line::from(""));

    // Buttons
    let ok_style = if ok_focused {
        Style::default()
            .fg(t.bg_sel)
            .bg(t.ok)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.ok)
    };
    let cancel_style = if cancel_focused {
        Style::default()
            .fg(t.bg_sel)
            .bg(t.bad)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.dim)
    };

    txt.push(Line::from(vec![
        Span::styled(
            if ok_focused {
                " [   OK   ] "
            } else {
                "  [  OK  ]  "
            },
            ok_style,
        ),
        Span::raw("    "),
        Span::styled(
            if cancel_focused {
                " [ Cancel ] "
            } else {
                "  [Cancel]  "
            },
            cancel_style,
        ),
    ]));

    txt.push(Line::from(""));
    txt.push(Line::from(Span::styled(
        "  Tab: switch fields · ↑↓: select folder · ←→: buttons · Enter: save · Esc: cancel",
        Style::default().fg(t.dim),
    )));

    f.render_widget(
        Paragraph::new(txt).alignment(Alignment::Center).block(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.header))
                .title(Span::styled(
                    " <Save Logs> ",
                    Style::default().fg(t.title).add_modifier(Modifier::BOLD),
                )),
        ),
        r,
    );
}

fn draw_port_forward(f: &mut Frame, app: &App, st: &crate::app::PfDialogState) {
    let t = &app.theme;
    let area = f.area();
    let w = 72u16.min(area.width.saturating_sub(4));
    let extra = if st.ports.len() > 1 {
        st.ports.len().min(4) as u16 + 1
    } else {
        0
    };
    let h = (14 + extra).min(area.height.saturating_sub(2));
    let r = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, r);

    let co_focused = st.focus == crate::app::PfFocus::ContainerPort;
    let lo_focused = st.focus == crate::app::PfFocus::LocalPort;
    let addr_focused = st.focus == crate::app::PfFocus::Address;
    let ok_focused = st.focus == crate::app::PfFocus::OkBtn;
    let cancel_focused = st.focus == crate::app::PfFocus::CancelBtn;

    let lbl_style = |focused: bool| {
        if focused {
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.info)
        }
    };

    let mut txt = vec![
        Line::from(Span::styled(
            " <PortForward> ",
            Style::default().fg(t.title).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Path: ", Style::default().fg(t.dim)),
            Span::styled(
                format!("{}/{}", st.ns, st.pod),
                Style::default().fg(t.title).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    if st.ports.len() > 1 {
        txt.push(Line::from(""));
        txt.push(Line::from(Span::styled(
            "Exposed Ports:",
            Style::default()
                .fg(t.dim)
                .add_modifier(Modifier::UNDERLINED),
        )));
        for (co, port, name) in st.ports.iter().take(4) {
            let n_str = name
                .as_deref()
                .map(|n| format!("({n})"))
                .unwrap_or_default();
            txt.push(Line::from(Span::styled(
                format!("  {co}::{port}{n_str}"),
                Style::default().fg(t.dim),
            )));
        }
    }

    txt.push(Line::from(""));

    // Container Port field
    let co_val = if st.container_port.is_empty() {
        if co_focused {
            vec![Span::styled(
                "Enter a container name::port█",
                Style::default().fg(t.dim).add_modifier(Modifier::ITALIC),
            )]
        } else {
            vec![Span::styled(
                "Enter a container name::port",
                Style::default().fg(t.dim),
            )]
        }
    } else {
        vec![Span::styled(
            format!("{}{}", st.container_port, if co_focused { "█" } else { "" }),
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
        )]
    };
    let mut co_line = vec![Span::styled("Container Port: ", lbl_style(co_focused))];
    co_line.extend(co_val);
    txt.push(Line::from(co_line));

    // Local Port field
    let lo_val = if st.local_port.is_empty() {
        if lo_focused {
            vec![Span::styled(
                "Enter a local port█",
                Style::default().fg(t.dim).add_modifier(Modifier::ITALIC),
            )]
        } else {
            vec![Span::styled(
                "Enter a local port",
                Style::default().fg(t.dim),
            )]
        }
    } else {
        vec![Span::styled(
            format!("{}{}", st.local_port, if lo_focused { "█" } else { "" }),
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
        )]
    };
    let mut lo_line = vec![Span::styled("Local Port:     ", lbl_style(lo_focused))];
    lo_line.extend(lo_val);
    txt.push(Line::from(lo_line));

    // Address field
    let addr_val = vec![Span::styled(
        format!("{}{}", st.address, if addr_focused { "█" } else { "" }),
        Style::default().fg(t.ok).add_modifier(Modifier::BOLD),
    )];
    let mut addr_line = vec![Span::styled("Address:        ", lbl_style(addr_focused))];
    addr_line.extend(addr_val);
    txt.push(Line::from(addr_line));

    txt.push(Line::from(""));

    // Buttons
    let ok_style = if ok_focused {
        Style::default()
            .fg(t.bg_sel)
            .bg(t.ok)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.ok)
    };
    let cancel_style = if cancel_focused {
        Style::default()
            .fg(t.bg_sel)
            .bg(t.bad)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.dim)
    };

    txt.push(Line::from(vec![
        Span::styled(
            if ok_focused {
                " [   OK   ] "
            } else {
                "  [  OK  ]  "
            },
            ok_style,
        ),
        Span::raw("    "),
        Span::styled(
            if cancel_focused {
                " [ Cancel ] "
            } else {
                "  [Cancel]  "
            },
            cancel_style,
        ),
    ]));

    txt.push(Line::from(""));
    txt.push(Line::from(Span::styled(
        "  tab / ↑↓  navigate   enter  submit   esc  cancel",
        Style::default().fg(t.dim),
    )));

    f.render_widget(
        Paragraph::new(txt).alignment(Alignment::Center).block(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.header))
                .title(Span::styled(
                    " <PortForward> ",
                    Style::default().fg(t.title).add_modifier(Modifier::BOLD),
                )),
        ),
        r,
    );
}

fn draw_input(f: &mut Frame, t: &Theme, buf: &str, label: String) {
    let area = centered(64, 5, f.area());
    f.render_widget(Clear, area);
    let txt = vec![
        Line::from(Span::styled(label, Style::default().fg(t.info))),
        Line::from(Span::raw(format!("{buf}█"))),
        Line::from(Span::styled(
            "<enter>apply  <esc>cancel",
            Style::default().fg(t.dim),
        )),
    ];
    f.render_widget(
        Paragraph::new(txt).block(Block::new().borders(Borders::ALL)),
        area,
    );
}

fn draw_input_line(f: &mut Frame, app: &App, buf: &str, r: Rect) {
    let is_cmd = matches!(app.mode, Mode::Cmd { .. });
    let label = if is_cmd { "command" } else { "filter" };
    let inner = Paragraph::new(Line::from(Span::styled(
        format!("{buf}█"),
        Style::default()
            .fg(app.theme.ok)
            .add_modifier(Modifier::BOLD),
    )))
    .block(
        Block::new()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.header))
            .title(Span::styled(
                format!(" {label} "),
                Style::default()
                    .fg(app.theme.header)
                    .add_modifier(Modifier::BOLD),
            )),
    );
    f.render_widget(inner, r);

    // k9s-style suggestion palette — floats directly above the command bar
    if is_cmd
        && !buf.is_empty()
        && let Mode::Cmd { sel, .. } = &app.mode
    {
        let sugg = crate::app::suggest(buf);
        if !sugg.is_empty() {
            let h = (sugg.len() as u16 + 2).min(11);
            let w = 26u16.max(sugg.iter().map(|sg| sg.len() as u16).max().unwrap_or(10) + 4);
            let pop = Rect {
                x: r.x,
                y: r.y.saturating_sub(h),
                width: w.min(r.width),
                height: h.min(r.y),
            };
            f.render_widget(Clear, pop);
            let items: Vec<ListItem> = sugg
                .iter()
                .enumerate()
                .map(|(i, sg)| {
                    let style = if i == *sel.min(&(sugg.len() - 1)) {
                        Style::default()
                            .bg(app.theme.bg_sel)
                            .fg(app.theme.ok)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(app.theme.info)
                    };
                    ListItem::new(Line::from(Span::styled(format!(" {sg} "), style)))
                })
                .collect();
            let lst = List::new(items).block(
                Block::new()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(app.theme.dim)),
            );
            f.render_widget(lst, pop);
        }
    }
}

/// modal acknowledgement box: message lines + a single highlighted [ OK ]
pub fn draw_notice(f: &mut Frame, app: &App, title: &str, lines: &[String]) {
    let t = &app.theme;
    let area = f.area();
    let w = 70u16.min(area.width.saturating_sub(4));
    let h = (lines.len() as u16 + 5).min(area.height.saturating_sub(4));
    let r = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h.max(6),
    };
    f.render_widget(Clear, r);
    let mut txt: Vec<Line> = vec![Line::from("")];
    for l in lines {
        // simple wrap at box inner width
        let max = (w as usize).saturating_sub(4);
        let mut cur = String::new();
        for word in l.split(' ') {
            if cur.chars().count() + word.chars().count() + 1 > max && !cur.is_empty() {
                txt.push(Line::from(Span::raw(cur.clone())));
                cur.clear();
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        txt.push(Line::from(Span::raw(cur)));
    }
    txt.push(Line::from(""));
    txt.push(Line::from(Span::styled(
        "[ OK ]  ·  press enter",
        Style::default()
            .fg(Color::Black)
            .bg(Color::LightBlue)
            .add_modifier(Modifier::BOLD),
    )));
    let para = Paragraph::new(txt).alignment(Alignment::Center).block(
        Block::new()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.bad))
            .title(Span::styled(
                format!(" {title} "),
                Style::default().fg(t.bad).add_modifier(Modifier::BOLD),
            )),
    );
    f.render_widget(para, r);
}

#[cfg(test)]
mod pulse_tests {
    use super::*;

    #[test]
    fn dial_font_uniform() {
        // every digit: 3 rows, every cell display-width 1 (3-wide matrix)
        for g in DIAL_DIGITS.iter() {
            assert_eq!(g.len(), 3);
            for row in g.iter() {
                assert_eq!(row.chars().count(), 3);
            }
        }
    }

    #[test]
    fn seven_seg_lines() {
        let st = Style::default();
        let lines = seven_seg(42, st);
        assert_eq!(lines.len(), 3);
        // two digits packed flush = one span per digit per row
        for l in &lines {
            assert_eq!(l.spans.len(), 2);
        }
        let empty = seven_seg(0, st);
        assert_eq!(empty.len(), 3);
        let s0: Vec<String> = empty
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(s0, vec!["┏━┓", "┃ ┃", "┗━┛"]);
    }

    #[test]
    fn counter_pair_shape() {
        let lines = counter_pair_lines(7, 4, Style::default(), Style::default(), Style::default());
        assert_eq!(lines.len(), 3);
        // 7 (3 cols) + ⠔ + 4 (3 cols) = width 7 on the middle row
        assert_eq!(lines[1].width(), 7);
        assert_eq!(lines[0].width(), 7);
    }
}

fn draw_theme_editor(
    f: &mut Frame,
    app: &App,
    values: &[(String, String)],
    sel: usize,
    editing: bool,
    buf: &str,
) {
    let t = &app.theme;
    let area = f.area();
    let w = 56u16.min(area.width.saturating_sub(4));
    let h = (values.len() as u16 + 6).min(area.height.saturating_sub(4));
    let r = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, r);
    let blk = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            " custom theme ".to_string(),
            Style::default()
                .fg(Color::Black)
                .bg(t.accent)
                .add_modifier(Modifier::BOLD),
        ));
    f.render_widget(blk, r);
    let inner = Rect {
        x: r.x + 2,
        y: r.y + 1,
        width: r.width.saturating_sub(4),
        height: r.height.saturating_sub(2),
    };
    for (i, (field, hexv)) in values.iter().enumerate() {
        let selected = i == sel;
        let content = if selected && editing {
            format!("{:<12}: {}█", field, buf)
        } else {
            format!(
                "{:<12}: {}{}",
                field,
                hexv,
                if selected { " ◄" } else { "" }
            )
        };
        // live swatch next to value
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(t.bg_sel)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.info)
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(content, style),
                Span::styled(
                    "  ██",
                    Style::default().fg(crate::cfg::hex_to_color(hexv).unwrap_or(t.dim)),
                ),
            ])),
            Rect {
                x: inner.x,
                y: inner.y + i as u16,
                width: inner.width,
                height: 1,
            },
        );
    }
    let foot_y = inner.y + values.len() as u16 + 1;
    if foot_y < inner.y + inner.height {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "↑↓ field · enter edit hex · s save · esc close".to_string(),
                Style::default().fg(t.dim),
            ))),
            Rect {
                x: inner.x,
                y: foot_y,
                width: inner.width,
                height: 1,
            },
        );
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;

    #[test]
    fn test_highlight_line_unicode_safety() {
        let t = Theme::resolve("dark");
        // Turkish dotted I (lowercase query and uppercase query)
        let res = highlight_line("İSTANBUL log line", "i", &t, None);
        assert!(!res.spans.is_empty());
        let res_sym = highlight_line("İSTANBUL log line", "İSTANBUL", &t, None);
        assert_eq!(res_sym.spans[0].content, "İSTANBUL");

        // German Umlaut & Accented characters
        let res2 = highlight_line("Über den Wolken schön", "über", &t, None);
        assert!(!res2.spans.is_empty());

        // Active occurrence selection
        let res3 = highlight_line("error 1 error 2 error 3", "error", &t, Some(1));
        assert_eq!(res3.spans.len(), 6); // [error, " 1 ", error, " 2 ", error, " 3"]
    }
}
