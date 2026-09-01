use crate::cfg::{Plugin, Theme};
use crate::k8s::{self, Cluster, ClusterRes, ExecCtl, LogMsg, LogWindow, Msg, PfEntry};
use crate::model::{ColSrc, KindSpec, Row, spec_for};
use anyhow::{Result, anyhow};
use kube::core::ApiResource;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ViewKind {
    Table,
    Pulse,
    Pf,
}

#[derive(Clone)]
pub enum LogSource {
    Single {
        ns: String,
        pod: String,
        container: Option<String>,
    },
    Multi {
        ns: String,
        selector: String,
    },
}

pub enum MenuPurpose {
    Containers {
        pod: String,
    },
    ContainerAction {
        pod: String,
        container: String,
    },
    Actions {
        kind: String,
        name: String,
    },
    Contexts,
    Namespaces,
    Shell(String),
    Logs(String),
    BrowseCrds,
    PfPorts {
        ns: String,
        name: String,
    },
    /// :helm — pick a release
    HelmList {
        releases: Vec<(String, String)>,
    }, // (name, ns)
    /// release → revision history (items = revisions)
    HelmHistory {
        ns: String,
        name: String,
        revs: Vec<crate::k8s::HelmRev>,
    },
    /// :policy subject picker
    PolicySubjects,
    /// :dir local browser (items are paths)
    DirBrowse,
    /// :themes picker
    Themes,
}

pub struct MenuItem {
    pub label: String,
    pub value: String,
}

impl MenuItem {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
    pub sel: usize,
    pub purpose: MenuPurpose,
}

pub enum Action {
    RunPlugin2 {
        name: String,
    },
    Restart {
        name: String,
    },
    ScaleApply {
        name: String,
        replicas: i64,
    },
    ApplyEdit {
        spec: Box<KindSpec>,
        ns: Option<String>,
        name: String,
        yaml: String,
    },
    Delete {
        name: String,
        force: bool,
    },
    /// bulk variants operating on the marked row set
    DeleteMarked {
        names: Vec<String>,
        force: bool,
    },
    RestartMarked {
        names: Vec<String>,
    },
    HelmRollback {
        ns: String,
        name: String,
        revision: i64,
    },
    NodeShell {
        node: String,
    },
    ApplyFile {
        path: String,
    },
    ThemeKeep {
        name: String,
    },
    Uncordon {
        node: String,
    },
    Cordon {
        node: String,
    },
    Drain {
        node: String,
    },
    TriggerCj {
        cron: String,
    },
    ToggleSuspendCj {
        cron: String,
        to: bool,
    },
    SaveLogs {
        path: String,
        content: String,
        logs_state: Box<LogsState>,
    },
}

impl Clone for Action {
    fn clone(&self) -> Self {
        match self {
            Self::RunPlugin2 { name } => Self::RunPlugin2 { name: name.clone() },
            Self::Restart { name } => Self::Restart { name: name.clone() },
            Self::ScaleApply { name, replicas } => Self::ScaleApply {
                name: name.clone(),
                replicas: *replicas,
            },
            Self::ApplyEdit {
                spec,
                ns,
                name,
                yaml,
            } => Self::ApplyEdit {
                spec: spec.clone(),
                ns: ns.clone(),
                name: name.clone(),
                yaml: yaml.clone(),
            },
            Self::Delete { name, force } => Self::Delete {
                name: name.clone(),
                force: *force,
            },
            Self::DeleteMarked { names, force } => Self::DeleteMarked {
                names: names.clone(),
                force: *force,
            },
            Self::RestartMarked { names } => Self::RestartMarked {
                names: names.clone(),
            },
            Self::HelmRollback { ns, name, revision } => Self::HelmRollback {
                ns: ns.clone(),
                name: name.clone(),
                revision: *revision,
            },
            Self::NodeShell { node } => Self::NodeShell { node: node.clone() },
            Self::ApplyFile { path } => Self::ApplyFile { path: path.clone() },
            Self::ThemeKeep { name } => Self::ThemeKeep { name: name.clone() },
            Self::Uncordon { node } => Self::Uncordon { node: node.clone() },
            Self::Cordon { node } => Self::Cordon { node: node.clone() },
            Self::Drain { node } => Self::Drain { node: node.clone() },
            Self::TriggerCj { cron } => Self::TriggerCj { cron: cron.clone() },
            Self::ToggleSuspendCj { cron, to } => Self::ToggleSuspendCj {
                cron: cron.clone(),
                to: *to,
            },
            Self::SaveLogs {
                path,
                content,
                logs_state,
            } => Self::SaveLogs {
                path: path.clone(),
                content: content.clone(),
                logs_state: Box::new(logs_state.clone_view()),
            },
        }
    }
}

