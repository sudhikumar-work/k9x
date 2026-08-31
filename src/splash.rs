use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use std::time::Instant;

const MATRIX_CHARS: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F', 'K', 'X', 'Z',
    'N', 'M', '#', '@', '$', '%', '&', '*', '+', '=', '-', ':', ';', '<', '>', '?', '/', '\\', '|',
    '~', '{', '}', '[', ']', 'ｦ', 'ｱ', 'ｳ', 'ｴ', 'ｵ', 'ｶ', 'ｷ', 'ｹ', 'ｺ', 'ｻ', 'ｼ', 'ｽ', 'ｾ', 'ｿ',
    'ﾀ', 'ﾂ', 'ﾃ', 'ﾅ', 'ﾆ', 'ﾇ', 'ﾍ', 'ﾎ', 'ﾏ', 'ﾐ', 'ﾑ', 'ﾒ', 'ﾓ', 'ﾔ', 'ﾕ', 'ﾗ', 'ﾘ', 'ﾜ', 'ﾝ',
];

const LOGO_K9XCLI: [&str; 6] = [
    "██╗  ██╗  ██████╗   ██╗  ██╗   ██████╗ ██╗      ██╗",
    "██║ ██╔╝ ██╔═══██╗  ╚██╗██╔╝  ██╔════╝ ██║      ██║",
    "█████╔╝  ╚██████║    ╚███╔╝   ██║      ██║      ██║",
    "██╔═██╗   ╚═══██║    ██╔██╗   ██║      ██║      ██║",
    "██║  ██╗  ██████╔╝  ██╔╝ ██╗  ╚██████╗ ███████╗ ██║",
    "╚═╝  ╚═╝  ╚═════╝   ╚═╝  ╚═╝   ╚═════╝ ╚══════╝ ╚═╝",
];

const LOGO_WIDTH: u16 = 51;
const LOGO_HEIGHT: u16 = 6;

struct RainColumn {
    head_y: f32,
    speed: f32,
    len: usize,
    chars: Vec<char>,
    delay: f32,
}

pub struct MatrixSplash {
    columns: Vec<RainColumn>,
    width: u16,
    height: u16,
    last_tick: Instant,
    rng_state: u64,
}

impl MatrixSplash {
    pub fn new(width: u16, height: u16) -> Self {
        let mut splash = Self {
            columns: Vec::new(),
            width: 0,
            height: 0,
            last_tick: Instant::now(),
            rng_state: 0x8542_5891_dead_beef,
        };
        splash.resize(width, height);
        splash
    }

