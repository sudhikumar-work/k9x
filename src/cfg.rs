use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Theme {
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub bad: Color,
    pub info: Color,
    pub dim: Color,
    pub header: Color,
    pub title: Color,
    pub bg_sel: Color,
}

impl Theme {
    pub fn resolve(name: &str) -> Self {
        match name {
            "light" => Self {
                accent: Color::White,
                ok: Color::Rgb(0, 128, 0),
                warn: Color::Rgb(180, 120, 0),
                bad: Color::Rgb(200, 0, 0),
                info: Color::Rgb(0, 100, 160),
                dim: Color::Rgb(120, 120, 120),
                header: Color::Rgb(0, 90, 140),
                title: Color::Black,
                bg_sel: Color::Rgb(220, 220, 220),
            },
            "mono" => Self {
                accent: Color::Gray,
                ok: Color::Gray,
                warn: Color::DarkGray,
                bad: Color::White,
                info: Color::Gray,
                dim: Color::DarkGray,
                header: Color::White,
                title: Color::White,
                bg_sel: Color::DarkGray,
            },
            "matrix" => Self {
                accent: Color::LightGreen,
                ok: Color::Green,
                warn: Color::Yellow,
                bad: Color::Red,
                info: Color::Cyan,
                dim: Color::Rgb(0, 160, 60),
                header: Color::LightGreen,
                title: Color::Rgb(0, 255, 128),
                bg_sel: Color::Indexed(22),
            },
            "neon" => Self {
                accent: Color::Rgb(255, 16, 240),
                ok: Color::Rgb(57, 255, 20),
                warn: Color::Rgb(255, 215, 0),
                bad: Color::Rgb(255, 45, 85),
                info: Color::Rgb(0, 229, 255),
                dim: Color::Rgb(108, 108, 140),
                header: Color::Rgb(57, 255, 20),
                title: Color::Rgb(255, 163, 255),
                bg_sel: Color::Rgb(58, 0, 58),
            },
            _ => Self {
                accent: Color::LightBlue,
                ok: Color::Green,
                warn: Color::Yellow,
                bad: Color::Red,
                info: Color::Cyan,
                dim: Color::DarkGray,
                header: Color::Cyan,
                title: Color::White,
                bg_sel: Color::DarkGray,
            },
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct FileCfg {
    pub context: String,
    pub namespace: String,
    pub all_namespaces: bool,
    pub readonly: bool,
    pub default_view: String,
    pub tick_ms: u64,
    pub log_tail: i64,
    pub log_cap: usize,
    /// sample per-pod CPU/MEM from metrics.k8s.io (scoped to current ns; 10s cadence)
    #[serde(default = "default_true")]
    pub pod_metrics: bool,
    #[serde(default)]
    pub thresholds: Thresholds,
    pub theme: String,
}

fn default_true() -> bool {
    true
}

/// warn/crit percentages driving Load-row + node usage colors
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Thresholds {
    #[serde(default = "d70")]
    pub cpu_warn: u16,
    #[serde(default = "d90")]
    pub cpu_crit: u16,
    #[serde(default = "d75")]
    pub mem_warn: u16,
    #[serde(default = "d90")]
    pub mem_crit: u16,
}
impl Default for Thresholds {
    fn default() -> Self {
        Self {
            cpu_warn: 70,
            cpu_crit: 90,
            mem_warn: 75,
            mem_crit: 90,
        }
    }
}
fn d70() -> u16 {
    70
}
fn d90() -> u16 {
    90
}
fn d75() -> u16 {
    75
}

impl Default for FileCfg {
    fn default() -> Self {
        Self {
            context: String::new(),
            namespace: String::new(),
            all_namespaces: false,
            readonly: false,
            default_view: "po".into(),
            tick_ms: 200,
            log_tail: 5000,
            log_cap: 50_000,
            pod_metrics: true,
            thresholds: Thresholds::default(),
            theme: "matrix".into(),
        }
    }
}

pub fn ensure_secure_dir(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

pub fn secure_write<P: AsRef<std::path::Path>, C: AsRef<[u8]>>(
    path: P,
    contents: C,
) -> std::io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        ensure_secure_dir(parent)?;
    }
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

impl FileCfg {
    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("K9X_CONFIG") {
            return PathBuf::from(p);
        }
        let base = match std::env::var("XDG_CONFIG_HOME") {
            Ok(v) => PathBuf::from(v),
            Err(_) => dirs_home().join(".config"),
        };
        base.join("k9x").join("config.toml")
    }

    pub fn load() -> Self {
        let p = Self::path();
        match std::fs::read_to_string(&p) {
            Ok(s) => toml::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }
    pub fn save(&self) {
        let p = Self::path();
        if let Ok(t) = toml::to_string(self) {
            let _ = secure_write(p, t);
        }
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

#[derive(Deserialize, Clone, Debug)]
pub struct Plugin {
    #[serde(rename = "shortCut")]
    pub short_cut: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub command: String,
    #[serde(default)]
    pub background: bool,
    #[serde(default)]
    pub dangerous: bool,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Deserialize)]
struct PluginFile {
    #[serde(default)]
    plugin: std::collections::BTreeMap<String, Plugin>,
}

pub fn load_plugins() -> Vec<(String, Plugin)> {
    let path = std::env::var("K9X_PLUGINS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let base = match std::env::var("XDG_CONFIG_HOME") {
                Ok(v) => PathBuf::from(v),
                Err(_) => dirs_home().join(".config"),
            };
            base.join("k9x").join("plugins.yml")
        });
    let Ok(txt) = std::fs::read_to_string(&path) else {
        return vec![];
    };
    match serde_yaml::from_str::<PluginFile>(&txt) {
        Ok(pf) => pf.plugin.into_iter().collect(),
        Err(_) => vec![],
    }
}

#[derive(Deserialize, Clone)]
pub struct HotKey {
    #[serde(rename = "shortCut")]
    pub short_cut: String,
    #[serde(default)]
    pub description: String,
    pub command: String,
}

pub fn cfg_dir() -> PathBuf {
    std::env::var("K9X_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let base = match std::env::var("XDG_CONFIG_HOME") {
                Ok(v) => PathBuf::from(v),
                Err(_) => dirs_home().join(".config"),
            };
            base.join("k9x")
        })
}