pub enum InputPurpose {
    Scale { name: String },
    PfBind { ns: String, pod: String, port: u16 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SaveFocus {
    Directory,
    Filename,
    OkBtn,
    CancelBtn,
}

pub enum Mode {
    Normal,
    Cmd {
        buf: String,
        sel: usize,
    },
    Filter {
        buf: String,
    },
    Confirm {
        prompt: String,
        action: Action,
        sel_yes: bool,
    },
    Input {
        buf: String,
        purpose: InputPurpose,
    },
    Menu(Menu),
    TextPane {
        title: String,
        lines: Vec<String>,
        pos: usize,
        wrap: bool,
    },
    Logs(LogsState),
    Exec(ExecState),
    /// modal acknowledgement box (e.g. auth/token failure); OK may exit the app
    Notice {
        title: String,
        lines: Vec<String>,
        ok_exits: bool,
    },
    /// interactive custom-theme editor (only editable theme)
    ThemeEditor {
        values: Vec<(String, String)>,
        sel: usize,
        editing: bool,
        buf: String,
    },
    /// interactive log export dialog (directory + filename + suggestions + buttons)
    LogExport {
        dir_buf: String,
        file_buf: String,
        focus: SaveFocus,
        suggestions: Vec<String>,
        sug_idx: Option<usize>,
        sug_scroll: usize,
        logs_state: Box<LogsState>,
    },
    /// interactive port forward dialog (mirrors k9s <PortForward> dialog)
    PortForward(PfDialogState),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PfFocus {
    ContainerPort,
    LocalPort,
    Address,
    OkBtn,
    CancelBtn,
}

pub struct PfDialogState {
    pub ns: String,
    pub pod: String,
    pub ports: Vec<(String, u16, Option<String>)>,
    pub container_port: String,
    pub local_port: String,
    pub address: String,
    pub focus: PfFocus,
}

pub struct LogsState {
    pub source: LogSource,
    pub label: String,
    pub ns: String,
    pub pod: String,
    pub container: Option<String>,
    pub previous: bool,
    pub timestamps: bool,
    pub lines: Vec<String>,
    pub scroll_from_end: usize,
    pub wrap: bool,
    pub status: String,
    /// active search filter (case-insensitive substring)
    pub query: String,
    /// true while typing the search query
    pub search: bool,
    /// current fetch window (tail/head/since) — digits 0-6 switch it live
    pub window: LogWindow,
    pub handles: Vec<JoinHandle<()>>,
    pub rx: mpsc::UnboundedReceiver<LogMsg>,
    /// match navigation: index of current match in filtered lines or distinct occurrences (0-based)
    pub match_idx: Option<usize>,
    /// total matches for the current query
    pub match_total: usize,
    /// when true ('o' toggled), counts each individual occurrence on each line distinctly
    pub count_occurrences: bool,
}

impl LogsState {
    pub fn clone_view(&self) -> Self {
        Self {
            source: self.source.clone(),
            label: self.label.clone(),
            ns: self.ns.clone(),
            pod: self.pod.clone(),
            container: self.container.clone(),
            previous: self.previous,
            timestamps: self.timestamps,
            lines: self.lines.clone(),
            scroll_from_end: self.scroll_from_end,
            wrap: self.wrap,
            status: self.status.clone(),
            query: self.query.clone(),
            search: self.search,
            window: self.window,
            handles: vec![],
            rx: {
                let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
                rx
            },
            match_idx: self.match_idx,
            match_total: self.match_total,
            count_occurrences: self.count_occurrences,
        }
    }
}

pub fn compute_log_matches(
    lines: &[String],
    query: &str,
    count_occurrences: bool,
) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    if query.is_empty() {
        return matches;
    }
    let ql = query.to_lowercase();
    let mut filtered_idx = 0;
    for line in lines {
        let low = line.to_lowercase();
        if !low.contains(&ql) {
            continue;
        }
        if !count_occurrences {
            matches.push((filtered_idx, usize::MAX));
        } else {
            let mut occ = 0;
            let mut i = 0;
            while let Some(pos) = low[i..].find(&ql) {
                let start = i + pos;
                let end = start + ql.len();
                if !low.is_char_boundary(start) || !low.is_char_boundary(end) {
                    break;
                }
                matches.push((filtered_idx, occ));
                occ += 1;
                i = end;
            }
        }
        filtered_idx += 1;
    }
    matches
}

/// Adjusts `scroll_from_end` so `target_line_idx` within `total_lines` is visible
/// inside a viewport of height `inner_h`.
///
/// In k9x, the viewport displays lines `[start .. end)` where:
/// - `scroll = scroll_from_end.min(total_lines.saturating_sub(1))`
/// - `end = total_lines.saturating_sub(scroll)`
/// - `start = end.saturating_sub(inner_h)`
pub fn ensure_log_line_visible(
    scroll_from_end: &mut usize,
    total_lines: usize,
    target_line_idx: usize,
    inner_h: usize,
) {
    if total_lines == 0 || inner_h == 0 {
        *scroll_from_end = 0;
        return;
    }
    let inner_h = inner_h.max(1);
    let scroll = (*scroll_from_end).min(total_lines.saturating_sub(1));
    let end = total_lines.saturating_sub(scroll);
    let start = end.saturating_sub(inner_h);

    if target_line_idx < start {
        // Target is above visible viewport -> scroll up so target is at the top of the visible window
        *scroll_from_end = total_lines.saturating_sub(target_line_idx + inner_h);
    } else if target_line_idx >= end {
        // Target is below visible viewport -> scroll down so target is at the bottom of the visible window
        *scroll_from_end = total_lines.saturating_sub(target_line_idx + 1);
    }
    // If start <= target_line_idx < end, it is already visible; do not jump
}

pub struct ExecState {
    pub pod: String,
    /// when this exec is a node shell, the ephemeral pod to clean up on detach
    pub node_pod: Option<(String, String)>,
    pub out_rx: mpsc::UnboundedReceiver<Result<Vec<u8>, String>>,
    pub ctl_tx: mpsc::UnboundedSender<ExecCtl>,
    pub size_tx: Option<futures::channel::mpsc::Sender<kube::api::TerminalSize>>,
    pub buffer: Vec<u8>,
    pub status: String,
}

pub type PulseCounts = Arc<Mutex<BTreeMap<String, (usize, usize)>>>;

/// the 12 pulse cards in display order: (label, resource alias)
pub const PULSE_CARDS: &[(&str, &str)] = &[
    ("Pods", "po"),
    ("Deployments", "deploy"),
    ("Statefulsets", "sts"),
    ("Daemonsets", "ds"),
    ("Jobs", "job"),
    ("Cronjobs", "cj"),
    ("Persistentvolumes", "pv"),
    ("Persistentvolumeclaims", "pvc"),
    ("Horizontalpodautoscalers", "hpa"),
    ("Ingresses", "ing"),
    ("Networkpolicies", "netpol"),
    ("Serviceaccounts", "sa"),
];

/// one historical cluster-load sample for the pulse charts
#[derive(Clone, Copy, Debug)]
pub struct PulseSample {
    pub ts: chrono::DateTime<chrono::Utc>,
    /// cluster-wide (node totals)
    pub cpu_m: Option<f64>,
    pub cpu_cap_m: f64,
    pub mem_b: Option<u64>,
    pub mem_cap_b: u64,
    /// namespace-scoped pod sums (None when metrics-server absent / all-ns mode)
    pub ns_cpu_m: Option<f64>,
    pub ns_mem_b: Option<u64>,
}
pub type PulseHistory = Arc<Mutex<std::collections::VecDeque<PulseSample>>>;

pub struct App {
    pub cluster: Arc<Cluster>,
    pub ro: bool,
    pub theme: Theme,
    pub log_cap: usize,
    pub log_tail: i64,
    pub view_spec: Option<KindSpec>,
    pub rows: BTreeMap<String, (Row, std::time::Instant)>,
    pub watch: Option<JoinHandle<()>>,
    pub tx: mpsc::UnboundedSender<Msg>,
    pub rx: mpsc::UnboundedReceiver<Msg>,
    pub filter: String,
    pub sort_col: usize,
    pub sort_desc: bool,
    pub sel_key: Option<String>,
    pub all_ns: bool,
    pub ns: String,
    pub mode: Mode,
    pub view: ViewKind,
    pub status: String,
    pub drill_selector: Option<String>,
    pub drill_title: Option<String>,
    pub pfs: Vec<PfEntry>,
    pub pf_sel: usize,
    pub plugins: Vec<(String, Plugin)>,
    pub hotkeys: Vec<(String, crate::cfg::HotKey)>,
    pub custom_aliases: Vec<(String, String)>,
    pub known_crds: Vec<k8s::CrdInfo>,
    pub ns_shortcuts: Vec<String>,
    pub pulse_counts: PulseCounts,
    pub pulse_handles: Vec<JoinHandle<()>>,
    /// selected card index 0..=11
    pub pulse_sel: usize,
    /// rolling cluster cpu/mem history for the pulse charts
    pub pulse_hist: PulseHistory,
    pub quit: bool,
    pub pending_ctx: Option<String>,
    // geometry stash used by the renderer (click targets / scroll)
    pub ui_body: Option<ratatui::layout::Rect>,
    pub ui_header: Option<ratatui::layout::Rect>,
    pub ui_col_starts: Vec<u16>,
    pub ui_row_keys: Vec<String>,
    pub ui_toffset: usize,
    /// clickable rect for the confirm dialog (button row)
    pub ui_confirm_btn: Option<(ratatui::layout::Rect, u16)>, // (row rect, mid-x)
    /// clickable rect for notice box
    pub ui_notice_rect: Option<ratatui::layout::Rect>,
    pub t0: std::time::Instant,
    pub first_frame_ms: Option<u128>,
    pub first_data_ms: Option<u128>,
    /// apiserver version string, e.g. "v1.33.1"
    pub k8s_version: Option<String>,
    /// support windows for the running version (AWS/EKS data or upstream estimate)
    pub sup_dates: Option<crate::awsup::SupDates>,
    /// latest cluster-wide cpu/mem sample (incl. per-pod usage)
    pub cluster_res: Option<ClusterRes>,
    /// warn/crit percentages for load + node usage coloring
    pub thresholds: (u16, u16, u16, u16), // cpu_warn, cpu_crit, mem_warn, mem_crit
    /// custom jump rules from jumps.yml
    pub jumps: Vec<crate::cfg::Jump>,
    /// optional per-context defaults from contexts.yml
    pub ctx_defaults: std::collections::BTreeMap<String, crate::cfg::CtxDefaults>,
    /// name of the currently applied theme
    pub theme_name: String,
    /// colors to restore if a theme preview times out / is rejected
    pub prev_theme: Option<Theme>,
    pub prev_theme_name: Option<String>,
    /// when set, theme preview auto-reverts at this instant
    pub theme_deadline: Option<std::time::Instant>,
    /// debounce so expired-token modal shows once per session
    pub auth_notice_shown: bool,
    /// marked rows for bulk actions (Table view)
    pub marks: std::collections::BTreeSet<String>,
    /// views.yml custom column overrides
    pub views_cfg: std::collections::BTreeMap<String, crate::cfg::ViewOverride>,
    /// shared namespace scope for the background metrics sampler
    pub scope: Arc<k8s::ScopeSync>,
    // k9s-parity display toggles (set from CLI flags)
    pub ui_headless: bool,
    pub ui_logoless: bool,
    pub ui_crumbsless: bool,
}

impl App {
    pub async fn new(
        cluster: Arc<Cluster>,
        ns_override: Option<String>,
        all_ns: bool,
        ro: bool,
        theme: Theme,
        log_cap: usize,
        log_tail: i64,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let ns = ns_override
            .or_else(|| cluster.default_namespace())
            .unwrap_or_else(|| "default".into());
        let ns_c = ns.clone();
        Ok(Self {
            cluster,
            ro,
            theme,
            log_cap,
            log_tail,
            view_spec: None,
            rows: Default::default(),
            watch: None,
            tx,
            rx,
            filter: String::new(),
            sort_col: 0,
            sort_desc: false,
            sel_key: None,
            all_ns,
            ns,
            mode: Mode::Normal,
            view: ViewKind::Table,
            status: String::new(),
            drill_selector: None,
            drill_title: None,
            pfs: vec![],
            pf_sel: 0,
            plugins: crate::cfg::load_plugins(),
            hotkeys: crate::cfg::load_hotkeys(),
            custom_aliases: crate::cfg::load_aliases(),
            known_crds: vec![],
            ns_shortcuts: vec![],
            pulse_counts: Arc::new(Mutex::new(Default::default())),
            pulse_handles: vec![],
            pulse_sel: 0,
            pulse_hist: Arc::new(Mutex::new(Default::default())),
            quit: false,
            pending_ctx: None,
            ui_body: None,
            ui_header: None,
            ui_col_starts: vec![],
            ui_row_keys: vec![],
            ui_toffset: 0,
            ui_confirm_btn: None,
            ui_notice_rect: None,
            t0: std::time::Instant::now(),
            first_frame_ms: None,
            first_data_ms: None,
            k8s_version: None,
            sup_dates: None,
            cluster_res: None,
            marks: Default::default(),
            views_cfg: crate::cfg::load_views(),
            thresholds: {
                let t = &crate::cfg::FileCfg::load().thresholds;
                (t.cpu_warn, t.cpu_crit, t.mem_warn, t.mem_crit)
            },
            jumps: crate::cfg::load_jumps(),
            ctx_defaults: crate::cfg::load_contexts(),
            theme_name: crate::cfg::FileCfg::load().theme,
            prev_theme: None,
            prev_theme_name: None,
            theme_deadline: None,
            auth_notice_shown: false,
            ui_headless: false,
            ui_logoless: false,
            ui_crumbsless: false,
            scope: Arc::new(k8s::ScopeSync {
                all: std::sync::atomic::AtomicBool::new(all_ns),
                ns: std::sync::Mutex::new(ns_c.clone()),
            }),
        })
    }

    /// apply views.yml column overrides (match by alias, plural, or kind)
    pub fn apply_view_override(&self, spec: &mut KindSpec) {
        let key = |s: &str| s.to_lowercase();
        let o = self
            .views_cfg
            .iter()
            .find(|(k, _)| {
                **k == spec.alias || key(k) == key(&spec.plural) || key(k) == key(&spec.kind)
            })
            .map(|(_, v)| v.clone());
        if let Some(o) = o {
            if o.columns.is_empty() {
                return;
            }
            let extra = o
                .columns
                .iter()
                .map(|cd| KindSpec::dyn_col(&cd.name, &cd.path))
                .collect::<Vec<_>>();
            if o.replace_columns {
                spec.cols = extra;
            } else {
                spec.cols.extend(extra);
            }
        }
    }

    /// toggle the mark on a row; returns true when now marked
    pub fn toggle_mark(&mut self, key: &str) -> bool {
        if self.marks.remove(key) {
            false
        } else {
            self.marks.insert(key.to_string());
            true
        }
    }

    /// mark the span between the last marked row and `key` in current sort order
    pub fn span_mark_to(&mut self, key: &str) -> usize {
        let order: Vec<String> = self.filtered_sorted().into_iter().map(|r| r.key).collect();
        let Some(cur) = order.iter().position(|k| k == key) else {
            return 0;
        };
        let last_marked = self
            .marks
            .iter()
            .filter_map(|m| order.iter().position(|k| k == m))
            .min();
        let (a, b) = match last_marked {
            Some(p) if p <= cur => (p, cur),
            Some(p) => (cur, p),
            None => (cur, cur),
        };
        let mut added = 0;
        for k in &order[a..=b] {
            if self.marks.insert(k.clone()) {
                added += 1;
            }
        }
        added
    }

    /// returns marked keys that are present in the current active view rows
    pub fn marked_active_keys(&self) -> Vec<String> {
        self.marks
            .iter()
            .filter(|k| self.rows.contains_key(*k))
            .cloned()
            .collect()
    }

    pub fn switch_view(&mut self, alias: &str) -> Result<()> {
        let mut spec =
            spec_for(alias).ok_or_else(|| anyhow!("unknown resource '{alias}' (?=help)"))?;
        self.apply_view_override(&mut spec);
        self.stop_watch();
        self.rows.clear();
        self.marks.clear();
        self.sel_key = None;
        self.drill_selector = None;
        self.drill_title = None;
        self.filter.clear();
        self.sort_col = 0;
        self.sort_desc = false;
        self.view = ViewKind::Table;
        self.view_spec = Some(spec.clone());
        let ns = self.effective_ns(&spec);
        self.watch = Some(k8s::spawn_watch(
            self.cluster.clone(),
            spec.clone(),
            ns,
            None,
            self.tx.clone(),
        ));
        self.status = format!("loading {}…", spec.plural);
        Ok(())
    }

    pub async fn drill_to_pods(&mut self, row_key: &str) -> Result<()> {
        let cur = self.view_spec.clone().ok_or_else(|| anyhow!("no view"))?;
        let owner_ns = self
            .target_ns(row_key)
            .ok_or_else(|| anyhow!("cannot locate {row_key} (still loading?)"))?;
        let pure = self.pure_name(row_key);
        let obj = self
            .cluster
            .dyn_api(&cur, Some(&owner_ns))
            .get(pure)
            .await?;
        let val = serde_json::to_value(&obj)?;
        let pairs = crate::model::selector_labels(&val);
        if pairs.is_empty() {
            return Err(anyhow!("no label selector on {}/{}", cur.kind, pure));
        }
        let sel = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        self.stop_watch();
        self.rows.clear();
        self.marks.clear();
        self.sel_key = None;
        self.drill_selector = Some(sel);
        self.drill_title = Some(format!("{}/{}", cur.kind.to_lowercase(), pure));
        let pod_spec = spec_for("po").unwrap();
        self.view_spec = Some(pod_spec.clone());
        self.watch = Some(k8s::spawn_watch(
            self.cluster.clone(),
            pod_spec,
            Some(owner_ns.clone()),
            self.drill_selector.clone(),
            self.tx.clone(),
        ));
        self.set_status(format!("drilled into pods of {pure} (ns {owner_ns})"));
        Ok(())
    }

    pub async fn browse_crds(&mut self) {
        match k8s::list_crds(&self.cluster).await {
            Ok(crds) => {
                self.known_crds = crds.clone();
                let items: Vec<MenuItem> = crds
                    .iter()
                    .map(|(pl, g, ver, kind, nsd)| {
                        let label = if g.is_empty() {
                            kind.clone()
                        } else {
                            format!("{kind} ({g})")
                        };
                        let scope = if *nsd { "ns" } else { "cluster" };
                        MenuItem::new(label, format!("{pl}|{g}|{ver}|{kind}|{scope}"))
                    })
                    .collect();
                self.mode = Mode::Menu(Menu {
                    title: "custom resources".into(),
                    items,
                    sel: 0,
                    purpose: MenuPurpose::BrowseCrds,
                });
            }
            Err(e) => self.err_status(e),
        }
    }

    pub fn browse_custom(&mut self, value: &str) {
        let parts: Vec<&str> = value.split('|').collect();
        if parts.len() != 5 {
            return;
        }
        let (pl, g, ver, kind, nsd) = (
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
            parts[3].to_string(),
            parts[4] == "ns",
        );
        let mut spec = crate::model::custom_spec(&pl, &g, &ver, &kind, nsd);
        self.apply_view_override(&mut spec);
        self.stop_watch();
        self.rows.clear();
        self.sel_key = None;
        self.drill_selector = None;
        self.drill_title = None;
        self.view_spec = Some(spec.clone());
        self.view = ViewKind::Table;
        let ns = self.effective_ns(&spec);
        self.watch = Some(k8s::spawn_watch(
            self.cluster.clone(),
            spec,
            ns,
            None,
            self.tx.clone(),
        ));
        self.set_status(format!("watching custom resource {kind}"));
    }

    pub fn effective_ns(&self, spec: &KindSpec) -> Option<String> {
        if !spec.namespaced || self.all_ns {
            None
        } else {
            Some(self.ns.clone())
        }
    }

    pub fn effective_ns_for_action(&self) -> Option<String> {
        if self.all_ns {
            None
        } else {
            Some(self.ns.clone())
        }
    }

    pub fn target_ns(&self, key: &str) -> Option<String> {
        let Some(spec) = &self.view_spec else {
            return Some(self.ns.clone());
        };
        if !spec.namespaced {
            return None;
        }
        if let Some((ns, _)) = key.split_once('/') {
            return Some(ns.to_string());
        }
        if !self.all_ns {
            return Some(self.ns.clone());
        }
        self.rows.get(key).and_then(|(r, _)| {
            if r.ns.is_empty() {
                None
            } else {
                Some(r.ns.clone())
            }
        })
    }

    pub fn pure_name<'a>(&self, key: &'a str) -> &'a str {
        key.split_once('/').map(|(_, n)| n).unwrap_or(key)
    }

    pub fn row_flags(&self, name: &str) -> u8 {
        self.rows.get(name).map(|(r, _)| r.flags).unwrap_or(0)
    }

    pub fn use_namespace(&mut self, ns: &str) {
        let ctx = self.cluster.ctx_name.clone();
        self.ns = ns.to_string();
        self.all_ns = false;
        self.restart_watch_full_reset();
        self.refresh_pulse_if_active();
        let mut st = crate::cfg::StateCfg::load();
        st.remember_ns(&ctx, ns);
        st.save();
    }

    pub fn stop_watch(&mut self) {
        if let Some(h) = self.watch.take() {
            h.abort();
        }
    }

    pub fn restart_watch(&mut self) {
        if let Some(spec) = self.view_spec.clone() {
            let sel = self.drill_selector.clone();
            self.stop_watch();
            let ns = self.effective_ns(&spec);
            self.watch = Some(k8s::spawn_watch(
                self.cluster.clone(),
                spec,
                ns,
                sel,
                self.tx.clone(),
            ));
        }
    }

    pub fn restart_watch_full_reset(&mut self) {
        self.rows.clear();
        self.marks.clear();
        self.sel_key = None;
        self.restart_watch();
    }

    pub fn restart_watch_if_pulse_left(&mut self) {
        if self.view == ViewKind::Table && self.watch.is_none() {
            self.restart_watch();
        }
    }

    pub fn cols_count(&self) -> usize {
        self.cols_len()
    }
    fn cols_len(&self) -> usize {
        self.view_spec.as_ref().map(|s| s.cols.len()).unwrap_or(1)
    }