    fn next_u32(&mut self) -> u32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        (self.rng_state >> 32) as u32
    }

    fn random_char(&mut self) -> char {
        let idx = (self.next_u32() as usize) % MATRIX_CHARS.len();
        MATRIX_CHARS[idx]
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        if self.width == width && self.height == height && !self.columns.is_empty() {
            return;
        }
        self.width = width;
        self.height = height;
        let cols_count = width as usize;
        let rows_count = height.max(1) as usize;

        self.columns.clear();
        for col_idx in 0..cols_count {
            let speed = 0.45 + ((self.next_u32() % 100) as f32 / 100.0) * 0.95;
            let len = 8 + ((self.next_u32() as usize) % 20);
            let mut chars = Vec::with_capacity(rows_count + len + 5);
            for _ in 0..(rows_count + len + 5) {
                let idx = (self.next_u32() as usize) % MATRIX_CHARS.len();
                chars.push(MATRIX_CHARS[idx]);
            }
            // Stagger start positions across screen
            let initial_y = -((self.next_u32() % ((height as u32).max(1) * 2 + 10)) as f32);
            let delay = if col_idx % 2 == 0 {
                0.0
            } else {
                (self.next_u32() % 15) as f32
            };

            self.columns.push(RainColumn {
                head_y: initial_y,
                speed,
                len,
                chars,
                delay,
            });
        }
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;

        let delta_factor = (elapsed * 30.0).clamp(0.5, 3.0);
        let h = self.height as f32;

        for col in &mut self.columns {
            if col.delay > 0.0 {
                col.delay -= delta_factor;
                continue;
            }

            col.head_y += col.speed * delta_factor;

            // Reset column when tail passes bottom
            if col.head_y - (col.len as f32) > h {
                col.head_y = -((col.len as f32) + 2.0);
                col.speed = 0.45 + ((col.speed * 100.0) as u32 % 100) as f32 / 100.0 * 0.95;
            }
        }

        // Randomly mutate a small percentage of characters for flickering effect
        let num_mutations = (self.columns.len() / 4).max(2);
        for _ in 0..num_mutations {
            let col_idx = (self.next_u32() as usize) % self.columns.len().max(1);
            let char_seed = self.next_u32();
            let ch = self.random_char();
            if let Some(col) = self.columns.get_mut(col_idx)
                && !col.chars.is_empty()
            {
                let char_idx = (char_seed as usize) % col.chars.len();
                col.chars[char_idx] = ch;
            }
        }
    }

    pub fn render(&mut self, f: &mut Frame, status: &str) {
        let area = f.area();
        if area.width != self.width || area.height != self.height {
            self.resize(area.width, area.height);
        }
        self.update();

        // 1. Render Matrix Digital Rain Background
        let mut lines = Vec::with_capacity(area.height as usize);
        for row in 0..area.height {
            let mut spans = Vec::with_capacity(area.width as usize);
            let row_f = row as f32;

            for col in &self.columns {
                let dist = row_f - col.head_y;
                let char_idx = ((row as usize) + (col.chars.len())) % col.chars.len();
                let ch = col.chars.get(char_idx).copied().unwrap_or(' ');

                if dist > 0.0 || dist < -(col.len as f32) || col.delay > 0.0 {
                    // Outside rain stream
                    spans.push(Span::raw(" "));
                } else if (-0.99..=0.0).contains(&dist) {
                    // Head of stream: glowing bright white-green
                    let style = Style::default()
                        .fg(Color::Rgb(220, 255, 230))
                        .add_modifier(Modifier::BOLD);
                    spans.push(Span::styled(ch.to_string(), style));
                } else if (-2.0..0.0).contains(&dist) {
                    // Just behind head: vibrant neon green
                    let style = Style::default()
                        .fg(Color::Rgb(0, 255, 65))
                        .add_modifier(Modifier::BOLD);
                    spans.push(Span::styled(ch.to_string(), style));
                } else {
                    // Tail gradient fading out down to black
                    let tail_pos = (-dist) / (col.len as f32).max(1.0); // 0.0 to 1.0
                    let green = ((1.0 - tail_pos) * 200.0) as u8 + 20;
                    let style =
                        Style::default().fg(Color::Rgb(0, green.max(30), (green / 4).max(10)));
                    spans.push(Span::styled(ch.to_string(), style));
                }
            }
            lines.push(Line::from(spans));
        }

        f.render_widget(Paragraph::new(lines), area);

        // 2. Render Centered Cyber Logo Box
        let box_w = (LOGO_WIDTH + 8).min(area.width.saturating_sub(4)).max(36);
        let box_h = (LOGO_HEIGHT + 10)
            .min(area.height.saturating_sub(2))
            .max(10);

        if area.width >= 40 && area.height >= 14 {
            let center_rect = centered_rect(box_w, box_h, area);
            f.render_widget(Clear, center_rect);

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(
                    Style::default()
                        .fg(Color::Rgb(0, 255, 128))
                        .add_modifier(Modifier::BOLD),
                )
                .style(Style::default().bg(Color::Rgb(5, 12, 8)));
            f.render_widget(block, center_rect);

            let inner = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),           // Top margin
                    Constraint::Length(LOGO_HEIGHT), // Logo (6)
                    Constraint::Length(1),           // Spacer
                    Constraint::Length(1),           // "— by —"
                    Constraint::Length(1),           // "⚡ Sudhi ⚡"
                    Constraint::Length(1),           // Spacer
                    Constraint::Length(1),           // Status / Loading
                ])
                .margin(1)
                .split(center_rect);

            // Compute exact horizontal padding for the ASCII logo to be perfectly centered
            let pad_x = inner[1].width.saturating_sub(LOGO_WIDTH) / 2;

            // Render Logo with Green Cyber Gradient
            for (r, line) in LOGO_K9XCLI.iter().enumerate() {
                if r < inner[1].height as usize {
                    let factor = r as f32 / LOGO_HEIGHT as f32;
                    let g = 255 - (factor * 60.0) as u8;
                    let b = 150 - (factor * 80.0) as u8;
                    let style = Style::default()
                        .fg(Color::Rgb(60, g, b))
                        .add_modifier(Modifier::BOLD);

                    let mut spans = Vec::with_capacity(2);
                    if pad_x > 0 {
                        spans.push(Span::raw(" ".repeat(pad_x as usize)));
                    }
                    spans.push(Span::styled(*line, style));

                    let p = Paragraph::new(Line::from(spans));
                    f.render_widget(
                        p,
                        Rect {
                            x: inner[1].x,
                            y: inner[1].y + r as u16,
                            width: inner[1].width,
                            height: 1,
                        },
                    );
                }
            }

            // Line 1: by
            let by_line = Line::from(vec![Span::styled(
                "— by —",
                Style::default().fg(Color::Rgb(100, 190, 130)),
            )]);
            f.render_widget(
                Paragraph::new(by_line).alignment(Alignment::Center),
                inner[3],
            );

            // Line 2: Sudhi
            let sudhi_line = Line::from(vec![
                Span::styled("⚡ ", Style::default().fg(Color::Rgb(0, 255, 150))),
                Span::styled(
                    "Sudhi",
                    Style::default()
                        .fg(Color::Rgb(0, 255, 128))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ⚡", Style::default().fg(Color::Rgb(0, 255, 150))),
            ]);
            f.render_widget(
                Paragraph::new(sudhi_line).alignment(Alignment::Center),
                inner[4],
            );

            // Status message
            let status_styled = Line::from(vec![
                Span::styled("► ", Style::default().fg(Color::Rgb(0, 255, 128))),
                Span::styled(status, Style::default().fg(Color::Rgb(160, 255, 180))),
            ]);
            f.render_widget(
                Paragraph::new(status_styled).alignment(Alignment::Center),
                inner[6],
            );
        } else {
            // Compact terminal fallback
            let p = Paragraph::new(vec![
                Line::from(Span::styled(
                    "k9x",
                    Style::default()
                        .fg(Color::Rgb(0, 255, 65))
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    "— by —",
                    Style::default().fg(Color::Rgb(120, 190, 140)),
                )),
                Line::from(Span::styled(
                    "Sudhi",
                    Style::default()
                        .fg(Color::Rgb(0, 255, 128))
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    status,
                    Style::default().fg(Color::Rgb(180, 255, 180)),
                )),
            ])
            .alignment(Alignment::Center);
            let r = centered_rect(32, 6, area);
            f.render_widget(Clear, r);
            f.render_widget(p, r);
        }
    }
}

fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let x = r.x + (r.width.saturating_sub(width)) / 2;
    let y = r.y + (r.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: width.min(r.width),
        height: height.min(r.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logo_dimensions_uniform() {
        for (i, line) in LOGO_K9XCLI.iter().enumerate() {
            println!("Line {i} count: {}", line.chars().count());
        }
        let first_len = LOGO_K9XCLI[0].chars().count();
        for line in LOGO_K9XCLI.iter() {
            assert_eq!(line.chars().count(), first_len);
        }
    }
}