pub fn load_aliases() -> Vec<(String, String)> {
    let p = cfg_dir().join("aliases.yml");
    let Ok(txt) = std::fs::read_to_string(&p) else {
        return vec![];
    };
    #[derive(Deserialize)]
    struct F {
        #[serde(default)]
        alias: std::collections::BTreeMap<String, String>,
    }
    serde_yaml::from_str::<F>(&txt)
        .map(|f| f.alias.into_iter().collect())
        .unwrap_or_default()
}

pub fn load_hotkeys() -> Vec<(String, HotKey)> {
    let p = cfg_dir().join("hotkeys.yml");
    let Ok(txt) = std::fs::read_to_string(&p) else {
        return vec![];
    };
    #[derive(Deserialize)]
    struct F {
        #[serde(default, rename = "hotKeys")]
        hot_keys: std::collections::BTreeMap<String, HotKey>,
    }
    serde_yaml::from_str::<F>(&txt)
        .map(|f| f.hot_keys.into_iter().collect())
        .unwrap_or_default()
}

// ---- views.yml: user-defined extra/replacement columns per resource ----

#[derive(Deserialize, Clone, Debug)]
pub struct ColDef {
    pub name: String,
    /// JSON path into the object, e.g. "status.podIP" or "spec.nodeName"
    pub path: String,
    /// optional relative width weight (clamped 1..=100 at apply time)
    #[serde(default)]
    pub weight: Option<u16>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct ViewOverride {
    /// replace ALL built-in columns instead of appending
    #[serde(default)]
    pub replace_columns: bool,
    #[serde(default, alias = "append_columns")]
    pub columns: Vec<ColDef>,
    /// reorder columns by name (case-insensitive, first unclaimed match).
    /// Unlisted columns keep their relative order after the listed ones;
    /// unknown names are ignored (fail-safe).
    #[serde(default)]
    pub order: Vec<String>,
    /// relative width weights per column name (case-insensitive),
    /// clamped to 1..=100 at apply time to keep percentage math safe
    #[serde(default)]
    pub widths: std::collections::BTreeMap<String, u16>,
}

pub fn views_path() -> PathBuf {
    cfg_dir().join("views.yml")
}

/// key = resource alias, plural, or kind (first match wins at apply time)
pub fn load_views() -> std::collections::BTreeMap<String, ViewOverride> {
    let p = views_path();
    let Ok(txt) = std::fs::read_to_string(&p) else {
        return Default::default();
    };
    #[derive(Deserialize)]
    struct F {
        #[serde(default)]
        views: std::collections::BTreeMap<String, ViewOverride>,
    }
    serde_yaml::from_str::<F>(&txt)
        .map(|f| f.views)
        .unwrap_or_default()
}

// ---- jumps.yml: custom navigation shortcuts ----

#[derive(Deserialize, Clone, Debug)]
pub struct Jump {
    #[serde(rename = "shortCut")]
    pub short_cut: String,
    #[serde(default)]
    pub description: String,
    /// target "alias" or "alias/filter-text"
    pub command: String,
}

pub fn load_jumps() -> Vec<Jump> {
    let p = cfg_dir().join("jumps.yml");
    let Ok(txt) = std::fs::read_to_string(&p) else {
        return vec![];
    };
    #[derive(Deserialize)]
    struct F {
        #[serde(default)]
        jumps: Vec<Jump>,
    }
    serde_yaml::from_str::<F>(&txt)
        .map(|f| f.jumps)
        .unwrap_or_default()
}

// ---- contexts.yml: optional per-context defaults ----

#[derive(Deserialize, Clone, Debug, Default)]
pub struct CtxDefaults {
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub view: String,
}

/// key = context name
pub fn load_contexts() -> std::collections::BTreeMap<String, CtxDefaults> {
    let p = cfg_dir().join("contexts.yml");
    let Ok(txt) = std::fs::read_to_string(&p) else {
        return Default::default();
    };
    #[derive(Deserialize)]
    struct F {
        #[serde(default)]
        contexts: std::collections::BTreeMap<String, CtxDefaults>,
    }
    serde_yaml::from_str::<F>(&txt)
        .map(|f| f.contexts)
        .unwrap_or_default()
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct StateCfg {
    #[serde(default)]
    pub last_namespace: String,
    #[serde(default)]
    pub last_context: String,
    /// remembered log export directory
    #[serde(default)]
    pub last_log_dir: String,
    /// remembered namespace per context ("" = all-namespaces was active)
    #[serde(default)]
    pub namespaces: std::collections::BTreeMap<String, String>,
    /// remembered view per context (e.g. "po")
    #[serde(default)]
    pub views: std::collections::BTreeMap<String, String>,
}

impl StateCfg {
    pub fn path() -> PathBuf {
        std::env::var("K9X_STATE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| cfg_dir().join("state.toml"))
    }
    pub fn load() -> Self {
        let mut st: Self = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        if !st.last_log_dir.is_empty() {
            let trimmed = st.last_log_dir.trim();
            if let Some(rest) = trimmed.strip_prefix("~/") {
                if let Ok(home) = std::env::var("HOME") {
                    st.last_log_dir = format!("{home}/{rest}");
                }
            } else if trimmed == "~" {
                if let Ok(home) = std::env::var("HOME") {
                    st.last_log_dir = home;
                }
            } else if let Ok(cwd) = std::env::current_dir() {
                let p = std::path::Path::new(trimmed);
                if p.is_relative() {
                    st.last_log_dir = cwd.join(p).to_string_lossy().to_string();
                }
            }
        }
        st
    }
    pub fn save(&self) {
        if let Ok(t) = toml::to_string(self) {
            let _ = secure_write(Self::path(), t);
        }
    }
    /// remembered namespace for a context; falls back to the legacy global
    /// last_namespace on first run after upgrade. "" (all-ns) yields None so the
    /// cluster default applies, matching the pre-per-context behavior.
    pub fn ns_for(&self, ctx: &str) -> Option<String> {
        if let Some(ns) = self.namespaces.get(ctx) {
            return if ns.is_empty() {
                None
            } else {
                Some(ns.clone())
            };
        }
        if !self.last_namespace.is_empty() {
            return Some(self.last_namespace.clone());
        }
        None
    }
    /// store the namespace under its context (and keep the legacy field in sync)
    pub fn remember_ns(&mut self, ctx: &str, ns: &str) {
        if !ctx.is_empty() {
            self.namespaces.insert(ctx.to_string(), ns.to_string());
            self.last_context = ctx.to_string();
        }
        self.last_namespace = ns.to_string();
    }

    /// remember the last view for a context
    pub fn remember_view(&mut self, ctx: &str, view: &str) {
        if !ctx.is_empty() && !view.is_empty() {
            self.views.insert(ctx.to_string(), view.to_string());
        }
    }

    /// remember the last directory used for saving logs (persisted as absolute path)
    pub fn remember_log_dir(&mut self, dir: &str) {
        let trimmed = dir.trim();
        if trimmed.is_empty() {
            return;
        }
        let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
            if let Ok(home) = std::env::var("HOME") {
                format!("{home}/{rest}")
            } else {
                trimmed.to_string()
            }
        } else if trimmed == "~" {
            std::env::var("HOME").unwrap_or_else(|_| trimmed.to_string())
        } else if let Ok(cwd) = std::env::current_dir() {
            let p = std::path::Path::new(trimmed);
            if p.is_relative() {
                cwd.join(p).to_string_lossy().to_string()
            } else {
                trimmed.to_string()
            }
        } else {
            trimmed.to_string()
        };
        self.last_log_dir = expanded;
        self.save();
    }
}

#[cfg(test)]
mod views_tests {
    use super::*;

    #[test]
    fn views_yml_parse() {
        let y = r#"
views:
  po:
    append_columns:
      - name: NODE
        path: spec.nodeName
        weight: 4
      - name: IP
        path: status.podIP
    order: [NAME, NODE, STATUS, AGE]
    widths:
      NAME: 5
      age: 1
  deploy:
    replace_columns: true
    columns:
      - name: NAME
        path: metadata.name
"#;
        #[derive(Deserialize)]
        struct F {
            #[serde(default)]
            views: std::collections::BTreeMap<String, ViewOverride>,
        }
        let f: F = serde_yaml::from_str(y).unwrap();
        let po = f.views.get("po").unwrap();
        assert_eq!(po.columns.len(), 2);
        assert_eq!(po.columns[0].path, "spec.nodeName");
        assert_eq!(po.columns[0].weight, Some(4));
        assert_eq!(po.columns[1].weight, None);
        assert!(!po.replace_columns);
        assert_eq!(
            po.order,
            vec!["NAME", "NODE", "STATUS", "AGE"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(po.widths.get("NAME"), Some(&5));
        assert_eq!(po.widths.get("age"), Some(&1));
        let dp = f.views.get("deploy").unwrap();
        assert!(dp.replace_columns);
        assert!(dp.order.is_empty());
        assert!(dp.widths.is_empty());
    }

    #[test]
    fn views_yml_backward_compatible_minimal() {
        // legacy files with only columns/replace_columns still parse
        let y = r#"
views:
  svc:
    columns:
      - name: EXTIP
        path: status.loadBalancer.ingress[0].ip
"#;
        #[derive(Deserialize)]
        struct F {
            #[serde(default)]
            views: std::collections::BTreeMap<String, ViewOverride>,
        }
        let f: F = serde_yaml::from_str(y).unwrap();
        let svc = f.views.get("svc").unwrap();
        assert_eq!(svc.columns.len(), 1);
        assert!(!svc.replace_columns);
        assert!(svc.order.is_empty());
        assert!(svc.widths.is_empty());
    }
}

// ---- theme system: hex colors, custom themes, save/load ----

use ratatui::style::Color as RColor;

/// parse "#rgb" / "#rrggbb" into a ratatui Rgb color
pub fn hex_to_color(s: &str) -> Option<RColor> {
    let s = s.trim().trim_start_matches('#');
    let (r, g, b) = match s.len() {
        3 => (
            u8::from_str_radix(&s[0..1].repeat(2), 16).ok()?,
            u8::from_str_radix(&s[1..2].repeat(2), 16).ok()?,
            u8::from_str_radix(&s[2..3].repeat(2), 16).ok()?,
        ),
        6 => (
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ),
        _ => return None,
    };
    Some(RColor::Rgb(r, g, b))
}

/// format a color as #rrggbb (named ANSI colors map to their closest hex)
pub fn color_to_hex(c: &Color) -> String {
    match c {
        RColor::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        RColor::Red => "#ff0000".into(),
        RColor::Green => "#00ff00".into(),
        RColor::Blue => "#0000ff".into(),
        RColor::Yellow => "#ffff00".into(),
        RColor::White => "#ffffff".into(),
        RColor::Black => "#000000".into(),
        RColor::Gray => "#808080".into(),
        RColor::DarkGray => "#555555".into(),
        RColor::LightRed => "#ff6666".into(),
        RColor::LightGreen => "#66ff66".into(),
        RColor::LightBlue => "#66b3ff".into(),
        RColor::Cyan => "#00ffff".into(),
        RColor::LightCyan => "#e0ffff".into(),
        RColor::Indexed(22) => "#005f00".into(),
        RColor::Indexed(i) => format!("#{i:02x}{i:02x}{i:02x}"),
        other => format!("{other:?}"),
    }
}

pub const THEME_FIELDS: &[&str] = &[
    "accent", "ok", "warn", "bad", "info", "dim", "header", "title", "bg_sel",
];

impl Theme {
    pub fn get_hex(&self, field: &str) -> Option<String> {
        Some(match field {
            "accent" => color_to_hex(&self.accent),
            "ok" => color_to_hex(&self.ok),
            "warn" => color_to_hex(&self.warn),
            "bad" => color_to_hex(&self.bad),
            "info" => color_to_hex(&self.info),
            "dim" => color_to_hex(&self.dim),
            "header" => color_to_hex(&self.header),
            "title" => color_to_hex(&self.title),
            "bg_sel" => color_to_hex(&self.bg_sel),
            _ => return None,
        })
    }
    pub fn set_hex(&mut self, field: &str, hex: &str) -> Option<()> {
        let c = hex_to_color(hex)?;
        match field {
            "accent" => self.accent = c,
            "ok" => self.ok = c,
            "warn" => self.warn = c,
            "bad" => self.bad = c,
            "info" => self.info = c,
            "dim" => self.dim = c,
            "header" => self.header = c,
            "title" => self.title = c,
            "bg_sel" => self.bg_sel = c,
            _ => return None,
        }
        Some(())
    }

    /// serialize all fields to YAML with hex strings
    pub fn to_yaml(&self) -> String {
        let f = |k: &str| self.get_hex(k).unwrap_or_default();
        THEME_FIELDS
            .iter()
            .map(|k| format!("{k}: \"{}\"", f(k)))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    pub fn from_yaml(txt: &str) -> Option<Self> {
        #[derive(serde::Deserialize)]
        struct Raw {
            accent: String,
            ok: String,
            warn: String,
            bad: String,
            info: String,
            dim: String,
            header: String,
            title: String,
            bg_sel: String,
        }
        let r: Raw = serde_yaml::from_str(txt).ok()?;
        let mut t = Self::resolve("dark");
        t.set_hex("accent", &r.accent)?;
        t.set_hex("ok", &r.ok)?;
        t.set_hex("warn", &r.warn)?;
        t.set_hex("bad", &r.bad)?;
        t.set_hex("info", &r.info)?;
        t.set_hex("dim", &r.dim)?;
        t.set_hex("header", &r.header)?;
        t.set_hex("title", &r.title)?;
        t.set_hex("bg_sel", &r.bg_sel)?;
        Some(t)
    }
}

fn themes_dir() -> PathBuf {
    cfg_dir().join("themes")
}

/// the canonical picker order: 4 presets + monochrome + custom
pub const THEME_MENU: &[&str] = &["matrix", "light", "dark", "neon", "monochrome", "custom"];

pub fn theme_label(name: &str) -> String {
    match name {
        "matrix" => "matrix (default)".to_string(),
        "mono" | "monochrome" => "monochrome".to_string(),
        "custom" => "custom \u{2014} editable".to_string(),
        other => other.to_string(),
    }
}

/// resolve by name: built-in preset or custom theme file
pub fn resolve_theme(name: &str) -> Option<Theme> {
    let builtin = matches!(name, "matrix" | "dark" | "light" | "neon")
        || name == "mono"
        || name == "monochrome";
    if builtin {
        return Some(Theme::resolve(if name == "monochrome" {
            "mono"
        } else {
            name
        }));
    }
    // custom themes: ~/.config/k9x/themes/<name>.yml (hex values)
    for ext in ["yml", "yaml"] {
        let p = themes_dir().join(format!("{name}.{ext}"));
        if let Ok(txt) = std::fs::read_to_string(&p)
            && let Some(t) = Theme::from_yaml(&txt)
        {
            return Some(t);
        }
    }
    None
}

/// persist a theme under a user-chosen name
pub fn save_theme_file(name: &str, t: &Theme) -> Result<PathBuf, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err("theme name may only contain letters, digits, - and _".into());
    }
    let p = themes_dir().join(format!("{name}.yml"));
    secure_write(&p, t.to_yaml()).map_err(|e| e.to_string())?;
    Ok(p)
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = hex_to_color("#00ff41").unwrap();
        assert_eq!(c, RColor::Rgb(0, 255, 65));
        assert_eq!(color_to_hex(&c), "#00ff41");
        assert!(hex_to_color("#0f0").is_some());
        assert!(hex_to_color("zzz").is_none());
    }

    #[test]
    fn theme_yaml_roundtrip() {
        let mut t = Theme::resolve("matrix");
        t.set_hex("bad", "#ff0000").unwrap();
        let y = t.to_yaml();
        let t2 = Theme::from_yaml(&y).unwrap();
        assert_eq!(t2.get_hex("bad").as_deref(), Some("#ff0000"));
        assert_eq!(t2.get_hex("ok"), t.get_hex("ok"));
        assert!(Theme::from_yaml("accent: nope").is_none());
    }

    #[test]
    fn set_get_all_fields() {
        let mut t = Theme::resolve("dark");
        for f in THEME_FIELDS {
            assert!(t.set_hex(f, "#123456").is_some(), "{f}");
            assert_eq!(t.get_hex(f).unwrap(), "#123456");
        }
        assert!(t.set_hex("nope", "#111111").is_none());
    }

    #[test]
    fn test_state_cfg_last_log_dir() {
        let mut st = StateCfg::default();
        assert_eq!(st.last_log_dir, "");
        st.last_log_dir = "/Users/dev/my-custom-logs".into();
        let serialized = toml::to_string(&st).unwrap();
        let deserialized: StateCfg = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.last_log_dir, "/Users/dev/my-custom-logs");
    }
}