    /// rows in current filter+sort order, with live metric cells patched in.
    /// returns owned Rows so callers can decorate freely.
    pub fn filtered_sorted(&self) -> Vec<Row> {
        let mut v: Vec<Row> = if self.filter.is_empty() {
            self.rows.values().map(|(r, _)| r.clone()).collect()
        } else {
            let fl = self.filter.to_lowercase();
            self.rows
                .values()
                .filter(|(r, _)| {
                    contains_ignore_case(&r.key, &fl)
                        || r.cells.iter().any(|c| contains_ignore_case(c, &fl))
                        || fuzzy_match_multi(&r.key, &r.cells, &fl)
                })
                .map(|(r, _)| r.clone())
                .collect()
        };
        // patch live CPU/MEM cells from the latest metrics sample
        if let Some(spec) = &self.view_spec {
            if spec.kind == "Node"
                && let Some(res) = &self.cluster_res
            {
                let (ci, mi) = (
                    spec.cols.iter().position(|c| c.src == ColSrc::NodeCpuPct),
                    spec.cols.iter().position(|c| c.src == ColSrc::NodeMemPct),
                );
                for r in v.iter_mut() {
                    if let Some(u) = res.nodes.get(&r.key) {
                        if let Some(i) = ci {
                            r.cells[i] = match (u.cpu_m, u.cpu_cap_m > 0.0) {
                                (Some(cm), true) => {
                                    format!("{}%", (cm / u.cpu_cap_m * 100.0).round() as u64)
                                }
                                _ => "-".into(),
                            };
                        }
                        if let Some(i) = mi {
                            r.cells[i] = match (u.mem_b, u.mem_cap_b > 0) {
                                (Some(mb), true) => format!(
                                    "{}%",
                                    (mb as f64 / u.mem_cap_b as f64 * 100.0).round() as u64
                                ),
                                _ => "-".into(),
                            };
                        }
                    }
                }
            }
            let (ci, mi) = crate::model::metric_cols(spec);
            if ci.is_some() || mi.is_some() {
                for r in v.iter_mut() {
                    if let Some(u) = self
                        .cluster_res
                        .as_ref()
                        .and_then(|c| c.pods.get(&format!("{}/{}", r.ns, r.key)))
                    {
                        if let Some(i) = ci {
                            r.cells[i] = crate::model::fmt_cpu_m(u.cpu_m);
                        }
                        if let Some(i) = mi {
                            r.cells[i] = crate::model::fmt_mem_mi(u.mem_b);
                        }
                    }
                }
            }
        }
        let col = self.sort_col.min(self.cols_len().saturating_sub(1));
        v.sort_by(|a, b| {
            let ka = a.cells.get(col).map(|s| s.as_str()).unwrap_or("");
            let kb = b.cells.get(col).map(|s| s.as_str()).unwrap_or("");
            let ord = natural_compare(ka, kb);
            if self.sort_desc { ord.reverse() } else { ord }
        });
        v
    }

    pub fn selected_row(&self) -> Option<Row> {
        let key = self.sel_key.as_ref()?;
        self.filtered_sorted().into_iter().find(|r| r.key == *key)
    }

    pub fn selected_or_first(&mut self) -> Option<String> {
        if self.sel_key.is_none() {
            self.sel_top();
        }
        self.sel_key.clone()
    }

    pub fn move_sel(&mut self, delta: i32) {
        let v = self.filtered_sorted();
        if v.is_empty() {
            return;
        }
        let cur = self
            .sel_key
            .as_ref()
            .and_then(|k| v.iter().position(|r| r.key == *k))
            .unwrap_or(0);
        let next = (cur as i32 + delta).clamp(0, v.len() as i32 - 1) as usize;
        self.sel_key = Some(v[next].key.clone());
    }

    pub fn sel_top(&mut self) {
        if let Some(r) = self.filtered_sorted().first() {
            self.sel_key = Some(r.key.clone());
        }
    }
    pub fn sel_bottom(&mut self) {
        if let Some(r) = self.filtered_sorted().last() {
            self.sel_key = Some(r.key.clone());
        }
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
    }

    /// token-expiry modal: shown once, OK exits gracefully.
    /// plain status text is used for any later repeats.
    pub fn maybe_auth_notice(&mut self, e: &str) {
        if self.auth_notice_shown || !k8s::is_auth_expired(e) {
            return;
        }
        self.auth_notice_shown = true;
        let ctx = &self.cluster.ctx_name;
        self.mode = Mode::Notice {
            title: "token expired".into(),
            lines: vec![
                format!("authentication failed for context \u{2018}{ctx}\u{2019}."),
                "your SSO/IAM token has expired or been revoked.".to_string(),
                String::new(),
                "re-authenticate (e.g. `aws sso login`),".to_string(),
                "then press enter to exit k9x and relaunch.".into(),
            ],
            ok_exits: true,
        };
        self.set_status("");
    }

    pub fn err_status(&mut self, e: impl std::fmt::Display) {
        self.maybe_auth_notice(&e.to_string());
        if matches!(self.mode, Mode::Notice { .. }) {
            return;
        }
        self.status = self.err_status_text(&e.to_string());
    }

    /// flat, logically ordered hint list for the top-right shortcuts grid;
    /// the renderer chunks it row-major so every row has the same cell count
    pub fn context_hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.view != ViewKind::Table {
            return vec![];
        }
        let Some(spec) = &self.view_spec else {
            return vec![];
        };
        let sel_flags = self
            .sel_key
            .as_deref()
            .map(|k| self.row_flags(k))
            .unwrap_or(0);
        let mut h: Vec<(&'static str, &'static str)> = vec![("enter", "actions")];
        match spec.kind.as_str() {
            "Pod" => {
                h.push(("l", "logs"));
                h.push(("s", "shell"));
                h.push(("a", "attach"));
                h.push(("p", "pf"));
            }
            "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" | "Job" => {
                h.push(("l", "logs(all)"));
                h.push(("R", "restart"));
                h.push(("S", "scale"));
            }
            "Node" => {
                h.push(("s", "node-shell"));
                if sel_flags & crate::model::FLAG_CORDONED != 0 {
                    h.push(("u", "uncordon"));
                } else {
                    h.push(("c", "cordon"));
                }
                h.push(("D", "drain"));
            }
            "CronJob" => {
                h.push(("t", "trigger"));
                if sel_flags & crate::model::FLAG_SUSPENDED != 0 {
                    h.push(("x", "resume"));
                } else {
                    h.push(("x", "suspend"));
                }
            }
            "Secret" => h.push(("X", "decode")),
            _ => {}
        }
        h.push(("d", "describe"));
        h.push(("y", "yaml"));
        h.push(("e", "edit"));
        h.push(("ctrl-d", "delete"));
        h.push(("ctrl-k", "kill"));
        if matches!(spec.kind.as_str(), "Secret" | "ConfigMap") {
            h.push(("U", "used-by"));
        }
        h.push(("space", "mark"));
        h.push((":", "cmd"));
        h.push(("?", "help"));
        h.push(("T", "themes"));
        h.push(("ctrl-q", "exit"));
        h
    }

    pub fn err_status_text(&self, e: &str) -> String {
        // permission/unreachable classification first — nicest possible message
        if let Some(nice) = k8s::classify_err(e) {
            return format!("!{nice}");
        }
        if e.contains("404") || e.contains("not found") || e.contains("NotFound") {
            return if !self.rows.is_empty() {
                "!gone \u{2014} object deleted/recreated; press r to refresh".into()
            } else {
                "!not found \u{2014} check name/ns ('r' reloads)".into()
            };
        }
        if e.contains("401") || e.contains("Unauthorized") || e.to_lowercase().contains("token") {
            return "!unauthorized \u{2014} credentials expired; ':ctx' to reconnect".into();
        }
        format!("!{e}")
    }

    pub fn apply_msg(&mut self, m: Msg) {
        match m {
            Msg::Reset => {
                if self.status.starts_with("loading") {
                    let plural = self
                        .view_spec
                        .as_ref()
                        .map(|s| s.plural.clone())
                        .unwrap_or_default();
                    if self.rows.is_empty() {
                        self.status = format!("no {plural} found in scope");
                    } else {
                        self.status.clear();
                    }
                }
            }
            Msg::Up(row) => {
                if self.first_data_ms.is_none() {
                    self.first_data_ms = Some(self.t0.elapsed().as_millis());
                    self.profile_write("first-data");
                }
                self.rows
                    .insert(row.key.clone(), (row, std::time::Instant::now()));
                if self.status.starts_with("loading") || self.status.starts_with("no ") {
                    self.status.clear();
                }
            }
            Msg::Down(key) => {
                self.rows.remove(&key);
                self.marks.remove(&key);
            }
            Msg::Err(e) => {
                self.maybe_auth_notice(&e);
                if !matches!(self.mode, Mode::Notice { .. }) {
                    self.status = self.err_status_text(&e);
                }
            }
            Msg::Status(stext) => self.status = stext,
            Msg::Pane { title, lines, wrap } => {
                self.mode = Mode::TextPane {
                    title,
                    lines,
                    pos: 0,
                    wrap,
                };
            }
            Msg::Crds(crds) => {
                self.known_crds = crds;
            }
            Msg::Nss(mut nss) => {
                nss.truncate(9);
                self.ns_shortcuts = nss;
            }
            Msg::Ver(v) => {
                self.k8s_version = Some(v.clone());
                let cl2 = self.cluster.clone();
                let tx2 = self.tx.clone();
                tokio::spawn(async move {
                    if let Some(sd) = crate::awsup::resolve(&cl2, &v).await {
                        let _ = tx2.send(Msg::Sup(sd));
                    }
                });
            }
            Msg::Sup(sd) => {
                self.sup_dates = Some(sd);
            }
            Msg::Res(r) => {
                self.cluster_res = Some(r.clone());
                // namespace-scoped pod sums for the pulse charts
                let mut ns_cpu = 0.0f64;
                let mut ns_mem = 0u64;
                for pu in r.pods.values() {
                    ns_cpu += pu.cpu_m;
                    ns_mem += pu.mem_b;
                }
                let ns_cpu_opt = if r.pods.is_empty() {
                    None
                } else {
                    Some(ns_cpu)
                };
                let ns_mem_opt = if r.pods.is_empty() {
                    None
                } else {
                    Some(ns_mem)
                };
                let mut h = self.pulse_hist.lock().unwrap();
                h.push_back(PulseSample {
                    ts: chrono::Utc::now(),
                    cpu_m: r.cpu_used_m,
                    cpu_cap_m: r.cpu_cap_m,
                    mem_b: r.mem_used,
                    mem_cap_b: r.mem_cap,
                    ns_cpu_m: ns_cpu_opt,
                    ns_mem_b: ns_mem_opt,
                });
                while h.len() > 60 {
                    h.pop_front();
                }
            }
        }
    }

    pub fn profile_write(&self, stage: &str) {
        if std::env::var("K9X_PROFILE").is_ok() {
            let ms = self.t0.elapsed().as_millis();
            let ff = self
                .first_frame_ms
                .map(|x| x.to_string())
                .unwrap_or("-".into());
            let fd = self
                .first_data_ms
                .map(|x| x.to_string())
                .unwrap_or("-".into());
            let _ = std::fs::write(
                "/tmp/k9x-profile.txt",
                format!("t={ms}ms stage={stage} first_frame={ff}ms first_data={fd}ms\n"),
            );
        }
    }
}

impl App {
    pub fn open_menu_contexts(&mut self) {
        let items: Vec<MenuItem> = self
            .cluster
            .contexts
            .iter()
            .map(|c| {
                MenuItem::new(
                    if *c == self.cluster.ctx_name {
                        format!("* {c}")
                    } else {
                        c.clone()
                    },
                    c.clone(),
                )
            })
            .collect();
        self.mode = Mode::Menu(Menu {
            title: "contexts".into(),
            items,
            sel: 0,
            purpose: MenuPurpose::Contexts,
        });
    }

    pub async fn open_menu_namespaces(&mut self) {
        match k8s::list_namespaces(&self.cluster).await {
            Ok(nss) => {
                let items: Vec<MenuItem> = nss
                    .iter()
                    .map(|n| MenuItem::new(n.clone(), n.clone()))
                    .collect();
                self.mode = Mode::Menu(Menu {
                    title: "namespaces".into(),
                    items,
                    sel: 0,
                    purpose: MenuPurpose::Namespaces,
                });
            }
            Err(e) => self.err_status(e),
        }
    }

    pub fn menu_move(&mut self, d: i32) {
        if let Mode::Menu(m) = &mut self.mode
            && !m.items.is_empty()
        {
            m.sel = ((m.sel as i32 + d).rem_euclid(m.items.len() as i32)) as usize;
        }
    }

    pub async fn menu_select(&mut self) {
        let taken = std::mem::replace(&mut self.mode, Mode::Normal);
        let Mode::Menu(m) = taken else { return };
        let value = m
            .items
            .get(m.sel)
            .map(|i| i.value.clone())
            .unwrap_or_default();
        match m.purpose {
            MenuPurpose::Contexts => self.pending_ctx = Some(value),
            MenuPurpose::Namespaces => {
                self.use_namespace(&value);
                self.set_status(format!("ns \u{2192} {}", self.ns));
            }
            MenuPurpose::Shell(pod) => {
                let sns = self.target_ns(&pod).unwrap_or_else(|| self.ns.clone());
                let pure = self.pure_name(&pod).to_string();
                self.start_exec(
                    sns,
                    pure,
                    Some(value),
                    vec![
                        "sh".into(),
                        "-c".into(),
                        "command -v bash >/dev/null 2>&1 && exec bash || exec sh".into(),
                    ],
                )
                .await;
            }
            MenuPurpose::Logs(pod) => {
                let lns = self.target_ns(&pod).unwrap_or_else(|| self.ns.clone());
                let pure = self.pure_name(&pod).to_string();
                self.open_logs_in(lns, pure, Some(value));
            }
            MenuPurpose::BrowseCrds => self.browse_custom(&value),
            MenuPurpose::PfPorts { ns, name } => {
                if let Ok(p) = value.parse::<u16>() {
                    self.mode = Mode::Input {
                        buf: "127.0.0.1:0".into(),
                        purpose: InputPurpose::PfBind {
                            ns,
                            pod: name,
                            port: p,
                        },
                    };
                }
            }
            MenuPurpose::Containers { pod } => self.open_container_action_menu(&pod, &value),
            MenuPurpose::ContainerAction { pod, container } => match value.as_str() {
                "logs" => {
                    let lns = self.target_ns(&pod).unwrap_or_else(|| self.ns.clone());
                    let pure = self.pure_name(&pod).to_string();
                    self.open_logs_in(lns, pure, Some(container));
                }
                "plogs" => {
                    let lns = self.target_ns(&pod).unwrap_or_else(|| self.ns.clone());
                    let pure = self.pure_name(&pod).to_string();
                    self.open_logs_in(lns.clone(), pure.clone(), Some(container.clone()));
                    if let Mode::Logs(st) = &mut self.mode {
                        st.previous = true;
                        self.restart_log_stream().await;
                    }
                }
                "attach" => {
                    let ans = self.target_ns(&pod).unwrap_or_else(|| self.ns.clone());
                    let pure = self.pure_name(&pod).to_string();
                    self.start_attach(ans, pure, Some(container)).await;
                }
                "stats" => self.open_container_stats(&pod, &container).await,
                "env" => self.open_container_env(&pod, &container).await,
                "shell" => {
                    let sns = self.target_ns(&pod).unwrap_or_else(|| self.ns.clone());
                    let pure = self.pure_name(&pod).to_string();
                    self.start_exec(
                        sns,
                        pure,
                        Some(container),
                        vec![
                            "sh".into(),
                            "-c".into(),
                            "command -v bash >/dev/null 2>&1 && exec bash || exec sh".into(),
                        ],
                    )
                    .await;
                }
                _ => {}
            },
            MenuPurpose::Actions { kind, name } => self.run_action(&kind, &name, &value).await,
            MenuPurpose::HelmList { releases: _ } => {
                if let Some((name, ns)) = value.split_once('|') {
                    let (name, ns) = (name.to_string(), ns.to_string());
                    self.open_helm_history_menu(ns, name).await;
                }
            }
            MenuPurpose::HelmHistory { .. } => self.helm_values_of_selected(),
            MenuPurpose::Themes => {
                if value == "custom" {
                    self.open_custom_theme_editor();
                } else if crate::cfg::resolve_theme(&value).is_some() {
                    // preset: live preview + keep/revert dialog (20s auto-revert)
                    let name = value.clone();
                    self.preview_theme(&name);
                    if !self.status.starts_with('!') {
                        confirm_pub(
                            self,
                            format!("keep theme '{}'? ", crate::cfg::theme_label(&name)),
                            Action::ThemeKeep { name },
                        );
                    }
                } else {
                    self.set_status(format!("!theme '{value}' unavailable"));
                }
            }
            MenuPurpose::DirBrowse => {
                let is_dir = std::fs::metadata(&value)
                    .map(|m| m.is_dir())
                    .unwrap_or(false);
                if is_dir {
                    Box::pin(self.open_dir(&value)).await;
                } else {
                    self.mode = Mode::Normal;
                    confirm_pub(
                        self,
                        format!("apply {}?", value.rsplit('/').next().unwrap_or(&value)),
                        Action::ApplyFile { path: value },
                    );
                }
            }
            MenuPurpose::PolicySubjects => {
                let (sns, sa) = match value.split_once('|') {
                    Some((a, b)) => (a.to_string(), b.to_string()),
                    None => (self.ns.clone(), value.clone()),
                };
                self.open_policy_for(sns, sa).await;
            }
        }
    }

    pub async fn run_action(&mut self, kind: &str, name: &str, action: &str) {
        let spec = self
            .view_spec
            .clone()
            .unwrap_or_else(|| spec_for("po").unwrap());
        let pure = self.pure_name(name);
        match action {
            "containers" => self.open_pod_containers_menu(pure).await,
            "logs" | "wlogs" => {
                if kind == "Pod" {
                    let lns = self.target_ns(name).unwrap_or_else(|| self.ns.clone());
                    self.open_logs_in(lns, pure.to_string(), None);
                } else {
                    let Some(own_ns) = self.target_ns(name) else {
                        self.set_status("!cannot resolve namespace");
                        return;
                    };
                    match workload_selector(self, &spec, pure).await {
                        Some(sel) => self.open_logs_multi(own_ns, sel).await,
                        None => self.set_status("!no selector / running pods"),
                    }
                }
            }
            "attach" => {
                let ans = self.target_ns(name).unwrap_or_else(|| self.ns.clone());
                self.start_attach(ans, pure.to_string(), None).await;
            }
            "shell" => {
                let sns = self.target_ns(name).unwrap_or_else(|| self.ns.clone());
                self.start_exec(
                    sns,
                    pure.to_string(),
                    None,
                    vec![
                        "sh".into(),
                        "-c".into(),
                        "command -v bash >/dev/null 2>&1 && exec bash || exec sh".into(),
                    ],
                )
                .await;
            }
            "nodeshell" => confirm_pub(
                self,
                format!("spawn privileged shell pod on node {pure}?"),
                Action::NodeShell {
                    node: pure.to_string(),
                },
            ),
            "drill" => {
                if let Err(e) = self.drill_to_pods(name).await {
                    self.err_status(&e);
                }
            }
            "scale" => {
                self.mode = Mode::Input {
                    buf: String::new(),
                    purpose: InputPurpose::Scale {
                        name: name.to_string(),
                    },
                }
            }
            "restart" => do_restart_pub(self, &spec, name),
            "cordon" => confirm_pub(
                self,
                format!("cordon node {pure}?"),
                Action::Cordon {
                    node: pure.to_string(),
                },
            ),
            "uncordon" => confirm_pub(
                self,
                format!("uncordon node {pure}?"),
                Action::Uncordon {
                    node: pure.to_string(),
                },
            ),
            "drain" => confirm_pub(
                self,
                format!("DRAIN node {pure}? (cordons + evicts pods)"),
                Action::Drain {
                    node: pure.to_string(),
                },
            ),
            "trigger" => confirm_pub(
                self,
                format!("trigger job from cronjob/{pure}?"),
                Action::TriggerCj {
                    cron: name.to_string(),
                },
            ),
            "suspend" => {
                let to = fetch_suspend_state_value(self, name)
                    .await
                    .map(|v| !v)
                    .unwrap_or(true);
                confirm_pub(
                    self,
                    format!("set suspend={to} on cronjob/{pure}?"),
                    Action::ToggleSuspendCj {
                        cron: name.to_string(),
                        to,
                    },
                );
            }
            "pf" => {
                self.open_port_forward_dialog(
                    self.target_ns(name).unwrap_or_else(|| self.ns.clone()),
                    pure.to_string(),
                )
                .await
            }
            "svc_pf" => self.pf_for_service(pure).await,
            "decode" => decode_secret_pub(self, name),
            "describe" => describe_pub(self, &spec, name).await,
            "yaml" => yaml_pub(self, &spec, name).await,
            "edit" => edit_pub(self, &spec, name),
            "delete" => confirm_pub(
                self,
                format!("delete {kind}/{pure}?"),
                Action::Delete {
                    name: name.to_string(),
                    force: false,
                },
            ),
            "fdelete" => confirm_pub(
                self,
                format!("FORCE delete {kind}/{pure}?"),
                Action::Delete {
                    name: name.to_string(),
                    force: true,
                },
            ),
            "usedby" => self.open_used_by(kind == "Secret", name.to_string()).await,
            "ref" => {
                let k2 = kind.to_string();
                self.open_ref_pane(&k2);
            }
            "rules" => self.open_role_rules(kind, name).await,
            _ => {}
        }
    }

    pub async fn open_pod_containers_menu(&mut self, pod: &str) {
        let pns = self.target_ns(pod).unwrap_or_else(|| self.ns.clone());
        let pure = self.pure_name(pod);
        let containers = self.pod_containers(&pns, pure).await.unwrap_or_default();
        if containers.is_empty() {
            self.set_status("!pod not found or terminated");
            return;
        }
        let items: Vec<MenuItem> = containers
            .iter()
            .map(|c| MenuItem::new(c.clone(), c.clone()))
            .collect();
        self.mode = Mode::Menu(Menu {
            title: format!("{pure} \u{00b7} containers"),
            items,
            sel: 0,
            purpose: MenuPurpose::Containers {
                pod: pod.to_string(),
            },
        });
    }

    fn open_container_action_menu(&mut self, pod: &str, container: &str) {
        let pure = self.pure_name(pod);
        let items = vec![
            MenuItem::new("logs", "logs"),
            MenuItem::new("previous logs", "plogs"),
            MenuItem::new("shell", "shell"),
            MenuItem::new("attach", "attach"),
            MenuItem::new("stats (cpu/mem)", "stats"),
            MenuItem::new("env vars", "env"),
        ];
        self.mode = Mode::Menu(Menu {
            title: format!("{pure}/{container}"),
            items,
            sel: 0,
            purpose: MenuPurpose::ContainerAction {
                pod: pod.to_string(),
                container: container.to_string(),
            },
        });
    }

    /// on-demand per-container usage pane (single metrics GET — nothing polled)
    pub async fn open_container_stats(&mut self, pod: &str, container: &str) {
        let ns = self.target_ns(pod).unwrap_or_else(|| self.ns.clone());
        let pure = self.pure_name(pod);
        match k8s::pod_metrics_one(&self.cluster, &ns, pure).await {
            Ok(u) => {
                let mut lines = vec![
                    format!("pod {ns}/{pure}"),
                    format!(
                        "total cpu {} · mem {}",
                        crate::model::fmt_cpu_m(u.cpu_m),
                        crate::model::fmt_mem_mi(u.mem_b)
                    ),
                    String::new(),
                ];
                for (cn, cm, mb) in &u.containers {
                    let mark = if cn == container { "▶" } else { " " };
                    lines.push(format!(
                        "{mark} {:<24} cpu {:<8} mem {}",
                        cn,
                        crate::model::fmt_cpu_m(*cm),
                        crate::model::fmt_mem_mi(*mb)
                    ));
                }
                self.mode = Mode::TextPane {
                    title: format!("stats:{pure}/{container}"),
                    lines,
                    pos: 0,
                    wrap: false,
                };
            }
            Err(_) => self.set_status("!metrics-server unavailable for this cluster"),
        }
    }

    pub async fn open_pod_menu(&mut self, pod: &str, shell: bool) {
        let pns = self.target_ns(pod).unwrap_or_else(|| self.ns.clone());
        let pure = self.pure_name(pod);
        let containers = self.pod_containers(&pns, pure).await.unwrap_or_default();
        let purpose = if shell {
            MenuPurpose::Shell(pod.to_string())
        } else {
            MenuPurpose::Logs(pod.to_string())
        };
        let items: Vec<MenuItem> = containers
            .iter()
            .map(|c| MenuItem::new(c.clone(), c.clone()))
            .collect();
        if items.is_empty() {
            self.set_status("!pod not found or terminated");
            return;
        }
        if items.len() == 1 {
            let value = items[0].value.clone();
            drop(items);
            match purpose {
                MenuPurpose::Shell(_) => {
                    let sns = self.target_ns(pod).unwrap_or_else(|| self.ns.clone());
                    let pure_name = self.pure_name(pod).to_string();
                    self.start_exec(
                        sns,
                        pure_name,
                        Some(value),
                        vec![
                            "sh".into(),
                            "-c".into(),
                            "command -v bash >/dev/null 2>&1 && exec bash || exec sh".into(),
                        ],
                    )
                    .await;
                }
                _ => {
                    let lns = self.target_ns(pod).unwrap_or_else(|| self.ns.clone());
                    let pure_name = self.pure_name(pod).to_string();
                    self.open_logs_in(lns, pure_name, Some(value));
                }
            }
            return;
        }
        self.mode = Mode::Menu(Menu {
            title: format!("{pure} \u{00b7} containers"),
            items,
            sel: 0,
            purpose,
        });
    }

    async fn pod_containers(&self, ns: &str, pod: &str) -> Result<Vec<String>> {
        let gvk = ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk("", "v1", "Pod"));
        let api: kube::Api<kube::core::dynamic::DynamicObject> =
            kube::Api::namespaced_with(self.cluster.client.clone(), ns, &gvk);
        let obj = api.get(pod).await?;
        let v = serde_json::to_value(&obj)?;
        Ok(v.pointer("/spec/containers")
            .and_then(|c| c.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }
}

impl App {
    pub fn open_logs(&mut self, pod: String, container: Option<String>) {
        let ns = self.target_ns(&pod).unwrap_or_else(|| self.ns.clone());
        self.open_logs_in(ns, pod, container);
    }

    pub fn open_logs_in(&mut self, ns: String, pod: String, container: Option<String>) {
        let source = LogSource::Single {
            ns: ns.clone(),
            pod: pod.clone(),
            container: container.clone(),
        };
        let label = format!(
            "{}/{}{}",
            ns,
            pod,
            container
                .as_ref()
                .map(|c| format!(":{c}"))
                .unwrap_or_default()
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = k8s::spawn_logs(
            self.cluster.clone(),
            ns.clone(),
            pod.clone(),
            container.clone(),
            false,
            false,
            self.log_tail,
            tx,
        );
        self.mode = Mode::Logs(LogsState {
            source,
            label,
            ns: ns.clone(),
            pod,
            container,
            previous: false,
            timestamps: false,
            lines: vec![],
            scroll_from_end: 0,
            wrap: true,
            status: "connecting\u{2026}".into(),
            query: String::new(),
            search: false,
            window: LogWindow::Tail(self.log_tail),
            handles: vec![handle],
            rx,
            match_idx: None,
            match_total: 0,
            count_occurrences: false,
        });
    }

    pub async fn open_logs_multi(&mut self, ns: String, selector: String) {
        let gvk = ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk("", "v1", "Pod"));
        let api: kube::Api<kube::core::dynamic::DynamicObject> =
            kube::Api::namespaced_with(self.cluster.client.clone(), &ns, &gvk);
        let lp = kube::api::ListParams::default().labels(&selector);
        let mut targets: Vec<String> = vec![];
        if let Ok(list) = api.list(&lp).await {
            for p in list.items {
                let pv = match serde_json::to_value(&p) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if pv.pointer("/status/phase").and_then(|x| x.as_str()) == Some("Running")
                    && let Some(name) = p.metadata.name.clone()
                {
                    targets.push(name);
                }
            }
        }
        if targets.is_empty() {
            self.set_status("!no running pods for this workload");
            return;
        }
        targets.truncate(8);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut handles = vec![];
        for name in &targets {
            handles.push(k8s::spawn_logs_prefixed(
                self.cluster.clone(),
                ns.clone(),
                name.clone(),
                None,
                false,
                false,
                LogWindow::Tail(self.log_tail),
                name.clone(),
                tx.clone(),
            ));
        }
        let label = format!("{}/{} pods({})", ns, selector, targets.len());
        self.mode = Mode::Logs(LogsState {
            source: LogSource::Multi {
                ns: ns.clone(),
                selector,
            },
            label,
            ns,
            pod: String::new(),
            container: None,
            previous: false,
            timestamps: false,
            lines: vec![],
            scroll_from_end: 0,
            wrap: true,
            status: format!("streaming {} pods\u{2026}", targets.len()),
            query: String::new(),
            search: false,
            window: LogWindow::Tail(self.log_tail),
            handles,
            rx,
            match_idx: None,
            match_total: 0,
            count_occurrences: false,
        });
    }

    pub async fn restart_log_stream(&mut self) {
        if let Mode::Logs(st) = &mut self.mode {
            for h in st.handles.drain(..) {
                h.abort();
            }
            st.lines.clear();
            st.scroll_from_end = 0;
            st.status = "restarting…".into();
            let (tx, rx) = mpsc::unbounded_channel();
            st.rx = rx;
            match st.source.clone() {
                LogSource::Single { ns, pod, container } => {
                    let h = k8s::spawn_logs_prefixed(
                        self.cluster.clone(),
                        ns,
                        pod,
                        container,
                        st.previous,
                        st.timestamps,
                        st.window,
                        String::new(),
                        tx,
                    );
                    st.handles.push(h);
                }
                LogSource::Multi { ns, selector } => {
                    let gvk = ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk(
                        "", "v1", "Pod",
                    ));
                    let api: kube::Api<kube::core::dynamic::DynamicObject> =
                        kube::Api::namespaced_with(self.cluster.client.clone(), &ns, &gvk);
                    let lp = kube::api::ListParams::default().labels(&selector);
                    let mut count = 0usize;
                    if let Ok(list) = api.list(&lp).await {
                        for p in list.items {
                            let pv = match serde_json::to_value(&p) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            if pv.pointer("/status/phase").and_then(|x| x.as_str())
                                == Some("Running")
                                && let Some(name) = p.metadata.name.clone()
                            {
                                let h = k8s::spawn_logs_prefixed(
                                    self.cluster.clone(),
                                    ns.clone(),
                                    name.clone(),
                                    None,
                                    st.previous,
                                    st.timestamps,
                                    st.window,
                                    name,
                                    tx.clone(),
                                );
                                st.handles.push(h);
                                count += 1;
                                if count >= 8 {
                                    break;
                                }
                            }
                        }
                    }
                    st.status = format!("streaming {count} pods…");
                }
            }
        }
    }

    pub fn close_logs(&mut self) {
        if let Mode::Logs(mut st) = std::mem::replace(&mut self.mode, Mode::Normal) {
            for h in st.handles.drain(..) {
                h.abort();
            }
        }
    }

    pub async fn start_exec(
        &mut self,
        ns: String,
        pod: String,
        container: Option<String>,
        command: Vec<String>,
    ) {
        match k8s::start_exec(self.cluster.clone(), ns, pod.clone(), container, command).await {
            Ok(mut sess) => {
                if let Some(stx) = &mut sess.size_tx
                    && let Ok((cols, rows)) = crossterm::terminal::size()
                {
                    let _ = stx.try_send(kube::api::TerminalSize {
                        width: cols,
                        height: rows.saturating_sub(6),
                    });
                }
                self.mode = Mode::Exec(ExecState {
                    pod,
                    out_rx: sess.out_rx,
                    ctl_tx: sess.ctl_tx,
                    size_tx: sess.size_tx,
                    buffer: Vec::with_capacity(64 * 1024),
                    node_pod: None,
                    status: "connected \u{2014} ctrl-q exits".into(),
                });
            }
            Err(e) => self.err_status(anyhow!("exec failed: {e}")),
        }
    }

    pub async fn start_attach(&mut self, ns: String, pod: String, container: Option<String>) {
        match k8s::start_attach(self.cluster.clone(), ns, pod.clone(), container).await {
            Ok(sess) => {
                self.mode = Mode::Exec(ExecState {
                    pod,
                    out_rx: sess.out_rx,
                    ctl_tx: sess.ctl_tx,
                    size_tx: sess.size_tx,
                    buffer: Vec::with_capacity(64 * 1024),
                    node_pod: None,
                    status: "attached \u{2014} ctrl-q exits".into(),
                });
            }
            Err(e) => self.err_status(anyhow!("attach failed: {e}")),
        }
    }

    pub fn close_exec(&mut self) {
        // node-shell sessions leave an ephemeral pod behind — remove it
        if let Mode::Exec(ex) = &self.mode
            && let Some((ns, pod)) = ex.node_pod.clone()
        {
            let cl = self.cluster.clone();
            tokio::spawn(async move { k8s::delete_pod_quiet(cl, ns, pod).await });
        }
        if let Mode::Exec(ex) = std::mem::replace(&mut self.mode, Mode::Normal) {
            let _ = ex.ctl_tx.send(ExecCtl::Abort);
        }
    }

    pub fn pump_streams(&mut self) {
        let cap = self.log_cap;
        if let Mode::Logs(st) = &mut self.mode {
            while let Ok(msg) = st.rx.try_recv() {
                match msg {
                    LogMsg::Line(l) => {
                        st.lines.push(l);
                        if st.lines.len() > cap {
                            st.lines.drain(..st.lines.len() - cap);
                        }
                        st.scroll_from_end = 0;
                        st.status.clear();
                    }
                    LogMsg::Done(s) => st.status = s,
                }
            }
        } else if let Mode::Exec(ex) = &mut self.mode {
            const EXEC_CAP: usize = 512 * 1024;
            let mut disconnected = false;
            loop {
                match ex.out_rx.try_recv() {
                    Ok(chunk) => match chunk {
                        Ok(bytes) => {
                            apply_exec_bytes(&mut ex.buffer, &bytes);
                            if ex.buffer.len() > EXEC_CAP {
                                let cut = ex.buffer.len() - EXEC_CAP;
                                let nl = ex.buffer[cut..]
                                    .iter()
                                    .position(|&b| b == b'\n')
                                    .unwrap_or(0);
                                ex.buffer.drain(..cut + nl);
                            }
                        }
                        Err(s) => ex.status = s,
                    },
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if disconnected {
                self.close_exec();
                self.set_status("exec closed");
            }
        }
    }
}

impl App {
    pub fn start_pulse(&mut self) {
        self.stop_pulse();
        self.view = ViewKind::Pulse;
        let cl = self.cluster.clone();
        let counts = self.pulse_counts.clone();
        let tx = self.tx.clone();
        let scope = self.scope.clone();
        self.pulse_handles.push(tokio::spawn(async move {
            loop {
                // one batched pass over the 12 kinds, scoped to current ns where applicable
                match k8s::pulse_counts(&cl, scope.get().as_deref()).await {
                    Ok(rows) => {
                        let mut m = counts.lock().unwrap();
                        m.clear();
                        for (label, total, healthy) in rows {
                            m.insert(label.to_string(), (total, healthy));
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Err(format!("pulse: {e}")));
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }));
    }

    /// restart the pulse poller when namespace/scope changed under it
    pub fn refresh_pulse_if_active(&mut self) {
        if self.view == ViewKind::Pulse {
            self.start_pulse();
        }
    }

    pub fn stop_pulse(&mut self) {
        for h in self.pulse_handles.drain(..) {
            h.abort();
        }
        self.pulse_counts.lock().unwrap().clear();
    }

    pub fn start_pf(&mut self, entry: PfEntry) {
        self.set_status(format!("pf #{}, :pf to manage", entry.local_port));
        self.pfs.push(entry);
    }

    pub fn stop_pf_entry(&mut self, id: u64) {
        if let Some(i) = self.pfs.iter().position(|p| p.id == id) {
            let e = self.pfs.remove(i);
            e.stop.store(true, Ordering::Relaxed);
            e.handle.abort();
            if let Ok(m) = e.conns_tasks.lock() {
                for (_, h) in m.iter() {
                    h.abort();
                }
            }
            self.set_status(format!("port-forward #{id} stopped"));
        }
    }

    pub fn stop_all_pfs(&mut self) {
        let n = self.pfs.len();
        for e in self.pfs.iter_mut() {
            e.stop.store(true, Ordering::Relaxed);
            e.handle.abort();
            if let Ok(m) = e.conns_tasks.lock() {
                for (_, h) in m.iter() {
                    h.abort();
                }
            }
        }
        self.pfs.clear();
        self.set_status(format!("stopped {n} port-forward(s)"));
    }

    pub fn shutdown(&mut self) {
        self.stop_watch();
        self.stop_pulse();
        self.close_logs();
        self.close_exec();
        self.stop_all_pfs();
    }

    // ---- lookups used by actions/hints ----

    pub fn plugin_for_key(
        &self,
        code: crossterm::event::KeyCode,
        mods: crossterm::event::KeyModifiers,
    ) -> Option<(String, Plugin)> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let scope = self
            .view_spec
            .as_ref()
            .map(|s| s.alias.clone())
            .unwrap_or_default();
        for (name, pl) in &self.plugins {
            if !pl.scopes.is_empty() && !pl.scopes.iter().any(|sc| *sc == scope || sc == "*") {
                continue;
            }
            let want = pl.short_cut.as_str();
            let hit = if let Some(stripped) = want.strip_prefix("ctrl-") {
                let c = stripped
                    .chars()
                    .next()
                    .unwrap_or('\u{0}')
                    .to_ascii_uppercase();
                matches!(code, KeyCode::Char(ch) if ch.to_ascii_uppercase() == c)
                    && mods.contains(KeyModifiers::CONTROL)
            } else {
                matches!(code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&want.chars().next().unwrap_or('\u{0}')))
                    && mods.is_empty()
            };
            if hit {
                return Some((name.clone(), pl.clone()));
            }
        }
        None
    }

    pub async fn build_xray(&self, name: &str) -> Result<Vec<String>> {
        let spec = self
            .view_spec
            .clone()
            .ok_or_else(|| anyhow!("no selection"))?;
        let ns = self.effective_ns(&spec).unwrap_or_else(|| self.ns.clone());
        let mut lines = vec![format!("xray {}/{} @ {}", spec.kind, name, ns)];
        let obj = self.cluster.dyn_api(&spec, Some(&ns)).get(name).await?;
        let uid = obj.metadata.uid.clone().unwrap_or_default();
        match spec.kind.as_str() {
            "Deployment" | "StatefulSet" | "DaemonSet" => {
                lines.push(format!("\u{2514}\u{2500} {} ({})", spec.kind, name));
                let rs_spec = spec_for("rs").unwrap();
                let pods_spec = spec_for("po").unwrap();
                let rs_api = self.cluster.dyn_api(&rs_spec, Some(&ns));
                let rss = rs_api.list(&kube::api::ListParams::default()).await?;
                for rs in rss.items {
                    let owned = rs
                        .metadata
                        .owner_references
                        .as_ref()
                        .map(|o| o.iter().any(|r| r.uid == uid))
                        .unwrap_or(false);
                    if !owned {
                        continue;
                    }
                    let rs_name = rs.metadata.name.clone().unwrap_or_default();
                    let rs_uid = rs.metadata.uid.clone().unwrap_or_default();
                    let ready = serde_json::to_value(&rs)
                        .ok()
                        .and_then(|v| v.pointer("/status/readyReplicas").and_then(|x| x.as_i64()))
                        .unwrap_or(0);
                    lines.push(format!(
                        "   \u{251c}\u{2500} replicaset/{rs_name} ready={ready}"
                    ));
                    let po_api = self.cluster.dyn_api(&pods_spec, Some(&ns));
                    let pos = po_api.list(&kube::api::ListParams::default()).await?;
                    for po in pos.items {
                        let pok = po
                            .metadata
                            .owner_references
                            .as_ref()
                            .map(|o| o.iter().any(|r| r.uid == rs_uid))
                            .unwrap_or(false);
                        if !pok {
                            continue;
                        }
                        let pv = serde_json::to_value(&po)?;
                        let row = crate::model::extract(&pods_spec, &pv);
                        lines.push(format!(
                            "   \u{2502}    \u{2514}\u{2500} pod/{} [{}] {}",
                            row.key,
                            row.cells.get(2).map(|s| s.as_str()).unwrap_or("?"),
                            row.cells.get(1).map(|s| s.as_str()).unwrap_or("?")
                        ));
                    }
                }
            }
            "Pod" => {
                lines.push(format!("\u{2514}\u{2500} pod/{name}"));
                let v = serde_json::to_value(&obj)?;
                if let Some(cs) = v.pointer("/spec/containers").and_then(|c| c.as_array()) {
                    for c in cs {
                        lines.push(format!(
                            "   \u{251c}\u{2500} container/{} ({})",
                            c.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
                            c.get("image").and_then(|n| n.as_str()).unwrap_or("?")
                        ));
                    }
                }
            }
            "Node" => {
                lines.push(format!("\u{2514}\u{2500} node/{name}"));
                let pods_spec = spec_for("po").unwrap();
                let po_api = self.cluster.dyn_api(&pods_spec, None);
                let lp = kube::api::ListParams::default().fields(&format!("spec.nodeName={name}"));
                let pos = po_api.list(&lp).await?;
                lines.push(format!(
                    "   \u{2514}\u{2500} {} pods scheduled",
                    pos.items.len()
                ));
                for po in pos.items.iter().take(20) {
                    if let (Some(n2), Some(ns2)) =
                        (po.metadata.name.clone(), po.metadata.namespace.clone())
                    {
                        lines.push(format!("      \u{251c}\u{2500} {ns2}/{n2}"));
                    }
                }
            }
            other => return Err(anyhow!("xray not supported for {other}")),
        }
        Ok(lines)
    }

    pub async fn drill_node_pods(&mut self, node: &str) -> Result<()> {
        let gvk = ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk("", "v1", "Pod"));
        let api: kube::Api<kube::core::dynamic::DynamicObject> =
            kube::Api::all_with(self.cluster.client.clone(), &gvk);
        let lp = kube::api::ListParams::default().fields(&format!("spec.nodeName={node}"));
        let objs = api.list(&lp).await?;
        self.stop_watch();
        self.watch = None;
        self.rows.clear();
        self.sel_key = None;
        self.drill_selector = None;
        self.drill_title = Some(format!("node/{node}"));
        self.view_spec = Some(spec_for("po").unwrap());
        self.view = ViewKind::Table;
        for o in objs.items {
            let v = serde_json::to_value(&o)?;
            let row = crate::model::extract(self.view_spec.as_ref().unwrap(), &v);
            self.rows
                .insert(row.key.clone(), (row, std::time::Instant::now()));
        }
        self.set_status(format!("snapshot: pods on node/{node} (r to refresh)"));
        Ok(())
    }

    pub async fn drill_cronjob_jobs(&mut self, cron: &str) -> Result<()> {
        let cj_spec = spec_for("cj").unwrap();
        let ns = self.target_ns(cron).unwrap_or_else(|| self.ns.clone());
        let cj_api = self.cluster.dyn_api(&cj_spec, Some(&ns));
        let pure = self.pure_name(cron).to_string();
        let cj = cj_api.get(&pure).await?;
        let uid = cj.metadata.uid.clone().unwrap_or_default();
        let job_spec = spec_for("job").unwrap();
        let job_api = self.cluster.dyn_api(&job_spec, Some(&ns));
        let jobs = job_api.list(&kube::api::ListParams::default()).await?;
        self.stop_watch();
        self.watch = None;
        self.rows.clear();
        self.sel_key = None;
        self.drill_selector = None;
        self.drill_title = Some(format!("cronjob/{cron}"));
        self.view_spec = Some(job_spec);
        self.view = ViewKind::Table;
        for j in jobs.items {
            let owned = j
                .metadata
                .owner_references
                .as_ref()
                .map(|o| o.iter().any(|r| r.uid == uid))
                .unwrap_or(false);
            if !owned {
                continue;
            }
            let v = serde_json::to_value(&j)?;
            let row = crate::model::extract(self.view_spec.as_ref().unwrap(), &v);
            self.rows
                .insert(row.key.clone(), (row, std::time::Instant::now()));
        }
        self.set_status(format!("snapshot: jobs of cronjob/{cron} (r to refresh)"));
        Ok(())
    }

    pub async fn first_pod_of_workload(&self, spec: &KindSpec, name: &str) -> Option<String> {
        let gvk = ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk(
            &spec.group,
            &spec.version,
            &spec.kind,
        ));
        let api: kube::Api<kube::core::dynamic::DynamicObject> =
            kube::Api::namespaced_with(self.cluster.client.clone(), &self.target_ns(name)?, &gvk);
        let obj = api.get(name).await.ok()?;
        let v = serde_json::to_value(&obj).ok()?;
        let pairs = crate::model::selector_labels(&v);
        if pairs.is_empty() {
            return None;
        }
        let sel = pairs
            .iter()
            .map(|(k, val)| format!("{k}={val}"))
            .collect::<Vec<_>>()
            .join(",");
        let pod_gvk =
            ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk("", "v1", "Pod"));
        let pods: kube::Api<kube::core::dynamic::DynamicObject> = kube::Api::namespaced_with(
            self.cluster.client.clone(),
            &self.target_ns(name)?,
            &pod_gvk,
        );
        let list = pods
            .list(&kube::api::ListParams::default().labels(&sel))
            .await
            .ok()?;
        for p in list.items {
            let pv = serde_json::to_value(&p).ok()?;
            if pv.pointer("/status/phase").and_then(|x| x.as_str()) == Some("Running") {
                return p.metadata.name.clone();
            }
        }
        None
    }

    pub async fn open_port_forward_dialog(&mut self, ns: String, pod: String) {
        let gvk = ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk("", "v1", "Pod"));
        let api: kube::Api<kube::core::dynamic::DynamicObject> =
            kube::Api::namespaced_with(self.cluster.client.clone(), &ns, &gvk);
        let obj = match api.get(&pod).await {
            Ok(o) => o,
            Err(e) => {
                self.set_status(format!("!pf: cannot get pod {pod}: {e}"));
                return;
            }
        };
        let v = match serde_json::to_value(&obj) {
            Ok(v) => v,
            Err(e) => {
                self.set_status(format!("!pf: parse error: {e}"));
                return;
            }
        };
        let phase = v
            .pointer("/status/phase")
            .and_then(|x| x.as_str())
            .unwrap_or("Unknown");
        if phase != "Running" {
            self.set_status(format!("pod must be running. Current status={phase}"));
            return;
        }

        let mut ports: Vec<(String, u16, Option<String>)> = Vec::new();
        if let Some(containers) = v.pointer("/spec/containers").and_then(|c| c.as_array()) {
            for c in containers {
                let co_name = c
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(c_ports) = c.get("ports").and_then(|p| p.as_array()) {
                    for p in c_ports {
                        let proto = p
                            .get("protocol")
                            .and_then(|pr| pr.as_str())
                            .unwrap_or("TCP");
                        if proto != "TCP" {
                            continue;
                        }
                        if let Some(port_num) = p.get("containerPort").and_then(|pn| pn.as_u64()) {
                            let port_name = p
                                .get("name")
                                .and_then(|pn| pn.as_str())
                                .map(|s| s.to_string());
                            ports.push((co_name.clone(), port_num as u16, port_name));
                        }
                    }
                }
            }
        }

        let (container_port_buf, local_port_buf) = if let Some((co, port, _)) = ports.first() {
            (format!("{co}::{port}"), port.to_string())
        } else {
            (String::new(), String::new())
        };
        let address_buf = std::env::var("K9X_PF_ADDRESS").unwrap_or_else(|_| "127.0.0.1".into());

        self.mode = Mode::PortForward(PfDialogState {
            ns,
            pod,
            ports,
            container_port: container_port_buf,
            local_port: local_port_buf,
            address: address_buf,
            focus: PfFocus::ContainerPort,
        });
    }

    pub async fn pf_menu_for_pod(&mut self, ns: String, name: String) {
        self.open_port_forward_dialog(ns, name).await;
    }

    pub async fn pf_for_service(&mut self, name: &str) {
        let svc_spec = match spec_for("svc") {
            Some(s) => s,
            None => {
                self.set_status("!service spec not found");
                return;
            }
        };
        let Some(ns) = self.target_ns(name) else {
            self.set_status("!cannot resolve namespace");
            return;
        };
        let pure = self.pure_name(name).to_string();
        let svc = match self.cluster.dyn_api(&svc_spec, Some(&ns)).get(&pure).await {
            Ok(s) => s,
            Err(e) => {
                self.err_status(anyhow!(e));
                return;
            }
        };
        let v = match serde_json::to_value(&svc) {
            Ok(v) => v,
            Err(e) => {
                self.err_status(&e);
                return;
            }
        };
        let pairs = crate::model::selector_labels(&v);
        if pairs.is_empty() {
            self.set_status("!service has no pod selector (externally managed)");
            return;
        }
        let sel = pairs
            .iter()
            .map(|(k, val)| format!("{k}={val}"))
            .collect::<Vec<_>>()
            .join(",");
        let pod_gvk =
            ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk("", "v1", "Pod"));
        let pods: kube::Api<kube::core::dynamic::DynamicObject> =
            kube::Api::namespaced_with(self.cluster.client.clone(), &ns, &pod_gvk);
        let list = match pods
            .list(&kube::api::ListParams::default().labels(&sel))
            .await
        {
            Ok(l) => l,
            Err(e) => {
                self.err_status(&e);
                return;
            }
        };
        let mut target: Option<String> = None;
        for p in list.items {
            if let Ok(pv) = serde_json::to_value(&p)
                && pv.pointer("/status/phase").and_then(|x| x.as_str()) == Some("Running")
            {
                target = p.metadata.name.clone();
                break;
            }
        }
        match target {
            Some(pod) => self.open_port_forward_dialog(ns, pod).await,
            None => self.set_status("!no ready pod behind service"),
        }
    }

    pub async fn pf_menu_for_service(&mut self, name: &str) {
        self.pf_for_service(name).await;
    }
}

// ---- helm deep integration ----

impl App {
    /// :helm with no args — pick a release
    pub async fn open_helm_releases_menu(&mut self) {
        let ns = if self.all_ns {
            None
        } else {
            Some(self.ns.clone())
        };
        self.set_status("loading helm releases…");
        match k8s::helm_releases(&self.cluster, ns.as_deref()).await {
            Ok(rels) if rels.is_empty() => self.set_status("!no helm releases found"),
            Ok(rels) => {
                let items: Vec<MenuItem> = rels
                    .iter()
                    .map(|r| {
                        MenuItem::new(
                            format!(
                                "{:<24} {:>3}  {:<10} {}",
                                r.name, r.revision, r.status, r.chart
                            ),
                            format!("{}|{}", r.name, r.namespace),
                        )
                    })
                    .collect();
                self.set_status("");
                self.mode = Mode::Menu(Menu {
                    title: "helm releases · enter=history".into(),
                    items,
                    sel: 0,
                    purpose: MenuPurpose::HelmList {
                        releases: rels
                            .iter()
                            .map(|r| (r.name.clone(), r.namespace.clone()))
                            .collect(),
                    },
                });
            }
            Err(e) => self.err_status(e),
        }
    }

    /// revision history for one release; Enter views values, R rolls back
    pub async fn open_helm_history_menu(&mut self, ns: String, name: String) {
        match k8s::helm_history(&self.cluster, &ns, &name).await {
            Ok(revs) if revs.is_empty() => self.set_status(format!("!no revisions for {name}")),
            Ok(revs) => {
                let items: Vec<MenuItem> = revs
                    .iter()
                    .map(|r| {
                        MenuItem::new(
                            format!(
                                "rev {:<3} {:<12} {}@{}  {}",
                                r.revision, r.status, r.chart, r.chart_ver, r.updated
                            ),
                            r.revision.to_string(),
                        )
                    })
                    .collect();
                self.mode = Mode::Menu(Menu {
                    title: format!("{name} · enter=values R=rollback"),
                    items,
                    sel: 0,
                    purpose: MenuPurpose::HelmHistory { ns, name, revs },
                });
            }
            Err(e) => self.err_status(e),
        }
    }

    /// values of the highlighted revision (no extra fetch — decoded at history load)
    pub fn helm_values_of_selected(&mut self) {
        let Mode::Menu(m) = &self.mode else { return };
        let MenuPurpose::HelmHistory { ns, name, revs } = &m.purpose else {
            return;
        };
        let Some(sel) = m.items.get(m.sel) else {
            return;
        };
        let Ok(rev) = sel.value.parse::<i64>() else {
            return;
        };
        let Some(r) = revs.iter().find(|r| r.revision == rev) else {
            return;
        };
        let lines: Vec<String> = format!(
            "# release: {name}\n# namespace: {ns}\n# revision: {rev} ({})\n\n{}",
            r.status, r.values_yaml
        )
        .lines()
        .map(String::from)
        .collect();
        self.mode = Mode::TextPane {
            title: format!("helm values:{name}/v{rev}"),
            lines,
            pos: 0,
            wrap: true,
        };
    }

    // ---- RBAC explorer ----

    /// :policy — pick a service account to inspect effective permissions
    pub async fn open_policy_subjects_menu(&mut self) {
        // gather SAs across current scope
        let mut items: Vec<MenuItem> = vec![];
        let spec = spec_for("sa").unwrap();
        let api = self
            .cluster
            .dyn_api(&spec, self.effective_ns(&spec).as_deref());
        if let Ok(list) = api.list(&kube::api::ListParams::default()).await {
            for o in list.items {
                let v = serde_json::to_value(&o).unwrap_or_default();
                let ns = v
                    .pointer("/metadata/namespace")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let n = v
                    .pointer("/metadata/name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if !n.is_empty() {
                    items.push(MenuItem::new(
                        if ns.is_empty() {
                            n.clone()
                        } else {
                            format!("{ns}/{n}")
                        },
                        format!("{ns}|{n}"),
                    ));
                }
            }
        }
        if items.is_empty() {
            self.set_status("!no service accounts found");
            return;
        }
        self.mode = Mode::Menu(Menu {
            title: "policy · service accounts".into(),
            items,
            sel: 0,
            purpose: MenuPurpose::PolicySubjects,
        });
    }

    /// effective permissions: bindings matching the SA + expanded policy rules
    pub async fn open_policy_for(&mut self, subject_ns: String, sa: String) {
        self.set_status("resolving permissions…");
        let mut lines: Vec<String> = vec![
            format!("effective permissions for serviceaccount/{sa} (ns {subject_ns})"),
            String::new(),
        ];
        let rb_spec = spec_for("rb").unwrap();
        let crb_spec = spec_for("crb").unwrap();
        let role_spec = spec_for("role").unwrap();
        let crole_spec = spec_for("cr").unwrap();

        let collect = |bind: &Value, out: &mut Vec<(String, String, String)>| {
            // (role_kind, role_name, binding_label)
            let bname = bind
                .pointer("/metadata/name")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
                .to_string();
            let bns = bind
                .pointer("/metadata/namespace")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(rr) = bind.pointer("/roleRef") {
                let rk = rr
                    .get("kind")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let rn = rr
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push((rk, rn, format!("{bname} (ns {bns})")));
            }
        };
        let subject_matches = |v: &Value| -> bool {
            v.pointer("/subjects")
                .and_then(|s| s.as_array())
                .map(|a| {
                    a.iter().any(|s| {
                        s.get("kind").and_then(|x| x.as_str()) == Some("ServiceAccount")
                            && s.get("name").and_then(|x| x.as_str()) == Some(sa.as_str())
                    })
                })
                .unwrap_or(false)
        };

        let mut bindings: Vec<(String, String, String)> = vec![];
        if let Ok(list) = self
            .cluster
            .dyn_api(&rb_spec, None)
            .list(&Default::default())
            .await
        {
            for o in list.items {
                let v = serde_json::to_value(&o).unwrap_or_default();
                if subject_matches(&v) {
                    collect(&v, &mut bindings);
                }
            }
        }
        if let Ok(list) = self
            .cluster
            .dyn_api(&crb_spec, None)
            .list(&Default::default())
            .await
        {
            for o in list.items {
                let v = serde_json::to_value(&o).unwrap_or_default();
                if subject_matches(&v) {
                    collect(&v, &mut bindings);
                }
            }
        }
        if bindings.is_empty() {
            lines.push("(no bindings found for this service account)".into());
        }
        for (rk, rn, blabel) in &bindings {
            lines.push(format!("via {} → {rk}/{}", blabel, rn));
            let src = if rk == "Role" {
                &role_spec
            } else {
                &crole_spec
            };
            let ns_arg = if rk == "Role" {
                Some(subject_ns.as_str())
            } else {
                None
            };
            if let Ok(obj) = self.cluster.dyn_api(src, ns_arg).get(rn).await
                && let Ok(v) = serde_json::to_value(&obj)
            {
                append_rule_lines(&v, &mut lines);
            }
            lines.push(String::new());
        }
        self.set_status("");
        self.mode = Mode::TextPane {
            title: format!("policy:sa/{sa}"),
            lines,
            pos: 0,
            wrap: false,
        };
    }

    /// expand policyRules of a Role/ClusterRole into a read-only pane
    pub async fn open_role_rules(&mut self, kind: &str, name: &str) {
        let Some(mut spec) = (if kind == "ClusterRole" {
            spec_for("cr")
        } else {
            spec_for("role")
        }) else {
            self.set_status("!unknown role kind");
            return;
        };
        self.apply_view_override(&mut spec);
        let ns = if kind == "ClusterRole" {
            None
        } else {
            Some(self.ns.clone())
        };
        let pure = self.pure_name(name);
        match self.cluster.dyn_api(&spec, ns.as_deref()).get(pure).await {
            Ok(obj) => {
                let v = serde_json::to_value(&obj).unwrap_or_default();
                let mut lines = vec![format!("{kind}/{name} — policy rules"), String::new()];
                append_rule_lines(&v, &mut lines);
                if lines.len() <= 2 {
                    lines.push("(no rules)".into());
                }
                self.mode = Mode::TextPane {
                    title: format!("rules:{kind}/{name}"),
                    lines,
                    pos: 0,
                    wrap: false,
                };
            }
            Err(e) => self.err_status(e),
        }
    }

    /// k9s-style UsedBy: which workloads/pods/SAs/ingresses reference this configmap/secret
    pub async fn open_used_by(&mut self, is_secret: bool, name: String) {
        let scope = self.effective_ns_for_action();
        let pure = self.pure_name(&name).to_string();
        self.set_status(format!("scanning references to {pure}…"));
        let cl = self.cluster.clone();
        let _tx = self.tx.clone();
        let res = k8s::used_by(&cl, &pure, is_secret, scope.as_deref()).await;
        match res {
            Ok(hits) if hits.is_empty() => {
                self.set_status(format!("!no references found for {name}"));
            }
            Ok(hits) => {
                let mut lines = vec![
                    format!("{hits_count} reference(s):", hits_count = hits.len()),
                    String::new(),
                ];
                for h in hits {
                    lines.push(format!(
                        "{:<14} {:<12} {:<40} via {}",
                        h.kind, h.ns, h.name, h.via
                    ));
                }
                let kindword = if is_secret { "secret" } else { "configmap" };
                self.set_status("");
                self.mode = Mode::TextPane {
                    title: format!("used-by:{kindword}/{name}"),
                    lines,
                    pos: 0,
                    wrap: false,
                };
            }
            Err(e) => self.err_status(e),
        }
    }
}

// ---- free helpers used by main/ui/tests ----

pub fn command_list() -> Vec<String> {
    let mut v: Vec<String> = crate::model::all_aliases()
        .iter()
        .map(|s| s.to_string())
        .collect();
    v.extend(
        [
            "ctx", "context", "ns", "pulse", "pf", "helm", "xray", "popeye", "crds", "policy",
            "help", "exit", "quit",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    v.sort();
    v.dedup();
    v
}

pub fn suggest(prefix: &str) -> Vec<String> {
    let p = prefix.to_lowercase();
    command_list()
        .into_iter()
        .filter(|c| c.starts_with(&p))
        .take(9)
        .collect()
}

pub fn contains_ignore_case(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle_lower.is_ascii() {
        let n = needle_lower.as_bytes();
        let h = haystack.as_bytes();
        if n.len() > h.len() {
            return false;
        }
        h.windows(n.len()).any(|w| {
            w.iter()
                .zip(n.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
        })
    } else {
        haystack.to_lowercase().contains(needle_lower)
    }
}

pub fn fuzzy_match_multi(key: &str, cells: &[String], needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    let mut needle_chars = needle_lower.chars().peekable();
    let iter = key.chars().chain(std::iter::once(' ')).chain(
        cells
            .iter()
            .flat_map(|c| c.chars().chain(std::iter::once(' '))),
    );
    for c in iter {
        if let Some(&nc) = needle_chars.peek() {
            if c.to_ascii_lowercase() == nc {
                needle_chars.next();
            }
        } else {
            return true;
        }
    }
    needle_chars.peek().is_none()
}

#[cfg(test)]
pub fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut it = haystack.char_indices();
    for nc in needle.chars() {
        if it.find(|(_, hc)| *hc == nc).is_none() {
            return false;
        }
    }
    true
}

pub fn natural_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let ba = a.as_bytes();
    let bb = b.as_bytes();
    let (mut pa, mut pb) = (0usize, 0usize);
    while pa < ba.len() && pb < bb.len() {
        let ca = ba[pa];
        let cb = bb[pb];
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let na = num_at(ba, &mut pa);
            let nb = num_at(bb, &mut pb);
            if na != nb {
                return na.cmp(&nb);
            }
        } else {
            let la = ca.to_ascii_lowercase();
            let lb = cb.to_ascii_lowercase();
            if la != lb {
                return la.cmp(&lb);
            }
            pa += 1;
            pb += 1;
        }
    }
    (ba.len() - pa).cmp(&(bb.len() - pb))
}

fn num_at(b: &[u8], p: &mut usize) -> u64 {
    let start = *p;
    while *p < b.len() && b[*p].is_ascii_digit() {
        *p += 1;
    }
    std::str::from_utf8(&b[start..*p])
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(u64::MAX)
}

pub fn compute_dir_suggestions(input: &str) -> Vec<String> {
    let raw = input.trim();
    let expanded = if let Some(rest) = raw.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            format!("{home}/{rest}")
        } else {
            raw.to_string()
        }
    } else if raw == "~" {
        std::env::var("HOME").unwrap_or_else(|_| "~".to_string())
    } else if raw.is_empty() {
        "/tmp".to_string()
    } else {
        raw.to_string()
    };

    let path = std::path::Path::new(&expanded);
    let (parent, prefix) =
        if expanded.ends_with('/') || expanded.ends_with(std::path::MAIN_SEPARATOR) {
            (path, "")
        } else {
            match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => {
                    (p, path.file_name().and_then(|f| f.to_str()).unwrap_or(""))
                }
                _ => (std::path::Path::new("."), expanded.as_str()),
            }
        };

    let mut list = Vec::new();
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type()
                && file_type.is_dir()
            {
                let name = entry.file_name().to_string_lossy().to_string();
                if prefix.is_empty() || name.to_lowercase().starts_with(&prefix.to_lowercase()) {
                    let full = parent.join(&name);
                    let mut display = full.to_string_lossy().to_string();
                    if !display.ends_with('/') {
                        display.push('/');
                    }
                    if raw.starts_with("~/")
                        && let Ok(home) = std::env::var("HOME")
                        && let Some(rest) = display.strip_prefix(&home)
                    {
                        display = format!("~{rest}");
                    }
                    list.push(display);
                }
            }
        }
    }
    list.sort_by_key(|a| a.to_lowercase());
    list.truncate(100);
    list
}

pub fn apply_exec_bytes(buffer: &mut Vec<u8>, incoming: &[u8]) {
    let mut i = 0;
    while i < incoming.len() {
        match incoming[i] {
            0x08 | 0x7f => {
                // Backspace / DEL: erase previous character on current line
                if let Some(&last) = buffer.last()
                    && last != b'\n'
                {
                    buffer.pop();
                }
                i += 1;
            }
            b'\r' => {
                if i + 1 < incoming.len() && incoming[i + 1] == b'\n' {
                    buffer.push(b'\n');
                    i += 2;
                } else {
                    // Normalize standalone CR to newline so line content is not destroyed across chunks
                    buffer.push(b'\n');
                    i += 1;
                }
            }
            0x1b => {
                // ANSI escape sequence
                if i + 1 < incoming.len() && (incoming[i + 1] == b'[' || incoming[i + 1] == b']') {
                    let start = i;
                    i += 2;
                    while i < incoming.len() && !(0x40..=0x7e).contains(&incoming[i]) {
                        i += 1;
                    }
                    if i < incoming.len() {
                        let final_byte = incoming[i];
                        i += 1;
                        if final_byte == b'K' {
                            let seq = &incoming[start..i];
                            if seq.contains(&b'2') {
                                while let Some(&last) = buffer.last() {
                                    if last == b'\n' {
                                        break;
                                    }
                                    buffer.pop();
                                }
                            }
                        }
                    }
                } else {
                    i += 1;
                }
            }
            b => {
                buffer.push(b);
                i += 1;
            }
        }
    }
}

async fn workload_selector(app: &App, spec: &KindSpec, name: &str) -> Option<String> {
    use kube::core::dynamic::DynamicObject;
    let gvk = ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk(
        &spec.group,
        &spec.version,
        &spec.kind,
    ));
    let api: kube::Api<DynamicObject> =
        kube::Api::namespaced_with(app.cluster.client.clone(), &app.target_ns(name)?, &gvk);
    let obj = api.get(name).await.ok()?;
    let v = serde_json::to_value(&obj).ok()?;
    let pairs = crate::model::selector_labels(&v);
    if pairs.is_empty() {
        return None;
    }
    Some(
        pairs
            .iter()
            .map(|(k, val)| format!("{k}={val}"))
            .collect::<Vec<_>>()
            .join(","),
    )
}

async fn fetch_suspend_state_value(app: &mut App, name: &str) -> Option<bool> {
    let spec = spec_for("cj")?;
    let gvk = ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk(
        &spec.group,
        &spec.version,
        &spec.kind,
    ));
    let api: kube::Api<kube::core::dynamic::DynamicObject> =
        kube::Api::namespaced_with(app.cluster.client.clone(), &app.ns, &gvk);
    let pure = app.pure_name(name).to_string();
    let obj = api.get(&pure).await.ok()?;
    let v = serde_json::to_value(&obj).ok()?;
    v.pointer("/data/spec/suspend")
        .or_else(|| v.pointer("/spec/suspend"))
        .and_then(|x| x.as_bool())
}

// ---- pub shims: main.rs owns the confirm/edit/yaml/describe implementations ----

pub fn do_restart_pub(app: &mut App, spec: &KindSpec, name: &str) {
    let Some(ns) = app.target_ns(name) else {
        app.set_status("!cannot resolve namespace");
        return;
    };
    let pure = app.pure_name(name).to_string();
    let cl = app.cluster.clone();
    let sp = spec.clone();
    let n = pure;
    if app.ro {
        app.set_status("!read-only mode: restart blocked");
        return;
    }
    app.set_status(format!("restarting {}/{}…", sp.kind, n));
    tokio::spawn(async move {
        if let Ok(m) = k8s::rollout_restart(&cl, &sp, &ns, &n).await {
            let _ = m;
        }
    });
}

pub fn confirm_pub(app: &mut App, prompt: String, action: Action) {
    if app.ro {
        app.set_status("!read-only mode: mutation blocked");
        return;
    }
    app.mode = Mode::Confirm {
        prompt,
        action,
        sel_yes: false,
    };
}

pub fn decode_secret_pub(app: &mut App, name: &str) {
    let Some(ns) = app.target_ns(name) else {
        app.set_status("!cannot resolve namespace");
        return;
    };
    let pure = app.pure_name(name).to_string();
    let cl = app.cluster.clone();
    let tx = app.tx.clone();
    let nm = pure;
    app.set_status("decoding secret…");
    tokio::spawn(async move {
        match k8s::decode_secret(&cl, &ns, &nm).await {
            Ok(text) => {
                let lines: Vec<String> = text.lines().map(String::from).collect();
                let _ = tx.send(Msg::Pane {
                    title: format!("decoded:{nm}"),
                    lines,
                    wrap: false,
                });
            }
            Err(e) => {
                let _ = tx.send(Msg::Err(e.to_string()));
            }
        }
    });
}

pub fn edit_pub(app: &mut App, spec: &KindSpec, name: &str) {
    // handled by main.rs via a message back; simplest: set a marker command in status
    // main.rs intercepts this through run_action before reaching here.
    let _ = (app, spec, name);
}

pub async fn describe_pub(app: &mut App, spec: &KindSpec, name: &str) {
    let ns = app.target_ns(name);
    let pure = app.pure_name(name).to_string();
    let cl = app.cluster.clone();
    let tx = app.tx.clone();
    let sp = spec.clone();
    let nm = pure;
    app.set_status(format!("describing {}/{}…", sp.kind, nm));
    tokio::spawn(async move {
        match k8s::describe_obj(&cl, &sp, ns.as_deref(), &nm).await {
            Ok(text) => {
                let lines: Vec<String> = text.lines().map(String::from).collect();
                let _ = tx.send(Msg::Pane {
                    title: format!("describe:{}/{}", sp.kind, nm),
                    lines,
                    wrap: true,
                });
            }
            Err(e) => {
                let _ = tx.send(Msg::Err(e.to_string()));
            }
        }
    });
}

pub async fn yaml_pub(app: &mut App, spec: &KindSpec, name: &str) {
    let ns = app.target_ns(name);
    let pure = app.pure_name(name).to_string();
    let cl = app.cluster.clone();
    let tx = app.tx.clone();
    let sp = spec.clone();
    let nm = pure;
    app.set_status(format!("fetching yaml {}/{}…", sp.kind, nm));
    tokio::spawn(async move {
        match k8s::get_yaml(&cl, &sp, ns.as_deref(), &nm).await {
            Ok(text) => {
                let lines: Vec<String> = text.lines().map(String::from).collect();
                let _ = tx.send(Msg::Pane {
                    title: format!("yaml:{}/{}", sp.kind, nm),
                    lines,
                    wrap: false,
                });
            }
            Err(e) => {
                let _ = tx.send(Msg::Err(e.to_string()));
            }
        }
    });
}

// ---- theme system ----

impl App {
    /// :themes — pick a theme; selection starts a live preview with 20s auto-revert
    pub fn open_themes_menu(&mut self) {
        let items: Vec<MenuItem> = crate::cfg::THEME_MENU
            .iter()
            .map(|n| {
                let suffix =
                    if *n == self.theme_name || (*n == "monochrome" && self.theme_name == "mono") {
                        " \u{2714}"
                    } else {
                        ""
                    };
                MenuItem::new(
                    format!("{}{suffix}", crate::cfg::theme_label(n)),
                    n.to_string(),
                )
            })
            .collect();
        self.mode = Mode::Menu(Menu {
            title: "themes · enter=apply / edit custom".into(),
            items,
            sel: 0,
            purpose: MenuPurpose::Themes,
        });
    }

    /// open the custom-theme editor seeded from themes/custom.yml (or current colors)
    pub fn open_custom_theme_editor(&mut self) {
        let seed = crate::cfg::resolve_theme("custom").unwrap_or_else(|| self.theme.clone());
        let values: Vec<(String, String)> = crate::cfg::THEME_FIELDS
            .iter()
            .map(|f| (f.to_string(), seed.get_hex(f).unwrap_or_default()))
            .collect();
        self.mode = Mode::ThemeEditor {
            values,
            sel: 0,
            editing: false,
            buf: String::new(),
        };
    }

    /// build + apply a Theme from editor values (live)
    pub fn apply_editor_values(&mut self, values: &[(String, String)]) {
        let mut t = self.theme.clone();
        for (f, hex) in values {
            let _ = t.set_hex(f, hex);
        }
        self.theme = t;
    }

    /// start live preview of a named theme
    pub fn preview_theme(&mut self, name: &str) {
        match crate::cfg::resolve_theme(name) {
            Some(t) => {
                self.prev_theme = Some(self.theme.clone());
                self.prev_theme_name = Some(self.theme_name.clone());
                self.theme = t;
                self.theme_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(20));
                let Mode::Menu(m) = &mut self.mode else {
                    return;
                };
                m.title = format!(
                    "theme \u{2018}{name}\u{2019} preview \u{2014} y=keep \u{00b7} auto-reverts in 20s"
                );
                self.set_status(format!(
                    "previewing '{name}' \u{2014} y keeps it, otherwise auto-revert"
                ));
            }
            None => self.set_status(format!("!unknown theme '{name}'")),
        }
    }

    /// timeout/reject: restore previous colors AND close the keep-dialog
    pub fn revert_theme_preview(&mut self) {
        if let Some(prev) = self.prev_theme.take() {
            self.theme = prev;
        }
        self.theme_deadline = None;
        if matches!(self.mode, Mode::Confirm { .. }) {
            self.mode = Mode::Normal;
        }
        self.set_status("theme preview reverted");
    }

    /// user accepted within the window: cancel timer and persist choice
    pub fn keep_theme(&mut self, name: &str) {
        self.theme_deadline = None;
        self.prev_theme = None;
        self.theme_name = name.to_string();
        let mut fc = crate::cfg::FileCfg::load();
        fc.theme = name.to_string();
        fc.save();
        self.set_status(format!("theme applied & saved: {name}"));
    }

    /// called every loop tick: fires auto-revert when the window expires
    pub fn theme_tick(&mut self) {
        if let Some(dl) = self.theme_deadline
            && std::time::Instant::now() >= dl
        {
            self.revert_theme_preview();
        }
    }
}

// ---- batch 2: aliases/hotkeys panes, env, ref, dir, node shell ----

impl App {
    /// image of the selected pod (first container, or a specific one)
    pub async fn resolve_pod_image(&self, name: &str, container: Option<&str>) -> Option<String> {
        let ns = self.target_ns(name)?;
        let pure = self.pure_name(name);
        let api = self.cluster.pod_api(&ns);
        let p = api.get(pure).await.ok()?;
        let conts = p.spec?.containers;
        let picked = match container {
            Some(cn) => conts.iter().find(|c| c.name == *cn),
            None => conts.first(),
        }?;
        picked.image.clone()
    }
}
// ---- batch 2: aliases/hotkeys panes, env, ref, dir, node shell ----

impl App {
    pub fn open_aliases_pane(&mut self) {
        let mut lines = vec!["built-in resources:".to_string(), String::new()];
        let mut chunk = String::new();
        for a in crate::model::all_aliases() {
            if chunk.len() + a.len() + 2 > 70 {
                lines.push(chunk.clone());
                chunk.clear();
            }
            chunk.push_str(a);
            chunk.push_str("  ");
        }
        if !chunk.is_empty() {
            lines.push(chunk);
        }
        lines.push(String::new());
        lines.push("commands:".to_string());
        lines.push(format!("  {}", crate::app::command_list().join("  ")));
        lines.push(String::new());
        lines.push("custom aliases (~/.config/k9x/aliases.yml):".to_string());
        if self.custom_aliases.is_empty() {
            lines.push("  (none)".into());
        }
        for (a, target) in &self.custom_aliases {
            lines.push(format!("  {a} → {target}"));
        }
        self.mode = Mode::TextPane {
            title: format!("aliases · v{}", env!("CARGO_PKG_VERSION")),
            lines,
            pos: 0,
            wrap: false,
        };
    }

    pub fn open_hotkeys_pane(&mut self) {
        let mut lines = vec!["custom hotkeys (~/.config/k9x/hotkeys.yml):".to_string()];
        if self.hotkeys.is_empty() {
            lines.push("  (none)".into());
        }
        for (_, hk) in &self.hotkeys {
            lines.push(format!(
                "  {:<10} {}  — {}",
                hk.short_cut, hk.command, hk.description
            ));
        }
        lines.push(String::new());
        lines.push("plugins (~/.config/k9x/plugins.yml):".to_string());
        if self.plugins.is_empty() {
            lines.push("  (none)".into());
        }
        for (name, pl) in &self.plugins {
            let scopes = if pl.scopes.is_empty() {
                "*".into()
            } else {
                pl.scopes.join(",")
            };
            lines.push(format!(
                "  {:<10} {:<28} scopes:{}{}",
                pl.short_cut,
                name,
                scopes,
                if pl.dangerous { " [dangerous]" } else { "" }
            ));
        }
        self.mode = Mode::TextPane {
            title: "hotkeys & plugins".into(),
            lines,
            pos: 0,
            wrap: false,
        };
    }

    /// container environment inspection pane
    pub async fn open_container_env(&mut self, pod: &str, container: &str) {
        let ns = self.target_ns(pod).unwrap_or_else(|| self.ns.clone());
        match k8s::pod_env(&self.cluster, &ns, pod).await {
            Ok(conts) => {
                let mut lines = vec![format!("pod {ns}/{pod}"), String::new()];
                for (cn, entries) in &conts {
                    let mark = if cn == container { "▶" } else { " " };
                    lines.push(format!("{mark} container {cn}:"));
                    if entries.is_empty() {
                        lines.push("   (no env set)".into());
                    }
                    for e in entries {
                        lines.push(format!("   {e}"));
                    }
                    lines.push(String::new());
                }
                self.mode = Mode::TextPane {
                    title: format!("env:{pod}/{container}"),
                    lines,
                    pos: 0,
                    wrap: false,
                };
            }
            Err(e) => self.err_status(e),
        }
    }

    /// resource reference pane from the embedded table
    pub fn open_ref_pane(&mut self, what: &str) {
        let key = what.to_lowercase();
        // resolve alias → kind
        let kind_name = spec_for(&key)
            .map(|s| s.kind)
            .unwrap_or_else(|| what.to_string());
        let mut lines = vec![format!("{kind_name} — API field reference"), String::new()];
        match crate::model::reference_for(&kind_name) {
            Some(fields) => {
                for (f, d) in fields {
                    lines.push(format!("  {:<34} {}", f, d));
                }
            }
            None => {
                lines.push("  no embedded reference for this kind yet.".into());
                lines.push("  covered kinds are listed under `:help`.".into());
            }
        }
        if let Some(spec) = spec_for(&key) {
            lines.push(String::new());
            lines.push(format!(
                "apiVersion: {}/{} · plural: {} · namespaced: {}",
                spec.group, spec.version, spec.plural, spec.namespaced
            ));
        }
        self.mode = Mode::TextPane {
            title: format!("ref:{kind_name}"),
            lines,
            pos: 0,
            wrap: false,
        };
    }

    /// :dir — browse local directories / pick YAML files to apply
    pub async fn open_dir(&mut self, raw_path: &str) {
        let path = shellexpand(raw_path);
        let rd = std::fs::read_dir(&path);
        let Ok(rd) = rd else {
            self.set_status(format!("!cannot read directory {path}"));
            return;
        };
        let mut dirs: Vec<MenuItem> = vec![];
        let mut files: Vec<MenuItem> = vec![];
        dirs.push(MenuItem::new("../".to_string(), join_path(&path, "..")));
        let mut names: Vec<String> = rd
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
            .collect();
        names.sort();
        for n in names {
            let full = join_path(&path, &n);
            let is_dir = std::fs::metadata(&full)
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if is_dir {
                dirs.push(MenuItem::new(format!("{n}/"), full));
            } else if n.ends_with(".yml") || n.ends_with(".yaml") {
                files.push(MenuItem::new(n, full));
            }
        }
        let mut items = dirs;
        items.extend(files);
        if items.len() <= 1 {
            self.set_status(format!("!no yaml files in {path}"));
            return;
        }
        self.mode = Mode::Menu(Menu {
            title: format!("dir:{path} · enter=apply file"),
            items,
            sel: 0,
            purpose: MenuPurpose::DirBrowse,
        });
    }

    /// spawn privileged nsenter pod on a node, wait ready, attach shell.
    /// cleans the pod up when the session detaches.
    pub async fn node_shell(&mut self, node: String) {
        if self.ro {
            self.set_status("!read-only mode: node shell blocked");
            return;
        }
        // permission pre-flight: creating pods here will be rejected anyway if RBAC denies
        match k8s::can_i(&self.cluster, "create", "", "pods", Some(self.ns.as_str())).await {
            Some(pc) if !pc.allowed => {
                let reason = if pc.reason.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", pc.reason)
                };
                self.set_status(format!(
                    "!RBAC denied: cannot create pods in ns {}{reason}",
                    self.ns
                ));
                return;
            }
            _ => {}
        }
        let ns = self.ns.clone();
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_millis())
            .unwrap_or(0);
        let pod = format!("k9x-node-shell-{suffix}");
        self.set_status(format!("spawning privileged shell pod on {node}…"));
        if let Err(e) =
            k8s::create_node_shell_pod(&self.cluster, &node, &ns, &pod, "nginx:alpine").await
        {
            self.err_status(e.to_string());
            return;
        }
        if let Err(e) = k8s::wait_pod_running(&self.cluster, &ns, &pod, 25).await {
            self.err_status(e.to_string());
            let cl = self.cluster.clone();
            tokio::spawn(async move { k8s::delete_pod_quiet(cl, ns, pod).await });
            return;
        }
        self.start_exec(
            ns.clone(),
            pod.clone(),
            None,
            vec![
                "nsenter".into(),
                "-t".into(),
                "1".into(),
                "-m".into(),
                "-u".into(),
                "-i".into(),
                "-n".into(),
                "--".into(),
                "sh".into(),
                "-i".into(),
            ],
        )
        .await;
        if let Mode::Exec(ex) = &mut self.mode {
            ex.node_pod = Some((ns, pod));
        }
        self.set_status(format!(
            "node shell on {node} — ctrl-q detaches & cleans up"
        ));
    }
}

fn join_path(base: &str, name: &str) -> String {
    if name == ".." {
        match std::path::Path::new(base).parent() {
            Some(p) => p.display().to_string(),
            None => base.to_string(),
        }
    } else {
        format!("{base}/{name}")
    }
}

pub fn shellexpand(p: &str) -> String {
    if let Some(rest) = p.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}{rest}")
    } else {
        p.to_string()
    }
}

/// expanded policy rules renderer shared by role panes
fn append_rule_lines(role: &Value, lines: &mut Vec<String>) {
    if let Some(rules) = role.pointer("/rules").and_then(|x| x.as_array()) {
        for r in rules {
            let g = r
                .get("apiGroups")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let res = r
                .get("resources")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let verbs = r
                .get("verbs")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let nru = r
                .get("nonResourceURLs")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            if !nru.is_empty() {
                lines.push(format!("  nonResourceURLs [{nru}]"));
            } else {
                lines.push(format!("  [{g}] {res} → {verbs}"));
            }
        }
    }
}
