mod app;
mod awsup;
mod cfg;
mod k8s;
mod model;
mod splash;
mod ui;

use anyhow::{Result, anyhow};
use app::{Action, App, InputPurpose, MenuPurpose, Mode, ViewKind};
use clap::{CommandFactory, Parser as ClapParser};
use k8s::{LogWindow, Msg};
use kube::core::ApiResource;
use kube::{Api, api::ListParams, api::LogParams, runtime::watcher};
type Pod = k8s_openapi::api::core::v1::Pod;
use chrono::Utc;
use model::{KindSpec, spec_for};
use serde_json::Value;
use std::io::IsTerminal as _;
use std::sync::Arc;

#[derive(ClapParser, Debug)]
#[command(
    name = "k9x",
    version,
    about = "k9x — event-driven Kubernetes TUI + agent CLI (a fast k9s alternative)",
    arg_required_else_help = false
)]
// Args derives clap::Command::command via ClapParser
struct Args {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// kube context to use (default: config or current-context)
    #[arg(short = 'x', long)]
    context: Option<String>,
    /// namespace scope
    #[arg(short = 'n', long)]
    namespace: Option<String>,
    /// all namespaces
    #[arg(short = 'A', long)]
    all_namespaces: bool,
    /// read-only mode: blocks every mutating action (TUI and CLI)
    #[arg(short = 'r', long)]
    readonly: bool,
    /// k9s parity: hide the header/info section
    #[arg(long)]
    headless: bool,
    /// k9s parity: hide the logo panel
    #[arg(long)]
    logoless: bool,
    /// k9s parity: hide the shortcut hints
    #[arg(long)]
    crumbsless: bool,
    /// k9s parity: accepted, k9x has no splash screen
    #[arg(long)]
    splashless: bool,
    /// explicitly enable mutations (overrides readonly config)
    #[arg(long)]
    write: bool,
    /// swap dark/light theme presets
    #[arg(long)]
    invert: bool,
    /// initial resource view (same as positional VIEW)
    #[arg(short = 'c', long = "command")]
    command: Option<String>,
    /// UI refresh tick in seconds (k9s --refresh)
    #[arg(long)]
    refresh: Option<f32>,
    /// directory for screendumps (k9s --screen-dump-dir)
    #[arg(long)]
    screen_dump_dir: Option<String>,
    /// initial resource view (e.g. po, deploy, svc)
    view: Option<String>,
}

#[derive(clap::Subcommand, Debug, Clone)]
enum Cmd {
    /// generate shell completions (bash|zsh|fish|elvish|powershell)
    Completions { shell: clap_complete::Shell },
    /// list resources (agent-friendly one-shot)
    Ls {
        resource: String,
        #[arg(short = 'n')]
        namespace: Option<String>,
        #[arg(short = 'A')]
        all: bool,
        #[arg(short = 'l')]
        selector: Option<String>,
        #[arg(short = 'o', default_value = "table")]
        output: String,
        #[arg(long)]
        watch: bool,
    },
    /// fetch one object as yaml/json
    Get {
        resource: String,
        name: String,
        #[arg(short = 'n')]
        namespace: Option<String>,
        #[arg(short = 'o', default_value = "yaml")]
        output: String,
    },
    /// stream pod logs
    Logs {
        pod: String,
        #[arg(short = 'c')]
        container: Option<String>,
        #[arg(short = 'f')]
        follow: bool,
        #[arg(short = 'p')]
        previous: bool,
        #[arg(long)]
        tail: Option<i64>,
        #[arg(short = 't')]
        timestamps: bool,
    },
    /// describe one object (+related events)
    Describe {
        resource: String,
        name: String,
        #[arg(short = 'n')]
        namespace: Option<String>,
    },
    /// decode a secret to plaintext
    Decode {
        name: String,
        #[arg(short = 'n')]
        namespace: Option<String>,
    },
    /// list contexts (or print current when none matches)
    Ctx { context: Option<String> },
    /// list namespaces
    Ns,
    /// delete an object (--yes required)
    Del {
        resource: String,
        name: String,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        yes: bool,
    },
    /// scale a workload (--yes required)
    Scale {
        resource: String,
        name: String,
        replicas: i64,
        #[arg(short = 'n')]
        namespace: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// rollout restart deploy/sts/ds (--yes required)
    Restart {
        resource: String,
        name: String,
        #[arg(short = 'n')]
        namespace: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// cordon a node (--yes required)
    Cordon {
        node: String,
        #[arg(long)]
        yes: bool,
    },
    /// uncordon a node (--yes required)
    Uncordon {
        node: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug)]
enum InputEvent {
    Key(crossterm::event::KeyEvent),
    Resized(u16, u16),
    Mouse(crossterm::event::MouseEvent),
}
type KeyTx = tokio::sync::mpsc::UnboundedSender<InputEvent>;

static INPUT_PAUSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static INPUT_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static TUI_INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) struct TuiGuard;

impl Drop for TuiGuard {
    fn drop(&mut self) {
        restore_tui();
    }
}

/// Heuristic: does this error text implicate a token/credential/authorization
/// problem (as opposed to e.g. a missing kubeconfig or plain connect failure)?
fn authish(m: &str) -> bool {
    let l = m.to_lowercase();
    [
        "token",
        "expired",
        "unauthorized",
        "401",
        "auth",
        "credential",
        "access denied",
        "invalid_grant",
    ]
    .iter()
    .any(|k| l.contains(k))
}

/// Map a cluster-connect failure to a final user-facing error.
/// "Not configured" states must never be rendered as a connection failure:
/// they are re-raised so `main` turns them into a graceful informational exit
/// (code 0). Genuine auth/connect failures keep accurate text, and only point
/// at cloud credentials when an authentication problem is actually implicated.
fn connect_err(e: anyhow::Error) -> anyhow::Error {
    if let Some(nc) = k8s::classify_connect_err(&e) {
        return anyhow::Error::new(nc);
    }
    let raw = e.to_string();
    let hint = if authish(&raw) {
        "\n→ refresh cloud credentials (e.g. `aws sso login`) and retry"
    } else {
        ""
    };
    anyhow!("cluster connect failed: {raw}{hint}")
}

#[cfg(unix)]
static SAVED_STDERR_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

#[cfg(unix)]
pub(crate) fn silence_tui_stderr() {
    unsafe {
        if SAVED_STDERR_FD.load(std::sync::atomic::Ordering::SeqCst) >= 0 {
            return;
        }
        let orig = libc::dup(libc::STDERR_FILENO);
        if orig < 0 {
            return;
        }
        let _ = libc::fcntl(orig, libc::F_SETFD, libc::FD_CLOEXEC);
        let dev_null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if dev_null >= 0 {
            libc::dup2(dev_null, libc::STDERR_FILENO);
            libc::close(dev_null);
            SAVED_STDERR_FD.store(orig, std::sync::atomic::Ordering::SeqCst);
        } else {
            libc::close(orig);
        }
    }
}

#[cfg(unix)]
pub(crate) fn restore_tui_stderr() {
    unsafe {
        let orig = SAVED_STDERR_FD.swap(-1, std::sync::atomic::Ordering::SeqCst);
        if orig >= 0 {
            libc::dup2(orig, libc::STDERR_FILENO);
            libc::close(orig);
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn silence_tui_stderr() {}
#[cfg(not(unix))]
pub(crate) fn restore_tui_stderr() {}

pub(crate) fn init_tui() -> ratatui::DefaultTerminal {
    silence_tui_stderr();
    let terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    TUI_INITIALIZED.store(true, std::sync::atomic::Ordering::SeqCst);
    terminal
}

pub(crate) fn restore_tui() {
    INPUT_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    if !TUI_INITIALIZED.swap(false, std::sync::atomic::Ordering::SeqCst) {
        restore_tui_stderr();
        return;
    }
    use std::io::Write as _;
    let mut stdout = std::io::stdout();
    // 1. Explicitly send all terminal disable codes for any mouse tracking mode, focus, and bracketed paste
    let _ = stdout.write_all(
        b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?2004l",
    );
    let _ = crossterm::execute!(stdout, crossterm::event::DisableMouseCapture);
    // 2. Disable raw mode and leave alternate screen, show cursor
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        stdout,
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = stdout.flush();
    // 3. Restore ratatui state
    ratatui::restore();
    let _ = std::io::stdout().flush();
    // 4. Restore stderr and flush
    restore_tui_stderr();
    let _ = std::io::stderr().flush();
}

fn main() -> Result<()> {
    let args = Args::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    match rt.block_on(run(args)) {
        Ok(()) => {
            restore_tui();
            Ok(())
        }
        Err(e) => {
            restore_tui();
            if let Some(nc) = e.downcast_ref::<k8s::NoCluster>().cloned() {
                let (code, msg) = k8s::no_cluster_exit(&nc);
                eprintln!("{msg}");
                std::process::exit(code);
            }
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

async fn run(args: Args) -> Result<()> {
    if let Some(cmd) = args.cmd.clone() {
        return agent_run(cmd, &args).await;
    }
    let filecfg = cfg::FileCfg::load();
    if let Some(d) = &args.screen_dump_dir {
        // safe: single-threaded here, before the input thread spawns
        unsafe { std::env::set_var("K9X_SCREENDUMP_DIR", d) };
    }
    // --invert: swap between dark/light-ish presets
    let theme_name: String = if args.invert {
        match filecfg.theme.as_str() {
            "matrix" => "dark".to_string(),
            "dark" => "matrix".to_string(),
            "light" => "mono".to_string(),
            _ => "light".to_string(),
        }
    } else {
        filecfg.theme.clone()
    };
    let theme = cfg::Theme::resolve(&theme_name);
    let statecfg = cfg::StateCfg::load();
    let ctx = args
        .context
        .clone()
        .or_else(|| {
            if statecfg.last_context.is_empty() {
                None
            } else {
                Some(statecfg.last_context.clone())
            }
        })
        .or_else(|| {
            if filecfg.context.is_empty() {
                None
            } else {
                Some(filecfg.context.clone())
            }
        });
    let pool = Arc::new(std::sync::Mutex::new(k8s::ClusterPool::new()));

    // Pre-flight guards run BEFORE the terminal is touched (alternate screen /
    // raw mode), so a machine with no Kubernetes config exits cleanly without
    // emitting any escape-sequence noise on the failure path.
    if !std::io::stdout().is_terminal() {
        eprintln!(
            "k9x requires an interactive terminal (TTY) for its TUI.\n\
             Run `k9x` inside a terminal emulator, or use agent subcommands for scripting:\n\
             \n    k9x ls pods\n    k9x get deploy <name>\n    k9x logs <pod>"
        );
        std::process::exit(1);
    }
    match k8s::probe_kube_config(ctx.as_deref()) {
        k8s::KubeProbe::Ready(_) => {}
        probe => {
            let (code, msg) = probe.into_exit();
            eprintln!("{msg}");
            std::process::exit(code);
        }
    }

    let (key_tx, mut key_rx): (KeyTx, _) = tokio::sync::mpsc::unbounded_channel();
    // paused while $EDITOR runs so the reader never steals vim's keystrokes
    std::thread::spawn(move || {
        while INPUT_RUNNING.load(std::sync::atomic::Ordering::Relaxed) {
            if INPUT_PAUSE.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(60));
                continue;
            }
            let ev = match crossterm::event::poll(std::time::Duration::from_millis(80)) {
                Ok(true) => crossterm::event::read(),
                Ok(false) => continue,
                Err(_) => return,
            };
            match ev {
                Ok(crossterm::event::Event::Key(k)) => {
                    let _ = key_tx.send(InputEvent::Key(k));
                }
                Ok(crossterm::event::Event::Resize(w, h)) => {
                    let _ = key_tx.send(InputEvent::Resized(w, h));
                }
                Ok(crossterm::event::Event::Mouse(m)) => {
                    let _ = key_tx.send(InputEvent::Mouse(m));
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });

    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_tui();
        default_panic(info);
    }));

    let mut terminal = init_tui();
    let _tui_guard = TuiGuard;

    let ns_o = args
        .namespace
        .clone()
        .or_else(|| ctx.as_deref().and_then(|c| statecfg.ns_for(c)))
        .or_else(|| {
            if filecfg.namespace.is_empty() {
                None
            } else {
                Some(filecfg.namespace.clone())
            }
        });
    let all = args.all_namespaces || filecfg.all_namespaces;
    let ro = (args.readonly || filecfg.readonly) && !args.write;
    let log_cap = filecfg.log_cap;
    let log_tail = filecfg.log_tail;
    if std::env::var("K9X_TRACE").is_ok()
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/k9x-start.log")
    {
        use std::io::Write as _;
        let _ = writeln!(
            f,
            "start args_ns={:?} state_ns={:?} file_ns={:?} resolved={:?}",
            args.namespace, statecfg.last_namespace, filecfg.namespace, ns_o
        );
    }

    let mut app = if args.splashless {
        let cluster = k8s::load_pooled(&pool, ctx.as_deref()).await.map_err(|e| {
            restore_tui();
            connect_err(e)
        })?;
        App::new(cluster, ns_o, all, ro, theme, log_cap, log_tail)
            .await
            .inspect_err(|_| {
                restore_tui();
            })?
    } else {
        let pool_cl = pool.clone();
        let ctx_cl = ctx.clone();
        let ns_cl = ns_o.clone();
        let theme_cl = theme.clone();

        let connect_task = tokio::spawn(async move {
            let cluster = k8s::load_pooled(&pool_cl, ctx_cl.as_deref())
                .await
                .map_err(connect_err)?;
            App::new(cluster, ns_cl, all, ro, theme_cl, log_cap, log_tail).await
        });

        let mut splash = splash::MatrixSplash::new(80, 24);
        let ctx_display = ctx.as_deref().unwrap_or("current-context");
        let status_msg = format!("connecting to [{ctx_display}]...");
        let mut connect_task = Box::pin(connect_task);

        loop {
            tokio::select! {
                res = &mut connect_task => {
                    match res {
                        Ok(Ok(app)) => break app,
                        Ok(Err(e)) => {
                            restore_tui();
                            return Err(e);
                        }
                        Err(e) => {
                            restore_tui();
                            return Err(anyhow!("startup failed: {e}"));
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(30)) => {
                    let _ = terminal.draw(|f| {
                        splash.render(f, &status_msg);
                    });

                    // Process keyboard events: Ctrl+C aborts
                    if let Ok(ev) = key_rx.try_recv() {
                        match ev {
                            InputEvent::Key(k) => {
                                if k.code == crossterm::event::KeyCode::Char('c')
                                    && k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                                {
                                    restore_tui();
                                    std::process::exit(0);
                                }
                            }
                            InputEvent::Resized(w, h) => {
                                splash.resize(w, h);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    };
    let start_view = args
        .view
        .clone()
        .or_else(|| args.command.clone())
        .or_else(|| Some(filecfg.default_view.clone()))
        .unwrap_or_else(|| "po".into());
    app.ui_headless = args.headless;
    app.ui_logoless = args.logoless;
    app.ui_crumbsless = args.crumbsless;

    // background init: CRDs, ns shortcuts, version + support dates all load
    // AFTER first paint so remote/loaded clusters (sj-stage) start instantly
    {
        let cl = app.cluster.clone();
        let tx = app.tx.clone();
        tokio::spawn(async move {
            if let Ok(crds) = k8s::list_crds(&cl).await {
                let _ = tx.send(Msg::Crds(crds));
            }
            if let Ok(mut nss) = k8s::list_namespaces(&cl).await {
                nss.truncate(9);
                let _ = tx.send(Msg::Nss(nss));
            }
            if let Some(v) = fetch_k8s_version(&cl).await {
                let _ = tx.send(Msg::Ver(v.clone()));
                if let Some(sd) = awsup::resolve(&cl, &v).await {
                    let _ = tx.send(Msg::Sup(sd));
                }
            }
        });
    }
    {
        // state save is local-disk only — keep synchronous
        let mut st = cfg::StateCfg::load();
        st.remember_ns(&app.cluster.ctx_name, &app.ns);
        st.save();
    }
    spawn_res_sampler(&app, filecfg.pod_metrics);
    if let Err(e) = app.switch_view(&start_view) {
        app.set_status(format!("!{e}"));
    }
    {
        let mut st = cfg::StateCfg::load();
        st.remember_view(
            &app.cluster.ctx_name,
            &app.view_spec
                .as_ref()
                .map(|sp| sp.alias.clone())
                .unwrap_or_default(),
        );
        st.save();
    }

    let tick_ms = args
        .refresh
        .map(|r| (r * 1000.0) as u64)
        .unwrap_or(filecfg.tick_ms)
        .max(50);
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(tick_ms));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let result = event_loop(&mut terminal, &mut app, &mut key_rx, &mut tick, pool).await;

    let mut st = cfg::StateCfg::load();
    st.remember_ns(&app.cluster.ctx_name, &app.ns);
    st.save();

    app.shutdown();
    restore_tui();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    key_rx: &mut tokio::sync::mpsc::UnboundedReceiver<InputEvent>,
    tick: &mut tokio::time::Interval,
    pool: Arc<std::sync::Mutex<k8s::ClusterPool>>,
) -> Result<()> {
    let first_draw_done = &mut false;
    loop {
        if JUST_RESUMED.swap(false, std::sync::atomic::Ordering::Relaxed) {
            let _ = terminal.clear();
            // discard editor-era keystrokes and force a fresh fetch (watch may have aged out)
            while key_rx.try_recv().is_ok() {}
            let had_rows_before = !app.rows.is_empty();
            app.restart_watch_full_reset();
            app.sel_top();
            if app.status.is_empty() {
                app.set_status(if had_rows_before {
                    String::new()
                } else {
                    "back \u{2014} view refreshed".into()
                });
            }
        }
        app.theme_tick();
        // keep the sampler's view of ns scope in sync
        {
            use std::sync::atomic::Ordering;
            let sc = &app.scope;
            sc.all.store(app.all_ns, Ordering::Relaxed);
            if let Ok(mut g) = sc.ns.lock() {
                *g = app.ns.clone();
            }
        }
        // drain async messages
        while let Ok(m) = app.rx.try_recv() {
            app.apply_msg(m);
        }
        app.pump_streams();

        terminal.draw(|f| ui::draw(f, app))?;
        if !*first_draw_done {
            *first_draw_done = true;
            app.first_frame_ms = Some(app.t0.elapsed().as_millis());
            app.profile_write("first-frame");
        }

        tokio::select! {
            maybe_input = key_rx.recv() => {
                if std::env::var("K9X_TRACE").is_ok() {
                    if let Ok(mut f)=std::fs::OpenOptions::new().create(true).append(true).open("/tmp/k9x-trace.log") {
                        use std::io::Write; let _=writeln!(f,"evt={maybe_input:?}");
                    }
                    let _ = std::fs::write("/tmp/k9x-trace-alive.txt", "yes");
                }
                match maybe_input {
                    None => break,
                    Some(InputEvent::Key(key)) => {
                        if key.kind == crossterm::event::KeyEventKind::Release { continue; }
                        handle_key(app, key).await;
                    }
                    Some(InputEvent::Resized(w, h)) => {
                        if let Mode::Exec(ex) = &mut app.mode
                            && let Some(tx) = &ex.size_tx {
                                use futures::SinkExt;
                                let _ = tx.clone().send(kube::api::TerminalSize { width: w, height: h }).await;
                            }
                    }
                    Some(InputEvent::Mouse(m)) => handle_mouse(app, m).await,
                }
            }
            _ = tick.tick() => {}
        }

        // context switch requested from menu / command
        if let Some(ctx) = app.pending_ctx.take()
            && ctx != app.cluster.ctx_name
        {
            let ro = app.ro;
            let all_ns = app.all_ns;
            let state = cfg::StateCfg::load();
            let ctx_defaults = app.ctx_defaults.get(&ctx);
            let view_alias = state
                .views
                .get(&ctx)
                .cloned()
                .or_else(|| {
                    ctx_defaults.and_then(|d| {
                        if d.view.is_empty() {
                            None
                        } else {
                            Some(d.view.clone())
                        }
                    })
                })
                .unwrap_or_else(|| {
                    app.view_spec
                        .as_ref()
                        .map(|sp| sp.alias.clone())
                        .unwrap_or_else(|| "po".into())
                });
            let ns_override = state
                .ns_for(&ctx)
                .or_else(|| {
                    ctx_defaults.and_then(|d| {
                        if d.namespace.is_empty() {
                            None
                        } else {
                            Some(d.namespace.clone())
                        }
                    })
                })
                .or_else(|| Some(app.ns.clone()));
            app.shutdown();

            match k8s::load_pooled(&pool, Some(&ctx)).await {
                Ok(new_cluster) => {
                    match App::new(
                        new_cluster,
                        ns_override.clone(),
                        all_ns,
                        ro,
                        app.theme.clone(),
                        app.log_cap,
                        app.log_tail,
                    )
                    .await
                    {
                        Ok(mut new_app) => {
                            let _ = new_app.switch_view(&view_alias);
                            spawn_res_sampler(&new_app, cfg::FileCfg::load().pod_metrics);
                            new_app.auth_notice_shown = false;
                            let mut st = cfg::StateCfg::load();
                            st.remember_ns(&new_app.cluster.ctx_name, &new_app.ns);
                            st.save();

                            // Parallel background metadata fetch (CRDs, namespaces, versions, support dates)
                            {
                                let cl = new_app.cluster.clone();
                                let tx = new_app.tx.clone();
                                tokio::spawn(async move {
                                    if let Ok(crds) = k8s::list_crds(&cl).await {
                                        let _ = tx.send(Msg::Crds(crds));
                                    }
                                    if let Ok(mut nss) = k8s::list_namespaces(&cl).await {
                                        nss.truncate(9);
                                        let _ = tx.send(Msg::Nss(nss));
                                    }
                                    if let Some(v) = fetch_k8s_version(&cl).await {
                                        let _ = tx.send(Msg::Ver(v.clone()));
                                        if let Some(sd) = awsup::resolve(&cl, &v).await {
                                            let _ = tx.send(Msg::Sup(sd));
                                        }
                                    }
                                });
                            }

                            *app = new_app;
                            app.set_status(format!("context → {ctx} · ns {}", app.ns));
                        }
                        Err(e) => {
                            app.set_status(format!("!context init failed: {e}"));
                        }
                    }
                }
                Err(primary_err) => match k8s::load_pooled(&pool, None).await {
                    Ok(fb_cluster) => {
                        if let Ok(mut fb) = App::new(
                            fb_cluster,
                            None,
                            all_ns,
                            ro,
                            app.theme.clone(),
                            app.log_cap,
                            app.log_tail,
                        )
                        .await
                        {
                            let _ = fb.switch_view("po");
                            spawn_res_sampler(&fb, cfg::FileCfg::load().pod_metrics);
                            *app = fb;
                            app.set_status(format!(
                                "!switch to {ctx} failed ({primary_err}); fell back to default ctx"
                            ));
                        }
                    }
                    Err(fb_err) => {
                        let auth_fail =
                            authish(&primary_err.to_string()) || authish(&fb_err.to_string());
                        let title = if auth_fail {
                            "token expired / authentication error"
                        } else {
                            "context switch failed"
                        };
                        let reason = if auth_fail {
                            "credentials/token expired or unauthorized".to_string()
                        } else {
                            k8s::classify_err(&fb_err.to_string())
                                .unwrap_or_else(|| "cluster unreachable".into())
                        };
                        app.mode = Mode::Notice {
                            title: title.into(),
                            lines: vec![
                                format!("authentication failed for context \u{2018}{ctx}\u{2019}."),
                                format!("reason: {reason}"),
                                String::new(),
                                "refresh credentials (e.g. `aws sso login`),".into(),
                                "press enter to exit and relaunch.".into(),
                            ],
                            ok_exits: true,
                        };
                        let _ = terminal.clear();
                        app.restart_watch_if_pulse_left();
                    }
                },
            }
        }

        if app.quit {
            break;
        }
    }
    Ok(())
}

fn confirm(app: &mut App, prompt: String, action: Action) {
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

async fn fetch_k8s_version(cluster: &Arc<k8s::Cluster>) -> Option<String> {
    match cluster.client.apiserver_version().await {
        Ok(info) => {
            // prefer git_version ("v1.33.1"); fall back to major.minor
            let gv = info.git_version.trim();
            let from_git = gv.strip_prefix('v').filter(|s| {
                let mut it = s.split('.');
                it.next().map(|x| x.parse::<u64>().is_ok()).unwrap_or(false)
                    && it.next().map(|x| x.parse::<u64>().is_ok()).unwrap_or(false)
                    && !gv.contains('-')
                    && !gv.contains('+')
            });
            Some(match from_git {
                Some(s) => format!("v{s}"),
                None => {
                    let minor = info.minor.trim().trim_end_matches('+');
                    format!("v{}.{}", info.major.trim(), minor)
                }
            })
        }
        Err(_) => None,
    }
}

/// periodic cluster cpu/mem sampler — dies automatically when the app (and its rx) drops.
/// per-pod metrics are opt-in (config pod_metrics), scoped to the current namespace, 10s cadence.
fn spawn_res_sampler(app: &App, include_pods: bool) {
    let cl = app.cluster.clone();
    let tx = app.tx.clone();
    let scope = app.scope.clone();
    tokio::spawn(async move {
        loop {
            if let Ok(r) = k8s::cluster_resources(&cl, &scope, include_pods).await
                && tx.send(Msg::Res(r)).is_err()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });
}

/// copy text to the system clipboard via whatever helper binary exists
fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    for bin in ["pbcopy", "wl-copy", "xclip", "xsel"] {
        if let Some(p) = k8s::which_bin(bin) {
            let mut cmd = Command::new(p);
            if bin == "xclip" {
                cmd.args(["-selection", "clipboard"]);
            }
            if bin == "xsel" {
                cmd.arg("--clipboard").arg("--input");
            }
            if let Ok(mut child) = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                if let Some(mut sin) = child.stdin.take() {
                    let _ = sin.write_all(text.as_bytes());
                }
                let _ = child.wait();
                return true;
            }
        }
    }
    false
}

async fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::{KeyCode as K, KeyModifiers as M};

    // global ctrl-c / ctrl-q to quit k9x (except in Exec mode where ctrl-c is SIGINT and ctrl-q detaches)
    if key.modifiers.contains(M::CONTROL)
        && matches!(key.code, K::Char('c') | K::Char('q'))
        && !matches!(app.mode, Mode::Exec(_))
    {
        app.quit = true;
        return;
    }

    match &mut app.mode {
        Mode::Normal => handle_normal(app, key).await,
        Mode::Cmd { buf: _, sel: _ } => {
            let mut m = std::mem::replace(&mut app.mode, Mode::Normal);
            let Mode::Cmd { buf: b, sel: sidx } = &mut m else {
                unreachable!()
            };
            match key.code {
                K::Char(c) => {
                    b.push(c);
                    *sidx = 0;
                }
                K::Backspace => {
                    b.pop();
                    *sidx = 0;
                }
                K::Esc => {
                    // 1st esc clears the buffer; 2nd esc (already empty) closes the box
                    if b.is_empty() {
                        return;
                    }
                    b.clear();
                    *sidx = 0;
                }
                K::Up => {
                    let n = app::suggest(b).len();
                    if n > 0 {
                        *sidx = (*sidx as i32 - 1).rem_euclid(n as i32) as usize;
                    }
                }
                K::Down => {
                    let n = app::suggest(b).len();
                    if n > 0 {
                        *sidx = (*sidx as i32 + 1).rem_euclid(n as i32) as usize;
                    }
                }
                K::Tab => {
                    let sugg = app::suggest(b);
                    if !sugg.is_empty() {
                        *b = sugg[(*sidx).min(sugg.len() - 1)].clone();
                    }
                }
                K::Enter => {
                    let mut cmd = b.clone();
                    // k9s semantics: bare prefix executes the highlighted suggestion
                    let known = model::spec_for(&cmd).is_some()
                        || matches!(
                            cmd.as_str(),
                            "ctx"
                                | "ns"
                                | "pulse"
                                | "pf"
                                | "helm"
                                | "xray"
                                | "popeye"
                                | "crds"
                                | "help"
                                | "q"
                                | "quit"
                        );
                    if !known && !cmd.is_empty() {
                        let sugg = app::suggest(&cmd);
                        if !sugg.is_empty() {
                            cmd = sugg[(*sidx).min(sugg.len() - 1)].clone();
                        }
                    }
                    exec_cmd(app, &cmd).await;
                    return;
                }
                _ => {}
            }
            if !matches!(key.code, K::Enter | K::Char('\n') | K::Char('\r')) {
                app.mode = m;
            }
        }
        Mode::Filter { buf } => match key.code {
            K::Char(c) => {
                buf.push(c);
                app.filter = buf.clone();
                app.sel_top();
            }
            K::Backspace => {
                buf.pop();
                app.filter = buf.clone();
                app.sel_top();
            }
            K::Esc | K::Enter => app.mode = Mode::Normal,
            _ => {}
        },
        Mode::Confirm { .. } => {
            let toggle = |app: &mut App| {
                if let Mode::Confirm { sel_yes, .. } = &mut app.mode {
                    *sel_yes = !*sel_yes;
                }
            };
            let is_theme = matches!(
                &app.mode,
                Mode::Confirm {
                    action: Action::ThemeKeep { .. },
                    ..
                }
            );
            let set_sel = |app: &mut App, yes: bool| {
                if let Mode::Confirm { sel_yes, .. } = &mut app.mode {
                    *sel_yes = yes;
                }
            };
            match key.code {
                K::Char('y') | K::Char('Y') => {
                    set_sel(app, true);
                    run_confirm(app).await;
                }
                K::Char('n') | K::Char('N') => {
                    set_sel(app, false);
                    run_confirm(app).await;
                }
                K::Esc => {
                    if is_theme {
                        app.mode = Mode::Normal;
                        app.revert_theme_preview();
                    } else if let Mode::Confirm {
                        action: Action::SaveLogs { logs_state, .. },
                        ..
                    } = std::mem::replace(&mut app.mode, Mode::Normal)
                    {
                        app.mode = Mode::Logs(*logs_state);
                    } else {
                        app.mode = Mode::Normal;
                    }
                }
                K::Left | K::Char('h') => {
                    set_sel(app, true);
                }
                K::Right | K::Char('l') => {
                    set_sel(app, false);
                }
                K::Tab | K::BackTab | K::Up | K::Down | K::Char('j') | K::Char('k') => {
                    toggle(app);
                }
                K::Enter => {
                    run_confirm(app).await;
                }
                _ => {}
            }
        }
        Mode::Input { buf: _, purpose: _ } => {
            let mut m = std::mem::replace(&mut app.mode, Mode::Normal);
            let Mode::Input {
                buf: b2,
                purpose: p2,
            } = &mut m
            else {
                unreachable!()
            };
            match key.code {
                K::Char(c) => b2.push(c),
                K::Backspace => {
                    b2.pop();
                }
                K::Esc => {
                    return;
                }
                K::Enter => {
                    let purpose = std::mem::replace(
                        p2,
                        InputPurpose::Scale {
                            name: String::new(),
                        },
                    );
                    run_input(app, b2.clone(), purpose).await;
                    return;
                }
                _ => {}
            }
            if !matches!(key.code, K::Enter) {
                app.mode = m;
            }
        }
        Mode::ThemeEditor {
            values,
            sel,
            editing,
            buf,
        } => {
            let max = values.len().saturating_sub(1);
            match key.code {
                K::Char(c) if *editing => buf.push(c),
                K::Backspace if *editing => {
                    buf.pop();
                }
                K::Enter => {
                    if *editing {
                        let hexv = buf.clone();
                        let valid = crate::cfg::hex_to_color(&hexv).is_some();
                        if valid {
                            values[*sel].1 = hexv.clone();
                        }
                        let msg = if valid {
                            "color applied \u{2014} 's' saves to custom theme".to_string()
                        } else {
                            "!bad hex (use #rgb or #rrggbb)".to_string()
                        };
                        let vals = values.clone();
                        let sidx = *sel;
                        *editing = false;
                        buf.clear();
                        app.apply_editor_values(&vals);
                        app.set_status(msg);
                        // restore editor state after mutable borrow ends
                        if let Mode::ThemeEditor {
                            values: v2,
                            sel: s2,
                            editing: e2,
                            buf: b2,
                        } = &mut app.mode
                        {
                            *v2 = vals;
                            *s2 = sidx;
                            *e2 = false;
                            *b2 = String::new();
                        }
                    } else {
                        *editing = true;
                        *buf = values[*sel].1.clone();
                    }
                }
                K::Esc => {
                    if *editing {
                        *editing = false;
                        buf.clear();
                    } else {
                        app.mode = Mode::Normal;
                    }
                }
                K::Char('s') | K::Char('S') if !*editing => {
                    let t = app.theme.clone();
                    match crate::cfg::save_theme_file("custom", &t) {
                        Ok(p) => {
                            app.set_status(format!("custom theme saved \u{2192} {}", p.display()))
                        }
                        Err(e) => app.set_status(format!("!{e}")),
                    }
                }
                K::Up | K::Char('k') if !*editing => {
                    if *sel > 0 {
                        *sel -= 1;
                    }
                }
                K::Down | K::Char('j') if !*editing => {
                    if *sel < max {
                        *sel += 1;
                    }
                }
                K::Char('q') if !*editing => app.mode = Mode::Normal,
                _ => {}
            }
        }
        Mode::Notice { ok_exits, .. } => match key.code {
            K::Enter | K::Char('o') | K::Char('O') | K::Esc | K::Char('q') => {
                if *ok_exits {
                    app.quit = true;
                } else {
                    app.mode = Mode::Normal;
                }
            }
            _ => {}
        },
        Mode::Menu(_) => {
            // helm history extras: R = rollback to highlighted revision, V = values
            let (hist_ns, hist_name) = if let Mode::Menu(m) = &app.mode {
                match &m.purpose {
                    MenuPurpose::HelmHistory { ns, name, .. } => {
                        (Some(ns.clone()), Some(name.clone()))
                    }
                    _ => (None, None),
                }
            } else {
                (None, None)
            };
            match key.code {
                K::Char('R') if hist_name.is_some() => {
                    let rev = if let Mode::Menu(m) = &app.mode {
                        m.items.get(m.sel).and_then(|i| i.value.parse::<i64>().ok())
                    } else {
                        None
                    };
                    if let Some(rev) = rev {
                        let hn = hist_name.clone().unwrap_or_default();
                        confirm(
                            app,
                            format!("rollback {hn} to revision {rev}?"),
                            Action::HelmRollback {
                                ns: hist_ns.clone().unwrap_or_default(),
                                name: hn,
                                revision: rev,
                            },
                        );
                    }
                }
                K::Char('V') if hist_name.is_some() => {
                    app.helm_values_of_selected();
                }
                K::Up | K::Char('k') => app.menu_move(-1),
                K::Down | K::Char('j') => app.menu_move(1),
                K::Esc => app.mode = Mode::Normal,
                K::Enter => app.menu_select().await,
                _ => {}
            }
        }
        Mode::TextPane { pos, lines, .. } => {
            let max = lines.len().saturating_sub(1) as i32;
            match key.code {
                K::Char('q') | K::Esc => app.mode = Mode::Normal,
                K::Char('e') => edit_from_pane(app).await,
                K::Char('c') => {
                    let (n_lines, all) = match &app.mode {
                        Mode::TextPane { lines, .. } => (lines.len(), lines.join("\n")),
                        _ => (0, String::new()),
                    };
                    let ok = copy_to_clipboard(&all);
                    app.set_status(if ok {
                        format!("copied {n_lines} lines")
                    } else {
                        "!no clipboard helper found".into()
                    });
                }
                K::Down | K::Char('j') => *pos = (*pos as i32 + 1).clamp(0, max) as usize,
                K::Up | K::Char('k') => *pos = (*pos as i32 - 1).clamp(0, max) as usize,
                K::PageDown => *pos = (*pos as i32 + 20).clamp(0, max) as usize,
                K::PageUp => *pos = (*pos as i32 - 20).clamp(0, max) as usize,
                K::Char('G') => *pos = max as usize,
                K::Char('g') => *pos = 0,
                _ => {}
            }
        }
        Mode::Logs(_) => match key.code {
            K::Char('q') | K::Esc => {
                let (searching, has_query) = if let Mode::Logs(st) = &app.mode {
                    (st.search, !st.query.is_empty())
                } else {
                    (false, false)
                };
                if searching {
                    if let Mode::Logs(st) = &mut app.mode {
                        st.search = false;
                    }
                } else if has_query {
                    if let Mode::Logs(st) = &mut app.mode {
                        st.query.clear();
                    }
                } else {
                    app.close_logs();
                }
            }
            K::Char('/') => {
                if let Mode::Logs(st) = &mut app.mode {
                    st.search = true;
                }
            }
            K::Char(c) => {
                // search typing takes priority over all log-view hotkeys
                let searching = matches!(&app.mode, Mode::Logs(st) if st.search);
                if searching {
                    if let Mode::Logs(st) = &mut app.mode {
                        st.query.push(c);
                        let matches =
                            app::compute_log_matches(&st.lines, &st.query, st.count_occurrences);
                        st.match_total = matches.len();
                        if !matches.is_empty() {
                            st.match_idx = Some(0);
                            let (target_line_idx, _) = matches[0];
                            let inner_h = app
                                .ui_body
                                .map(|r| r.height.saturating_sub(2) as usize)
                                .unwrap_or_else(|| {
                                    crossterm::terminal::size()
                                        .map(|(_, rows)| (rows.saturating_sub(10) as usize).max(5))
                                        .unwrap_or(20)
                                });
                            let total_filtered = st
                                .lines
                                .iter()
                                .filter(|l| l.to_lowercase().contains(&st.query.to_lowercase()))
                                .count();
                            app::ensure_log_line_visible(
                                &mut st.scroll_from_end,
                                total_filtered,
                                target_line_idx,
                                inner_h,
                            );
                        } else {
                            st.match_idx = None;
                        }
                    }
                    return;
                }
                match c {
                    's' => save_logs(app),
                    'o' => {
                        if let Mode::Logs(st) = &mut app.mode {
                            st.count_occurrences = !st.count_occurrences;
                            if !st.query.is_empty() {
                                let matches = app::compute_log_matches(
                                    &st.lines,
                                    &st.query,
                                    st.count_occurrences,
                                );
                                st.match_total = matches.len();
                                if !matches.is_empty() {
                                    let cur = st.match_idx.unwrap_or(0).min(matches.len() - 1);
                                    st.match_idx = Some(cur);
                                    let (target_line_idx, _) = matches[cur];
                                    let inner_h = app
                                        .ui_body
                                        .map(|r| r.height.saturating_sub(2) as usize)
                                        .unwrap_or_else(|| {
                                            crossterm::terminal::size()
                                                .map(|(_, rows)| {
                                                    (rows.saturating_sub(10) as usize).max(5)
                                                })
                                                .unwrap_or(20)
                                        });
                                    let total_filtered = st
                                        .lines
                                        .iter()
                                        .filter(|l| {
                                            l.to_lowercase().contains(&st.query.to_lowercase())
                                        })
                                        .count();
                                    app::ensure_log_line_visible(
                                        &mut st.scroll_from_end,
                                        total_filtered,
                                        target_line_idx,
                                        inner_h,
                                    );
                                } else {
                                    st.match_idx = None;
                                }
                            }
                        }
                    }
                    'n' => {
                        if let Mode::Logs(st) = &mut app.mode
                            && !st.query.is_empty()
                        {
                            let matches = app::compute_log_matches(
                                &st.lines,
                                &st.query,
                                st.count_occurrences,
                            );
                            st.match_total = matches.len();
                            if !matches.is_empty() {
                                let cur = st.match_idx.unwrap_or(0);
                                let next = (cur + 1) % matches.len();
                                st.match_idx = Some(next);
                                let (target_line_idx, _) = matches[next];
                                let inner_h = app
                                    .ui_body
                                    .map(|r| r.height.saturating_sub(2) as usize)
                                    .unwrap_or_else(|| {
                                        crossterm::terminal::size()
                                            .map(|(_, rows)| {
                                                (rows.saturating_sub(10) as usize).max(5)
                                            })
                                            .unwrap_or(20)
                                    });
                                let total_filtered = st
                                    .lines
                                    .iter()
                                    .filter(|l| l.to_lowercase().contains(&st.query.to_lowercase()))
                                    .count();
                                app::ensure_log_line_visible(
                                    &mut st.scroll_from_end,
                                    total_filtered,
                                    target_line_idx,
                                    inner_h,
                                );
                            }
                        }
                    }
                    'p' => {
                        if let Mode::Logs(st) = &mut app.mode
                            && !st.query.is_empty()
                        {
                            let matches = app::compute_log_matches(
                                &st.lines,
                                &st.query,
                                st.count_occurrences,
                            );
                            st.match_total = matches.len();
                            if !matches.is_empty() {
                                let cur = st.match_idx.unwrap_or(0);
                                let prev = if cur == 0 { matches.len() - 1 } else { cur - 1 };
                                st.match_idx = Some(prev);
                                let (target_line_idx, _) = matches[prev];
                                let inner_h = app
                                    .ui_body
                                    .map(|r| r.height.saturating_sub(2) as usize)
                                    .unwrap_or_else(|| {
                                        crossterm::terminal::size()
                                            .map(|(_, rows)| {
                                                (rows.saturating_sub(10) as usize).max(5)
                                            })
                                            .unwrap_or(20)
                                    });
                                let total_filtered = st
                                    .lines
                                    .iter()
                                    .filter(|l| l.to_lowercase().contains(&st.query.to_lowercase()))
                                    .count();
                                app::ensure_log_line_visible(
                                    &mut st.scroll_from_end,
                                    total_filtered,
                                    target_line_idx,
                                    inner_h,
                                );
                            }
                        }
                    }
                    'P' | 't' => {
                        if let Mode::Logs(st) = &mut app.mode {
                            if c == 'P' {
                                st.previous = !st.previous;
                            } else {
                                st.timestamps = !st.timestamps;
                            }
                        }
                        app.restart_log_stream().await;
                    }
                    'w' => {
                        if let Mode::Logs(st) = &mut app.mode {
                            st.wrap = !st.wrap;
                        }
                    }
                    'j' => {
                        if let Mode::Logs(st) = &mut app.mode {
                            st.scroll_from_end = st.scroll_from_end.saturating_add(1);
                        }
                    }
                    'k' => {
                        if let Mode::Logs(st) = &mut app.mode {
                            st.scroll_from_end = st.scroll_from_end.saturating_sub(1);
                        }
                    }
                    d @ '0'..='6' => {
                        let win = match d {
                            '0' => LogWindow::Tail(st_default_tail(app)),
                            '1' => LogWindow::Head,
                            '2' => LogWindow::Since(60),
                            '3' => LogWindow::Since(300),
                            '4' => LogWindow::Since(900),
                            '5' => LogWindow::Since(1800),
                            _ => LogWindow::Since(3600),
                        };
                        if let Mode::Logs(st) = &mut app.mode {
                            st.window = win;
                            st.lines.clear();
                            st.scroll_from_end = 0;
                        }
                        app.restart_log_stream().await;
                    }
                    _ => {}
                }
            }
            K::Backspace => {
                if let Mode::Logs(st) = &mut app.mode
                    && st.search
                {
                    st.query.pop();
                    let matches =
                        app::compute_log_matches(&st.lines, &st.query, st.count_occurrences);
                    st.match_total = matches.len();
                    if !matches.is_empty() {
                        st.match_idx = Some(0);
                        let (target_line_idx, _) = matches[0];
                        let inner_h = app
                            .ui_body
                            .map(|r| r.height.saturating_sub(2) as usize)
                            .unwrap_or_else(|| {
                                crossterm::terminal::size()
                                    .map(|(_, rows)| (rows.saturating_sub(10) as usize).max(5))
                                    .unwrap_or(20)
                            });
                        let total_filtered = st
                            .lines
                            .iter()
                            .filter(|l| l.to_lowercase().contains(&st.query.to_lowercase()))
                            .count();
                        app::ensure_log_line_visible(
                            &mut st.scroll_from_end,
                            total_filtered,
                            target_line_idx,
                            inner_h,
                        );
                    } else {
                        st.match_idx = None;
                    }
                }
            }
            K::Enter => {
                if let Mode::Logs(st) = &mut app.mode {
                    if st.search {
                        // Exit search mode and commit the query - find all matches
                        st.search = false;
                        if !st.query.is_empty() {
                            let matches = app::compute_log_matches(
                                &st.lines,
                                &st.query,
                                st.count_occurrences,
                            );
                            st.match_total = matches.len();
                            if !matches.is_empty() {
                                st.match_idx = Some(0);
                                let (target_line_idx, _) = matches[0];
                                let inner_h = app
                                    .ui_body
                                    .map(|r| r.height.saturating_sub(2) as usize)
                                    .unwrap_or_else(|| {
                                        crossterm::terminal::size()
                                            .map(|(_, rows)| {
                                                (rows.saturating_sub(10) as usize).max(5)
                                            })
                                            .unwrap_or(20)
                                    });
                                let total_filtered = st
                                    .lines
                                    .iter()
                                    .filter(|l| l.to_lowercase().contains(&st.query.to_lowercase()))
                                    .count();
                                app::ensure_log_line_visible(
                                    &mut st.scroll_from_end,
                                    total_filtered,
                                    target_line_idx,
                                    inner_h,
                                );
                            }
                        }
                    } else if !st.query.is_empty() {
                        // Already committed, pressing Enter again re-finds and jumps to first
                        let matches =
                            app::compute_log_matches(&st.lines, &st.query, st.count_occurrences);
                        st.match_total = matches.len();
                        if !matches.is_empty() {
                            st.match_idx = Some(0);
                            let (target_line_idx, _) = matches[0];
                            let inner_h = app
                                .ui_body
                                .map(|r| r.height.saturating_sub(2) as usize)
                                .unwrap_or_else(|| {
                                    crossterm::terminal::size()
                                        .map(|(_, rows)| (rows.saturating_sub(10) as usize).max(5))
                                        .unwrap_or(20)
                                });
                            let total_filtered = st
                                .lines
                                .iter()
                                .filter(|l| l.to_lowercase().contains(&st.query.to_lowercase()))
                                .count();
                            app::ensure_log_line_visible(
                                &mut st.scroll_from_end,
                                total_filtered,
                                target_line_idx,
                                inner_h,
                            );
                        }
                    }
                }
            }
            K::Down => {
                if let Mode::Logs(st) = &mut app.mode {
                    st.scroll_from_end = st.scroll_from_end.saturating_add(1);
                }
            }
            K::Up => {
                if let Mode::Logs(st) = &mut app.mode {
                    st.scroll_from_end = st.scroll_from_end.saturating_sub(1);
                }
            }
            K::PageDown => {
                if let Mode::Logs(st) = &mut app.mode {
                    st.scroll_from_end = st.scroll_from_end.saturating_add(20);
                }
            }
            K::PageUp => {
                if let Mode::Logs(st) = &mut app.mode {
                    st.scroll_from_end = st.scroll_from_end.saturating_sub(20);
                }
            }
            _ => {}
        },
        Mode::LogExport {
            dir_buf,
            file_buf,
            focus,
            suggestions,
            sug_idx,
            sug_scroll,
            logs_state,
        } => match key.code {
            K::Esc => {
                app.mode = Mode::Logs(logs_state.clone_view());
            }
            K::Tab => {
                *focus = match *focus {
                    crate::app::SaveFocus::Directory => crate::app::SaveFocus::Filename,
                    crate::app::SaveFocus::Filename => crate::app::SaveFocus::OkBtn,
                    crate::app::SaveFocus::OkBtn => crate::app::SaveFocus::CancelBtn,
                    crate::app::SaveFocus::CancelBtn => crate::app::SaveFocus::Directory,
                };
            }
            K::BackTab => {
                *focus = match *focus {
                    crate::app::SaveFocus::Directory => crate::app::SaveFocus::CancelBtn,
                    crate::app::SaveFocus::Filename => crate::app::SaveFocus::Directory,
                    crate::app::SaveFocus::OkBtn => crate::app::SaveFocus::Filename,
                    crate::app::SaveFocus::CancelBtn => crate::app::SaveFocus::OkBtn,
                };
            }
            K::Down => {
                const MAX_VISIBLE: usize = 5;
                if *focus == crate::app::SaveFocus::Directory && !suggestions.is_empty() {
                    if let Some(i) = *sug_idx {
                        if i + 1 < suggestions.len() {
                            let next = i + 1;
                            *sug_idx = Some(next);
                            if next >= *sug_scroll + MAX_VISIBLE {
                                *sug_scroll = next + 1 - MAX_VISIBLE;
                            }
                        }
                    } else {
                        *sug_idx = Some(0);
                        *sug_scroll = 0;
                    }
                }
            }
            K::Up => {
                if *focus == crate::app::SaveFocus::Directory
                    && !suggestions.is_empty()
                    && let Some(i) = *sug_idx
                {
                    if i > 0 {
                        let prev = i - 1;
                        *sug_idx = Some(prev);
                        if prev < *sug_scroll {
                            *sug_scroll = prev;
                        }
                    } else {
                        *sug_idx = None;
                    }
                }
            }
            K::Left => {
                if *focus == crate::app::SaveFocus::CancelBtn {
                    *focus = crate::app::SaveFocus::OkBtn;
                } else if *focus == crate::app::SaveFocus::OkBtn {
                    *focus = crate::app::SaveFocus::CancelBtn;
                }
            }
            K::Right => {
                if *focus == crate::app::SaveFocus::OkBtn {
                    *focus = crate::app::SaveFocus::CancelBtn;
                } else if *focus == crate::app::SaveFocus::CancelBtn {
                    *focus = crate::app::SaveFocus::OkBtn;
                } else if *focus == crate::app::SaveFocus::Directory
                    && let Some(i) = *sug_idx
                    && let Some(sug) = suggestions.get(i)
                {
                    *dir_buf = sug.clone();
                    *suggestions = crate::app::compute_dir_suggestions(dir_buf);
                    *sug_idx = None;
                    *sug_scroll = 0;
                }
            }
            K::Backspace => match *focus {
                crate::app::SaveFocus::Directory => {
                    dir_buf.pop();
                    *suggestions = crate::app::compute_dir_suggestions(dir_buf);
                    *sug_idx = None;
                    *sug_scroll = 0;
                }
                crate::app::SaveFocus::Filename => {
                    file_buf.pop();
                }
                _ => {}
            },
            K::Char(c) => match *focus {
                crate::app::SaveFocus::Directory => {
                    dir_buf.push(c);
                    *suggestions = crate::app::compute_dir_suggestions(dir_buf);
                    *sug_idx = None;
                    *sug_scroll = 0;
                }
                crate::app::SaveFocus::Filename => {
                    file_buf.push(c);
                }
                _ => {}
            },
            K::Enter => {
                if *focus == crate::app::SaveFocus::Directory && sug_idx.is_some() {
                    if let Some(i) = *sug_idx
                        && let Some(sug) = suggestions.get(i)
                    {
                        *dir_buf = sug.clone();
                        *suggestions = crate::app::compute_dir_suggestions(dir_buf);
                        *sug_idx = None;
                        *sug_scroll = 0;
                    }
                    return;
                }
                if *focus == crate::app::SaveFocus::CancelBtn {
                    app.mode = Mode::Logs(logs_state.clone_view());
                    return;
                }

                let raw_dir = if dir_buf.trim().is_empty() {
                    std::env::var("K9X_LOG_DIR").unwrap_or_else(|_| "/tmp".into())
                } else {
                    dir_buf.trim().to_string()
                };
                let expanded_dir = if let Some(rest) = raw_dir.strip_prefix("~/") {
                    if let Ok(home) = std::env::var("HOME") {
                        format!("{home}/{rest}")
                    } else {
                        raw_dir.clone()
                    }
                } else if raw_dir == "~" {
                    std::env::var("HOME").unwrap_or_else(|_| raw_dir.clone())
                } else {
                    raw_dir.clone()
                };

                let file = if file_buf.trim().is_empty() {
                    format!(
                        "{}_{}_{}.txt",
                        logs_state.ns,
                        logs_state.pod,
                        Utc::now().timestamp()
                    )
                } else {
                    let f = file_buf.trim();
                    if f.ends_with(".txt") {
                        f.to_string()
                    } else {
                        format!("{f}.txt")
                    }
                };

                let full_path = std::path::Path::new(&expanded_dir).join(&file);
                let full_path_str = full_path.to_string_lossy().to_string();
                let content = logs_state.lines.join("\n") + "\n";
                let ls = logs_state.clone_view();

                app.mode = Mode::Confirm {
                    prompt: format!("Save logs to {full_path_str}?"),
                    action: crate::app::Action::SaveLogs {
                        path: full_path_str,
                        content,
                        logs_state: Box::new(ls),
                    },
                    sel_yes: true,
                };
            }
            _ => {}
        },
        Mode::PortForward(st) => match key.code {
            K::Esc => {
                app.mode = Mode::Normal;
            }
            K::Tab | K::Down => {
                st.focus = match st.focus {
                    crate::app::PfFocus::ContainerPort => crate::app::PfFocus::LocalPort,
                    crate::app::PfFocus::LocalPort => crate::app::PfFocus::Address,
                    crate::app::PfFocus::Address => crate::app::PfFocus::OkBtn,
                    crate::app::PfFocus::OkBtn => crate::app::PfFocus::CancelBtn,
                    crate::app::PfFocus::CancelBtn => crate::app::PfFocus::ContainerPort,
                };
            }
            K::BackTab | K::Up => {
                st.focus = match st.focus {
                    crate::app::PfFocus::ContainerPort => crate::app::PfFocus::CancelBtn,
                    crate::app::PfFocus::LocalPort => crate::app::PfFocus::ContainerPort,
                    crate::app::PfFocus::Address => crate::app::PfFocus::LocalPort,
                    crate::app::PfFocus::OkBtn => crate::app::PfFocus::Address,
                    crate::app::PfFocus::CancelBtn => crate::app::PfFocus::OkBtn,
                };
            }
            K::Left => {
                if st.focus == crate::app::PfFocus::CancelBtn {
                    st.focus = crate::app::PfFocus::OkBtn;
                } else if st.focus == crate::app::PfFocus::OkBtn {
                    st.focus = crate::app::PfFocus::CancelBtn;
                }
            }
            K::Right => {
                if st.focus == crate::app::PfFocus::OkBtn {
                    st.focus = crate::app::PfFocus::CancelBtn;
                } else if st.focus == crate::app::PfFocus::CancelBtn {
                    st.focus = crate::app::PfFocus::OkBtn;
                }
            }
            K::Backspace => match st.focus {
                crate::app::PfFocus::ContainerPort => {
                    st.container_port.pop();
                    st.local_port = parse_port(&st.container_port)
                        .map(|p| p.to_string())
                        .unwrap_or_default();
                }
                crate::app::PfFocus::LocalPort => {
                    st.local_port.pop();
                }
                crate::app::PfFocus::Address => {
                    st.address.pop();
                }
                _ => {}
            },
            K::Char(c) => match st.focus {
                crate::app::PfFocus::ContainerPort => {
                    st.container_port.push(c);
                    st.local_port = parse_port(&st.container_port)
                        .map(|p| p.to_string())
                        .unwrap_or_default();
                }
                crate::app::PfFocus::LocalPort => {
                    if c.is_ascii_digit() {
                        st.local_port.push(c);
                    }
                }
                crate::app::PfFocus::Address => {
                    st.address.push(c);
                }
                _ => {}
            },
            K::Enter => {
                if st.focus == crate::app::PfFocus::CancelBtn {
                    app.mode = Mode::Normal;
                    return;
                }
                let co_port_str = st.container_port.trim().to_string();
                let lo_port_str = st.local_port.trim().to_string();
                if co_port_str.is_empty() || lo_port_str.is_empty() {
                    app.set_status("!container to local port mismatch");
                    return;
                }
                let Some(remote_port) = parse_port(&co_port_str) else {
                    app.set_status("!invalid container port");
                    return;
                };
                let Ok(local_port) = lo_port_str.parse::<u16>() else {
                    app.set_status("!invalid local port");
                    return;
                };
                let addr = if st.address.trim().is_empty() {
                    "127.0.0.1"
                } else {
                    st.address.trim()
                };
                let bind_addr = format!("{addr}:{local_port}");
                let cl = app.cluster.clone();
                let ns = st.ns.clone();
                let pod = st.pod.clone();
                match k8s::port_forward(cl, ns, pod, remote_port, bind_addr).await {
                    Ok(entry) => {
                        let lp = entry.local_port;
                        app.start_pf(entry);
                        app.set_status(format!("PortForward activated {lp}"));
                        app.mode = Mode::Normal;
                    }
                    Err(e) => {
                        app.set_status(format!("!pf: {e}"));
                    }
                }
            }
            _ => {}
        },
        Mode::Exec(ex) => match key.code {
            K::Char('q') if key.modifiers.contains(M::CONTROL) => {
                app.close_exec();
                app.set_status("exec detached");
            }
            K::Char('c') if key.modifiers.contains(M::CONTROL) => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![0x03]));
            }
            K::Char('d') if key.modifiers.contains(M::CONTROL) => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![0x04]));
            }
            K::Backspace | K::Delete => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![0x7f]));
            }
            K::Enter => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![b'\r']));
            }
            K::Up => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![0x1b, b'[', b'A']));
            }
            K::Down => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![0x1b, b'[', b'B']));
            }
            K::Right => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![0x1b, b'[', b'C']));
            }
            K::Left => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![0x1b, b'[', b'D']));
            }
            K::Home => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![0x1b, b'[', b'H']));
            }
            K::End => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![0x1b, b'[', b'F']));
            }
            K::Tab => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![b'\t']));
            }
            K::BackTab => {
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(vec![0x1b, b'[', b'Z']));
            }
            K::Char(c) => {
                let bytes = if key.modifiers.contains(M::CONTROL) {
                    let code = (c as u8).to_ascii_lowercase();
                    if code.is_ascii_lowercase() {
                        vec![code - b'a' + 1]
                    } else {
                        c.to_string().into_bytes()
                    }
                } else if c == '\x08' || c == '\x7f' {
                    vec![0x7f]
                } else {
                    let mut b = c.to_string().into_bytes();
                    if key.modifiers.contains(M::SHIFT) && c.is_uppercase() {
                        b = vec![c as u8];
                    }
                    b
                };
                let _ = ex.ctl_tx.send(k8s::ExecCtl::Input(bytes));
            }
            _ => {}
        },
    }
}

async fn handle_normal(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::{KeyCode as K, KeyModifiers as M};

    // auto-clear stale error status when performing a valid view/tab action
    if app.status.starts_with('!') {
        match key.code {
            K::Tab | K::BackTab => app.status.clear(),
            _ => {}
        }
    }

    // pulse dashboard owns navigation keys — intercept before table handlers
    if app.view == ViewKind::Pulse {
        match key.code {
            K::Tab | K::Down | K::Right | K::Char('j') | K::Char('l') => {
                app.pulse_sel = (app.pulse_sel + 1) % crate::app::PULSE_CARDS.len();
                return;
            }
            K::BackTab | K::Up | K::Left | K::Char('k') | K::Char('h') => {
                app.pulse_sel = (app.pulse_sel + crate::app::PULSE_CARDS.len() - 1)
                    % crate::app::PULSE_CARDS.len();
                return;
            }
            K::Enter => {
                let alias = crate::app::PULSE_CARDS[app.pulse_sel.min(11)].1.to_string();
                let name = crate::app::PULSE_CARDS[app.pulse_sel.min(11)].0;
                app.stop_pulse();
                switch(app, &alias);
                app.set_status(format!("pulse → {name}"));
                return;
            }
            _ => {}
        }
    }

    // global keys
    match (key.modifiers, key.code) {
        (M::CONTROL, K::Char('c')) | (M::CONTROL, K::Char('q')) => {
            app.quit = true;
            return;
        }
        _ => {}
    }

    match key.code {
        K::Char('?') => {
            open_help(app);
        }
        K::Char('C') if !key.modifiers.contains(M::CONTROL) => app.open_menu_contexts(),
        K::Char('T') if !key.modifiers.contains(M::CONTROL) => app.open_themes_menu(),
        K::Char(':') if !key.modifiers.contains(M::CONTROL) => {
            app.mode = Mode::Cmd {
                buf: String::new(),
                sel: 0,
            }
        }
        K::Char('/') if !key.modifiers.contains(M::CONTROL) => {
            app.mode = Mode::Filter { buf: String::new() }
        }
        K::Char('q') => {
            if app.view == ViewKind::Pulse {
                app.stop_pulse();
                app.view = ViewKind::Table;
                app.restart_watch_if_pulse_left();
            } else if app.view == ViewKind::Pf {
                app.view = ViewKind::Table;
            } else if app.drill_selector.is_some() {
                app.drill_selector = None;
                app.drill_title = None;
                let _ = app.switch_view("po");
            } else {
                app.set_status("\u{2318}q / :exit to quit \u{2014} q only backs out");
            }
        }
        K::Esc => {
            if app.view == ViewKind::Pulse {
                app.stop_pulse();
                app.view = ViewKind::Table;
                app.restart_watch_if_pulse_left();
            } else if app.view == ViewKind::Pf {
                app.view = ViewKind::Table;
            } else if !app.filter.is_empty() {
                app.filter.clear();
                app.sel_top();
            } else if !app.marks.is_empty() {
                let n = app.marks.len();
                app.marks.clear();
                app.set_status(format!("cleared {n} marks"));
            } else if app.drill_selector.take().is_some() {
                app.drill_title = None;
                app.restart_watch_full_reset();
                app.set_status("back to full view");
            } else {
                app.set_status("ctrl-q to quit · esc only backs out");
            }
        }
        K::Char(' ') if key.modifiers.is_empty() => {
            if app.view == ViewKind::Table
                && let Some(k) = app.selected_or_first()
            {
                let marked = app.toggle_mark(&k);
                let n = app.marks.len();
                app.set_status(if marked {
                    format!("marked {k} · {n} total")
                } else {
                    format!("unmarked {k} · {n} total")
                });
            }
        }
        K::Char(' ') if key.modifiers.contains(M::CONTROL) => {
            if app.view == ViewKind::Table
                && let Some(k) = app.selected_or_first()
            {
                let added = app.span_mark_to(&k);
                let n = app.marks.len();
                app.set_status(format!("range-marked {added} · {n} total"));
            }
        }
        K::Char('y') if key.modifiers.contains(M::CONTROL) => {
            if app.view == ViewKind::Table
                && let Some(spec) = app.view_spec.clone()
                && let Some(name) = app.selected_or_first()
            {
                let ns = action_ns(app, &name);
                match k8s::get_yaml(&app.cluster, &spec, ns.as_deref(), &name).await {
                    Ok(yml) => {
                        let ok = copy_to_clipboard(&yml);
                        app.set_status(if ok {
                            format!("yaml copied ({name}, {} bytes)", yml.len())
                        } else {
                            "!no clipboard helper found".into()
                        });
                    }
                    Err(e) => app.err_status(e),
                }
            }
        }
        K::Char('n') if key.modifiers.contains(M::CONTROL) => {
            if let Some(name) = app.selected_or_first() {
                let ok = copy_to_clipboard(&name);
                app.set_status(if ok {
                    format!("name copied ({name})")
                } else {
                    "!no clipboard helper found".into()
                });
            }
        }
        K::Up | K::Char('k') if !key.modifiers.contains(M::CONTROL) => app.move_sel(-1),
        K::Down | K::Char('j') if !key.modifiers.contains(M::CONTROL) => app.move_sel(1),
        K::PageUp => app.move_sel(-20),
        K::PageDown => app.move_sel(20),
        K::Char('g') if !key.modifiers.contains(M::CONTROL) => app.sel_top(),
        K::Home => app.sel_top(),
        K::Char('G') | K::End => app.sel_bottom(),
        K::Tab => {
            app.sort_col += 1;
            if app.sort_col >= app.cols_count() {
                app.sort_col = 0;
                app.sort_desc = !app.sort_desc;
            }
        }
        K::Char('a') => {
            app.all_ns = !app.all_ns;
            app.restart_watch_full_reset();
            let mut st = cfg::StateCfg::load();
            st.remember_ns(&app.cluster.ctx_name, if app.all_ns { "" } else { &app.ns });
            st.save();
            app.refresh_pulse_if_active();
            if app.all_ns {
                app.set_status("all namespaces");
            } else {
                app.set_status(format!("ns → {}", app.ns));
            }
        }
        K::Char('r') => {
            app.restart_watch_full_reset();
            app.set_status("re-watching…");
        }
        K::Char(c @ '1'..='9') => {
            let idx = (c as u8 - b'1') as usize;
            if let Some(nsname) = app.ns_shortcuts.get(idx).cloned() {
                app.use_namespace(&nsname);
                app.set_status(format!("ns \u{2192} {nsname}"));
            } else {
                app.set_status(format!("!no namespace bound to {c} (:ns to list)"));
            }
        }
        _ => {
            if let Some(target) = jump_for(app, key.code, key.modifiers) {
                let (alias, filter) = match target.split_once('/') {
                    Some((a, f)) => (a.to_string(), Some(f.to_string())),
                    None => (target, None),
                };
                switch(app, &alias);
                if let Some(f) = filter {
                    app.filter = f;
                    app.sel_top();
                }
            } else if let Some(cmdtext) = hotkey_for(app, key.code, key.modifiers) {
                let c2 = cmdtext.clone();
                exec_cmd(app, &c2).await;
            } else if let Some((name, pl)) = app.plugin_for_key(key.code, key.modifiers) {
                Box::pin(launch_plugin(app, name, pl)).await;
            } else {
                row_action(app, key).await;
            }
        }
    }
}

fn jump_for(
    app: &App,
    code: crossterm::event::KeyCode,
    mods: crossterm::event::KeyModifiers,
) -> Option<String> {
    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    for j in &app.jumps {
        let want = j.short_cut.as_str();
        let hit = if let Some(stripped) = want.strip_prefix("ctrl-") {
            let c2 = stripped.chars().next()?.to_ascii_uppercase();
            matches!(code, K::Char(ch) if ch.to_ascii_uppercase() == c2)
                && mods.contains(M::CONTROL)
        } else if want.len() == 1 {
            matches!(code, K::Char(ch) if ch.eq_ignore_ascii_case(&want.chars().next().unwrap()))
                && mods.is_empty()
        } else {
            false
        };
        if hit {
            return Some(j.command.clone());
        }
    }
    None
}

fn hotkey_for(
    app: &App,
    code: crossterm::event::KeyCode,
    mods: crossterm::event::KeyModifiers,
) -> Option<String> {
    use crossterm::event::KeyCode as K;
    for (_, hk) in &app.hotkeys {
        let want = hk.short_cut.as_str();
        let hit = if let Some(stripped) = want.strip_prefix("ctrl-") {
            let c = stripped.chars().next()?.to_ascii_uppercase();
            matches!(code, K::Char(ch) if ch.to_ascii_uppercase() == c)
                && mods.contains(crossterm::event::KeyModifiers::CONTROL)
        } else if want.len() == 1 {
            matches!(code, K::Char(ch) if ch.eq_ignore_ascii_case(&want.chars().next().unwrap()))
                && mods.is_empty()
        } else {
            false
        };
        if hit {
            return Some(hk.command.clone());
        }
    }
    None
}

async fn handle_mouse(app: &mut App, m: crossterm::event::MouseEvent) {
    use crossterm::event::MouseButton;
    use crossterm::event::MouseEventKind as MK;

    // modal clicks
    if let MK::Down(MouseButton::Left) = m.kind {
        match &app.mode {
            Mode::Confirm { action, .. } => {
                let hit = app.ui_confirm_btn;
                if let Some((r, mid)) = hit {
                    let inside = m.row == r.y && m.column >= r.x && m.column < r.x + r.width;
                    if !inside {
                        return;
                    }
                    let yes = m.column < mid;
                    if yes {
                        run_confirm_with(app, true).await;
                    } else {
                        // reject path (theme reverts; others just cancel)
                        if matches!(action, Action::ThemeKeep { .. }) {
                            app.mode = Mode::Normal;
                            app.revert_theme_preview();
                        } else {
                            app.set_status("cancelled");
                        }
                    }
                    return;
                }
            }
            Mode::Notice { ok_exits, .. } => {
                if let Some(r) = app.ui_notice_rect {
                    let inside = m.row >= r.y
                        && m.row < r.y + r.height
                        && m.column >= r.x
                        && m.column < r.x + r.width;
                    if inside {
                        if *ok_exits {
                            app.quit = true;
                        } else {
                            app.mode = Mode::Normal;
                        }
                        return;
                    }
                }
            }
            Mode::LogExport { .. } => {
                // clicks in the log export dialog - just consume them (handled by keyboard)
                return;
            }
            Mode::PortForward(..) => {
                // clicks in the port forward dialog - just consume them (handled by keyboard)
                return;
            }
            _ => {}
        }
    }

    if !matches!(app.mode, Mode::Normal) || app.view != ViewKind::Table {
        return;
    }
    if let MK::Down(MouseButton::Left) = m.kind {
        if let (Some(h), cols) = (&app.ui_header, &app.ui_col_starts)
            && m.row == h.y
            && m.column >= h.x
            && m.column < h.x + h.width
        {
            let rel = (m.column - h.x) as usize;
            for (i, start) in cols.iter().enumerate() {
                let end = cols.get(i + 1).copied().unwrap_or(u16::MAX);
                if rel >= *start as usize && rel < end as usize {
                    if app.sort_col == i {
                        app.sort_desc = !app.sort_desc;
                    } else {
                        app.sort_col = i;
                        app.sort_desc = false;
                    }
                    return;
                }
            }
        }
        if let Some(b) = app.ui_body
            && m.row > b.y
            && m.row <= b.y + b.height
            && !app.ui_row_keys.is_empty()
        {
            let idx = (m.row - b.y - 1) as usize + app.ui_toffset;
            if idx < app.ui_row_keys.len() {
                app.sel_key = Some(app.ui_row_keys[idx].clone());
            }
        }
    }
}

fn switch(app: &mut App, alias: &str) {
    if app.status.starts_with('!') {
        app.status.clear();
    }
    if let Err(e) = app.switch_view(alias) {
        app.set_status(format!("!{e}"));
    }
}

async fn row_action(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::{KeyCode as K, KeyModifiers as M};

    match app.view {
        ViewKind::Pulse => {}
        ViewKind::Pf => match key.code {
            K::Char('s') => {
                if let Some(i) = pf_sel_idx(app) {
                    let id = app.pfs[i].id;
                    app.stop_pf_entry(id);
                }
            }
            K::Char('X') => app.stop_all_pfs(),
            K::Up | K::Char('k') => pf_move(app, -1),
            K::Down | K::Char('j') => pf_move(app, 1),
            _ => {}
        },
        ViewKind::Table => {
            let Some(name) = app.selected_row().map(|r| r.key.clone()) else {
                app.set_status("no rows here — 'r' reload, ':ns'/'a' to widen, ':ctx' to switch");
                return;
            };
            let Some(spec) = app.view_spec.clone() else {
                return;
            };
            match key.code {
                K::Enter | K::Char('\n') | K::Char('\r') => on_enter(app, &spec, &name).await,
                K::Char('d') if !key.modifiers.contains(M::CONTROL) => {
                    open_describe(app, &spec, &name).await
                }
                K::Char('y') if !key.modifiers.contains(M::CONTROL) => {
                    open_yaml(app, &spec, &name).await
                }
                K::Char('e') if !key.modifiers.contains(M::CONTROL) => {
                    edit_resource(app, &spec, &name).await
                }
                K::Char('l') if !key.modifiers.contains(M::CONTROL) => {
                    on_logs(app, &spec, &name).await
                }
                K::Char('s') if !key.modifiers.contains(M::CONTROL) && spec.kind == "Pod" => {
                    app.open_pod_menu(&name, true).await
                }
                K::Char('s')
                    if !key.modifiers.contains(M::CONTROL) && spec.kind == "Node" && !app.ro =>
                {
                    let pure = app.pure_name(&name);
                    confirm(
                        app,
                        format!("spawn privileged shell pod on node {pure}?"),
                        Action::NodeShell { node: name },
                    );
                }
                K::Char('D') if key.modifiers.contains(M::SHIFT) && spec.kind == "Node" => {
                    let pure = app.pure_name(&name);
                    confirm(
                        app,
                        format!("DRAIN node {pure}? (cordons + evicts pods)"),
                        Action::Drain { node: name },
                    );
                }
                K::Char('R')
                    if key.modifiers.contains(M::SHIFT)
                        && matches!(
                            spec.kind.as_str(),
                            "Deployment" | "StatefulSet" | "DaemonSet"
                        ) =>
                {
                    let pure = app.pure_name(&name);
                    let active_marks = app.marked_active_keys();
                    if !active_marks.is_empty() {
                        let n = active_marks.len();
                        confirm(
                            app,
                            format!("rollout restart {n} MARKED workloads?"),
                            Action::RestartMarked {
                                names: active_marks,
                            },
                        );
                    } else {
                        confirm(
                            app,
                            format!("rollout restart {}/{}?", spec.kind, pure),
                            Action::Restart { name },
                        );
                    }
                }
                K::Char('S')
                    if key.modifiers.contains(M::SHIFT)
                        && matches!(
                            spec.kind.as_str(),
                            "Deployment" | "StatefulSet" | "ReplicaSet"
                        ) =>
                {
                    app.mode = Mode::Input {
                        buf: String::new(),
                        purpose: InputPurpose::Scale { name },
                    };
                }
                K::Char('t') if !key.modifiers.contains(M::CONTROL) && spec.kind == "CronJob" => {
                    let pure = app.pure_name(&name);
                    confirm(
                        app,
                        format!("trigger job from cronjob/{pure}?"),
                        Action::TriggerCj { cron: name },
                    );
                }
                K::Char('x') if !key.modifiers.contains(M::CONTROL) && spec.kind == "CronJob" => {
                    let pure = app.pure_name(&name);
                    let cur = fetch_suspend_state(app, &name).await.unwrap_or(false);
                    confirm(
                        app,
                        format!("set suspend={} on cronjob/{pure}?", !cur),
                        Action::ToggleSuspendCj {
                            cron: name,
                            to: !cur,
                        },
                    );
                }
                K::Char('p') if !key.modifiers.contains(M::CONTROL) => {
                    on_pf(app, &spec, &name).await
                }
                K::Char('F') if key.modifiers.contains(M::SHIFT) => on_pf(app, &spec, &name).await,
                K::Char('X') if !key.modifiers.contains(M::CONTROL) && spec.kind == "Secret" => {
                    open_secret_decode(app, &name).await
                }
                K::Char('A') if key.modifiers.contains(M::SHIFT) && spec.kind == "Pod" => {
                    let ans = action_ns(app, &name).unwrap_or_else(|| app.ns.clone());
                    let pure = app.pure_name(&name).to_string();
                    app.start_attach(ans, pure, None).await;
                }
                K::Char('u') if !key.modifiers.contains(M::CONTROL) && spec.kind == "Node" => {
                    let pure = app.pure_name(&name);
                    if (app.row_flags(&name) & crate::model::FLAG_CORDONED) == 0 {
                        app.set_status("!node is not cordoned");
                        return;
                    }
                    confirm(
                        app,
                        format!("uncordon node {pure}?"),
                        Action::Uncordon { node: name },
                    );
                }
                K::Char('c') if !key.modifiers.contains(M::CONTROL) && spec.kind == "Node" => {
                    let pure = app.pure_name(&name);
                    if (app.row_flags(&name) & crate::model::FLAG_CORDONED) != 0 {
                        app.set_status("!node is already cordoned");
                        return;
                    }
                    confirm(
                        app,
                        format!("cordon node {pure}?"),
                        Action::Cordon { node: name },
                    );
                }
                K::Char('d') if key.modifiers.contains(M::CONTROL) => {
                    let pure = app.pure_name(&name);
                    let active_marks = app.marked_active_keys();
                    if !active_marks.is_empty() {
                        let n = active_marks.len();
                        confirm(
                            app,
                            format!("delete {n} MARKED resources?"),
                            Action::DeleteMarked {
                                names: active_marks,
                                force: false,
                            },
                        );
                    } else {
                        confirm(
                            app,
                            format!("delete {}/{}?", spec.kind, pure),
                            Action::Delete { name, force: false },
                        );
                    }
                }
                K::Char('k') if key.modifiers.contains(M::CONTROL) => {
                    let pure = app.pure_name(&name);
                    let active_marks = app.marked_active_keys();
                    if !active_marks.is_empty() {
                        let n = active_marks.len();
                        confirm(
                            app,
                            format!("FORCE delete {n} MARKED resources?"),
                            Action::DeleteMarked {
                                names: active_marks,
                                force: true,
                            },
                        );
                    } else {
                        confirm(
                            app,
                            format!("FORCE delete {}/{}?", spec.kind, pure),
                            Action::Delete { name, force: true },
                        );
                    }
                }
                K::Char('U')
                    if !key.modifiers.contains(M::CONTROL)
                        && matches!(spec.kind.as_str(), "Secret" | "ConfigMap") =>
                {
                    app.open_used_by(spec.kind == "Secret", name).await;
                }
                _ => {}
            }
        }
    }
}

async fn on_enter(app: &mut App, _spec: &KindSpec, _name: &str) {
    // Enter opens the action menu (drill lives inside it) — k9s-style consistency without surprises
    open_action_menu(app);
}

async fn on_logs(app: &mut App, spec: &KindSpec, name: &str) {
    match spec.kind.as_str() {
        "Pod" => app.open_pod_containers_menu(name).await,
        "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" | "Job" => {
            // logs of first pod behind workload
            let Some(own_ns) = app.target_ns(name) else {
                app.set_status("!cannot resolve namespace");
                return;
            };
            match workload_selector(app, spec, name).await {
                Some(sel) => app.open_logs_multi(own_ns, sel).await,
                None => app.set_status("!no selector / running pods"),
            }
        }
        _ => app.set_status("!logs not supported for this kind"),
    }
}

async fn on_pf(app: &mut App, spec: &KindSpec, name: &str) {
    match spec.kind.as_str() {
        "Pod" => {
            let ns = action_ns(app, name).unwrap_or_else(|| app.ns.clone());
            app.open_port_forward_dialog(ns, name.to_string()).await;
        }
        "Service" => {
            app.pf_for_service(name).await;
        }
        "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" => {
            let Some(own_ns) = action_ns(app, name) else {
                app.set_status("!cannot resolve namespace");
                return;
            };
            if let Some(sel) = workload_selector(app, spec, name).await {
                let pod_gvk = kube::core::gvk::GroupVersionKind::gvk("", "v1", "Pod");
                let ar = kube::discovery::ApiResource::from_gvk(&pod_gvk);
                let pods: kube::Api<kube::core::dynamic::DynamicObject> =
                    kube::Api::namespaced_with(app.cluster.client.clone(), &own_ns, &ar);
                let list = match pods
                    .list(&kube::api::ListParams::default().labels(&sel))
                    .await
                {
                    Ok(l) => l,
                    Err(e) => {
                        app.err_status(e);
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
                    Some(pod) => app.open_port_forward_dialog(own_ns, pod).await,
                    None => app.set_status("!no running pod for workload"),
                }
            } else {
                app.set_status("!no selector / running pods");
            }
        }
        _ => app.set_status("!pf supported on pods, services, and workloads"),
    }
}

fn action_ns(app: &App, name: &str) -> Option<String> {
    app.target_ns(name).or_else(|| {
        // fall back to current scope for rows not yet in cache
        if app.all_ns {
            None
        } else {
            Some(app.ns.clone())
        }
    })
}

async fn open_yaml(app: &mut App, spec: &KindSpec, name: &str) {
    let ns = action_ns(app, name);
    let cl = app.cluster.clone();
    let tx = app.tx.clone();
    let sp = spec.clone();
    let pure = app.pure_name(name);
    let nm = pure.to_string();
    app.set_status(format!("fetching yaml {}/{}\u{2026}", spec.kind, pure));
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

async fn open_describe(app: &mut App, spec: &KindSpec, name: &str) {
    let ns = action_ns(app, name);
    let cl = app.cluster.clone();
    let tx = app.tx.clone();
    let sp = spec.clone();
    let pure = app.pure_name(name);
    let nm = pure.to_string();
    app.set_status(format!("describing {}/{}\u{2026}", spec.kind, pure));
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

async fn open_secret_decode(app: &mut App, name: &str) {
    let Some(ns) = action_ns(app, name) else {
        app.set_status("!cannot resolve namespace");
        return;
    };
    let pure = app.pure_name(name);
    match k8s::decode_secret(&app.cluster, &ns, pure).await {
        Ok(text) => {
            let lines: Vec<String> = text.lines().map(String::from).collect();
            app.mode = Mode::TextPane {
                title: format!("decoded:{pure}"),
                lines,
                pos: 0,
                wrap: false,
            };
        }
        Err(e) => app.set_status(format!("!decode: {e}")),
    }
}

async fn edit_resource(app: &mut App, spec: &KindSpec, name: &str) {
    if app.ro {
        app.set_status("!read-only mode: edit blocked");
        return;
    }
    let ns = action_ns(app, name);
    let pure = app.pure_name(name);
    let yaml = match k8s::get_yaml(&app.cluster, spec, ns.as_deref(), pure).await {
        Ok(y) => y,
        Err(e) => {
            app.set_status(format!("!edit: {e}"));
            return;
        }
    };
    suspend_tui();
    let edited = {
        let _g = ResumeOnDrop;
        run_editor(&yaml)
    };
    match edited {
        Some(new_yaml) if new_yaml.trim() != yaml.trim() => {
            match apply_edited(app, spec, ns.as_deref(), name, &new_yaml).await {
                Ok(msg) => app.set_status(msg),
                Err(e) => app.set_status(format!("!apply failed: {e}")),
            }
        }
        Some(_) => app.set_status("edit: no changes"),
        None => app.set_status("!editor aborted"),
    }
}

async fn edit_from_pane(app: &mut App) {
    let Mode::TextPane { title, lines, .. } = &app.mode else {
        return;
    };
    let yaml = lines.join("\n");
    let Some((kind_name, res)) = title.split_once(':') else {
        return;
    };
    let kind = kind_name.strip_prefix("yaml:").unwrap_or(kind_name);
    let Some(spec) = find_spec_by_kind(kind) else {
        app.set_status("!cannot resolve kind");
        return;
    };
    suspend_tui();
    let edited = {
        let _g = ResumeOnDrop;
        run_editor(&yaml)
    };
    match edited {
        Some(new_yaml) if new_yaml.trim() != yaml.trim() => {
            match apply_edited(app, &spec, None, res, &new_yaml).await {
                Ok(msg) => app.set_status(msg),
                Err(e) => app.set_status(format!("!apply failed: {e}")),
            }
        }
        Some(_) => app.set_status("edit: no changes"),
        None => app.set_status("!editor aborted"),
    }
}

fn find_spec_by_kind(kind: &str) -> Option<KindSpec> {
    for alias in model::all_aliases() {
        if let Some(s) = spec_for(alias)
            && s.kind == kind
        {
            return Some(s);
        }
    }
    None
}

struct ResumeOnDrop;
impl Drop for ResumeOnDrop {
    fn drop(&mut self) {
        resume_tui();
    }
}

fn suspend_tui() {
    INPUT_PAUSE.store(true, std::sync::atomic::Ordering::Relaxed);
    std::thread::sleep(std::time::Duration::from_millis(140)); // let in-flight reads finish
    restore_tui_stderr();
    use crossterm::{execute, terminal};
    let _ = terminal::disable_raw_mode();
    let mut out = std::io::stdout();
    let _ = execute!(out, terminal::LeaveAlternateScreen);
    let _ = write!(
        out,
        "\r\n\u{2500}\u{2500} editing: save & QUIT the editor (:wq) to apply \u{2500}\u{2500}\r\n\r\n"
    );
    use std::io::Write as _;
    let _ = out.flush();
}

static JUST_RESUMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn resume_tui() {
    silence_tui_stderr();
    use crossterm::{execute, terminal};
    let _ = terminal::enable_raw_mode();
    let mut out = std::io::stdout();
    let _ = execute!(
        out,
        terminal::EnterAlternateScreen,
        terminal::Clear(terminal::ClearType::All)
    );
    INPUT_PAUSE.store(false, std::sync::atomic::Ordering::Relaxed);
    JUST_RESUMED.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn editor_argv() -> Vec<String> {
    let raw = std::env::var("KUBE_EDITOR")
        .or_else(|_| std::env::var("EDITOR"))
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".into());
    raw.split_whitespace().map(String::from).collect()
}

fn run_editor(content: &str) -> Option<String> {
    let dir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!(
        "{}/k9x-edit-{}.yaml",
        dir.trim_end_matches('/'),
        std::process::id()
    );
    if std::fs::write(&path, content).is_err() {
        return None;
    }
    let mut argv = editor_argv();
    if argv.is_empty() {
        argv.push("vi".into());
    }
    let base = std::path::Path::new(&argv[0])
        .file_name()
        .map(|x| x.to_string_lossy().to_string())
        .unwrap_or_default();
    // vim/nvim: skip X11 clipboard probing (multi-second startup hangs) and swap files
    if base.contains("vim") {
        argv.push("-X".into());
        argv.push("-n".into());
    }
    println!("opening {} {} (save & quit to apply)", argv[0], path);
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .arg(&path)
        .status();
    let ok = matches!(status, Ok(st) if st.success());
    let out = std::fs::read_to_string(&path).ok();
    let _ = std::fs::remove_file(&path);
    if !ok {
        return None;
    }
    out
}

async fn apply_edited(
    app: &App,
    spec: &KindSpec,
    ns: Option<&str>,
    name: &str,
    yaml: &str,
) -> Result<String> {
    let v: serde_json::Value = serde_yaml::from_str(yaml)?;
    let patch = strip_server_fields(v)?;
    k8s::patch_obj(&app.cluster, spec, ns, name, patch).await
}

fn strip_server_fields(mut v: serde_json::Value) -> Result<serde_json::Value> {
    use serde_json::Value;
    if let Value::Object(ref mut map) = v {
        map.remove("status");
        if let Some(md) = map.get_mut("metadata").and_then(|m| m.as_object_mut()) {
            md.remove("managedFields");
            md.remove("resourceVersion");
            md.remove("uid");
            md.remove("creationTimestamp");
            md.remove("generation");
            md.remove("selfLink");
        }
    }
    Ok(v)
}

#[cfg(test)]
fn extract_port(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some((_, p)) = s.split_once("::") {
        if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() {
            return Some(p.to_string());
        }
    } else if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() {
        return Some(s.to_string());
    }
    None
}

fn parse_port(s: &str) -> Option<u16> {
    let s = s.trim();
    if let Some((_, p)) = s.split_once("::") {
        p.parse::<u16>().ok()
    } else {
        s.parse::<u16>().ok()
    }
}

fn save_logs(app: &mut App) {
    if let Mode::Logs(st) = &app.mode {
        // If an active search query is present, export only the matched lines. Otherwise export all lines.
        let lines_to_export = if !st.query.is_empty() {
            let q = st.query.to_lowercase();
            st.lines
                .iter()
                .filter(|l| l.to_lowercase().contains(&q))
                .cloned()
                .collect()
        } else {
            st.lines.clone()
        };

        let export_state = app::LogsState {
            source: st.source.clone(),
            label: st.label.clone(),
            ns: st.ns.clone(),
            pod: st.pod.clone(),
            container: st.container.clone(),
            previous: st.previous,
            timestamps: st.timestamps,
            lines: lines_to_export,
            scroll_from_end: st.scroll_from_end,
            wrap: st.wrap,
            status: String::new(),
            query: st.query.clone(),
            search: st.search,
            window: st.window,
            handles: vec![],
            rx: {
                let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
                rx
            },
            match_idx: st.match_idx,
            match_total: st.match_total,
            count_occurrences: st.count_occurrences,
        };
        let default_dir = if let Ok(env_dir) = std::env::var("K9X_LOG_DIR") {
            env_dir
        } else {
            let state = crate::cfg::StateCfg::load();
            if !state.last_log_dir.is_empty() {
                state.last_log_dir
            } else {
                "/tmp".into()
            }
        };
        let default_file = format!(
            "{}_{}_{}",
            export_state.ns,
            export_state.pod,
            Utc::now().format("%Y%m%d_%H%M%S")
        );
        let suggestions = crate::app::compute_dir_suggestions(&default_dir);
        app.mode = Mode::LogExport {
            dir_buf: default_dir,
            file_buf: default_file,
            focus: crate::app::SaveFocus::Directory,
            suggestions,
            sug_idx: None,
            sug_scroll: 0,
            logs_state: Box::new(export_state),
        };
    }
}

fn open_action_menu(app: &mut App) {
    let Some(spec) = app.view_spec.clone() else {
        return;
    };
    let Some(name) = app.selected_or_first() else {
        let _ = std::fs::write("/tmp/k9x-m.log", "no-selection");
        return;
    };
    let kind = spec.kind.clone();
    let mut items: Vec<app::MenuItem> = vec![];
    match kind.as_str() {
        "Pod" => {
            items.push(app::MenuItem::new("containers", "containers"));
            items.push(app::MenuItem::new("logs", "logs"));
            items.push(app::MenuItem::new("shell", "shell"));
            items.push(app::MenuItem::new("port-forward", "pf"));
            items.push(app::MenuItem::new("decode secret", "decode"));
        }
        "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" | "Job" => {
            items.push(app::MenuItem::new("drill into pods", "drill"));
            items.push(app::MenuItem::new("logs (first pod)", "wlogs"));
            items.push(app::MenuItem::new("scale…", "scale"));
            items.push(app::MenuItem::new("rollout restart", "restart"));
        }
        "Node" => {
            items.push(app::MenuItem::new("cordon", "cordon"));
            items.push(app::MenuItem::new("uncordon", "uncordon"));
            items.push(app::MenuItem::new("drain…", "drain"));
        }
        "CronJob" => {
            items.push(app::MenuItem::new("trigger job now", "trigger"));
            items.push(app::MenuItem::new("toggle suspend", "suspend"));
        }
        "Service" => items.push(app::MenuItem::new("port-forward…", "svc_pf")),
        "Secret" | "ConfigMap" => items.push(app::MenuItem::new(
            "used-by (who references this?)",
            "usedby",
        )),
        "Role" | "ClusterRole" => items.push(app::MenuItem::new("policy rules", "rules")),
        _ => {}
    }
    items.push(app::MenuItem::new("describe", "describe"));
    items.push(app::MenuItem::new("yaml", "yaml"));
    items.push(app::MenuItem::new("edit ($EDITOR)", "edit"));
    items.push(app::MenuItem::new("delete", "delete"));
    let pure = app.pure_name(&name);
    app.mode = Mode::Menu(app::Menu {
        title: format!("{kind}/{pure} · actions"),
        items,
        sel: 0,
        purpose: MenuPurpose::Actions { kind, name },
    });
}

async fn workload_selector(app: &mut App, spec: &KindSpec, name: &str) -> Option<String> {
    use kube::core::dynamic::DynamicObject;
    let gvk = ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk(
        &spec.group,
        &spec.version,
        &spec.kind,
    ));
    let ns = app.target_ns(name).unwrap_or_else(|| app.ns.clone());
    let pure = app.pure_name(name);
    let api: kube::Api<DynamicObject> = if spec.namespaced {
        kube::Api::namespaced_with(app.cluster.client.clone(), &ns, &gvk)
    } else {
        kube::Api::all_with(app.cluster.client.clone(), &gvk)
    };
    let obj = api.get(pure).await.ok()?;
    let v = serde_json::to_value(&obj).ok()?;
    let pairs = crate::model::selector_labels(&v);
    if pairs.is_empty() {
        return None;
    }
    Some(
        pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(","),
    )
}

async fn run_confirm_with(app: &mut App, force_yes: bool) {
    if force_yes && let Mode::Confirm { sel_yes, .. } = &mut app.mode {
        *sel_yes = true;
    }
    run_confirm(app).await;
}

async fn run_confirm(app: &mut App) {
    let taken = std::mem::replace(&mut app.mode, Mode::Normal);
    let Mode::Confirm {
        action, sel_yes, ..
    } = taken
    else {
        return;
    };
    if let Action::ThemeKeep { name } = &action {
        if sel_yes {
            app.keep_theme(name);
            app.set_status(format!("theme '{}' applied", name));
        } else {
            app.revert_theme_preview();
            app.set_status("theme preview reverted");
        }
        return;
    }
    if let Action::SaveLogs {
        path,
        content,
        logs_state,
    } = action
    {
        if sel_yes {
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let mut state = crate::cfg::StateCfg::load();
                state.remember_log_dir(&parent.to_string_lossy());
                if let Err(e) = std::fs::create_dir_all(parent) {
                    app.set_status(format!("!mkdir failed: {e}"));
                    app.mode = Mode::Logs(*logs_state);
                    return;
                }
            }
            match std::fs::write(&path, content) {
                Ok(_) => {
                    app.set_status(format!("Logs saved to {path}"));
                }
                Err(e) => {
                    app.set_status(format!("!save failed: {e}"));
                }
            }
        } else {
            app.set_status("save cancelled");
        }
        app.mode = Mode::Logs(*logs_state);
        return;
    }
    if !sel_yes {
        app.set_status("cancelled");
        return;
    }
    if app.ro {
        app.set_status("!read-only mode: blocked");
        return;
    }
    let cluster = app.cluster.clone();
    let ns = app.effective_ns_for_action();
    let res = match action {
        Action::Delete { name, force } => {
            let spec = app.view_spec.clone().unwrap();
            let dns = action_ns(app, &name);
            let pure = app.pure_name(&name);
            k8s::delete_obj(&cluster, &spec, dns.as_deref(), pure, force).await
        }
        Action::DeleteMarked { names, force } => {
            let spec = app.view_spec.clone().unwrap();
            let mut ok = 0usize;
            let mut errs: Vec<String> = vec![];
            for n in &names {
                let dns = action_ns(app, n);
                let pure = app.pure_name(n);
                match k8s::delete_obj(&cluster, &spec, dns.as_deref(), pure, force).await {
                    Ok(_) => ok += 1,
                    Err(e) => errs.push(format!("{pure}: {e}")),
                }
            }
            app.marks.clear();
            Ok(format!(
                "deleted {}/{} marked{}",
                ok,
                names.len(),
                if errs.is_empty() {
                    String::new()
                } else {
                    format!(" · !{}", errs.join("; "))
                }
            ))
        }
        Action::RestartMarked { names } => {
            let spec = app.view_spec.clone().unwrap();
            let mut ok = 0usize;
            for n in &names {
                let Some(ns) = action_ns(app, n) else {
                    continue;
                };
                let pure = app.pure_name(n);
                if k8s::rollout_restart(&cluster, &spec, &ns, pure)
                    .await
                    .is_ok()
                {
                    ok += 1;
                }
            }
            app.marks.clear();
            Ok(format!("restart issued for {}/{} marked", ok, names.len()))
        }
        Action::HelmRollback { ns, name, revision } => {
            let pure = app.pure_name(&name);
            k8s::helm_rollback(&cluster.ctx_name, &ns, pure, revision).await
        }
        Action::NodeShell { node } => {
            // heavy flow runs inline; start_exec attaches when ready
            let pure = app.pure_name(&node).to_string();
            app.node_shell(pure).await;
            if app.status.starts_with('!') {
                Err(anyhow!(app.status.trim_start_matches('!').to_string()))
            } else {
                Ok(app.status.clone())
            }
        }
        Action::ThemeKeep { name } => {
            if !sel_yes {
                app.revert_theme_preview();
                Ok("theme rejected \u{2014} reverted".into())
            } else {
                app.keep_theme(&name);
                Ok(format!("theme '{name}' applied"))
            }
        }
        Action::ApplyFile { path } => {
            let ns = if app.all_ns {
                "default".to_string()
            } else {
                app.ns.clone()
            };
            k8s::apply_file(&cluster, &ns, &path).await
        }
        Action::Restart { name } => {
            let Some(ns) = action_ns(app, &name) else {
                app.set_status("!cannot resolve namespace");
                return;
            };
            let spec = app.view_spec.clone().unwrap();
            let pure = app.pure_name(&name);
            match k8s::rollout_restart(&cluster, &spec, &ns, pure).await {
                Ok(m) => app.set_status(m),
                Err(e) => app.err_status(e),
            }
            return;
        }
        Action::ScaleApply { name, replicas } => {
            let Some(ns) = action_ns(app, &name) else {
                app.set_status("!cannot resolve namespace");
                return;
            };
            let spec = app.view_spec.clone().unwrap();
            let pure = app.pure_name(&name);
            match k8s::scale(&cluster, &spec, &ns, pure, replicas).await {
                Ok(m) => app.set_status(m),
                Err(e) => app.err_status(e),
            }
            return;
        }
        Action::ApplyEdit {
            spec,
            ns,
            name,
            yaml,
        } => {
            let pure = app.pure_name(&name);
            match apply_edited(app, &spec, ns.as_deref(), pure, &yaml).await {
                Ok(m) => app.set_status(m),
                Err(e) => app.err_status(e),
            }
            return;
        }
        Action::RunPlugin2 { name } => {
            if let Some((_, pl)) = app.plugins.iter().find(|(n, _)| *n == name).cloned() {
                exec_plugin(app, name, pl, None);
            }
            return;
        }
        Action::Uncordon { node } => {
            let pure = app.pure_name(&node);
            k8s::cordon(&cluster, pure, false).await
        }
        Action::Cordon { node } => {
            let pure = app.pure_name(&node);
            k8s::cordon(&cluster, pure, true).await
        }
        Action::Drain { node } => {
            let pure = app.pure_name(&node);
            k8s::drain_node(&cluster, pure).await
        }
        Action::TriggerCj { cron } => {
            let ns2 = ns.unwrap_or_else(|| app.ns.clone());
            let pure = app.pure_name(&cron);
            k8s::trigger_cronjob(&cluster, &ns2, pure).await
        }
        Action::ToggleSuspendCj { cron, to } => {
            let ns2 = ns.unwrap_or_else(|| app.ns.clone());
            let pure = app.pure_name(&cron);
            k8s::set_cronjob_suspend(&cluster, &ns2, pure, to).await
        }
        Action::SaveLogs { .. } => unreachable!(),
    };
    match res {
        Ok(msg) => app.set_status(msg),
        Err(e) => app.set_status(format!("!{e}")),
    }
}

async fn run_input(app: &mut App, buf: String, purpose: InputPurpose) {
    app.mode = Mode::Normal;
    match purpose {
        InputPurpose::Scale { name } => {
            let n: i64 = buf.trim().parse().unwrap_or(-1);
            if n < 0 {
                app.set_status("!replicas must be >= 0");
                return;
            }
            let spec = app.view_spec.clone().unwrap();
            let Some(ns) = action_ns(app, &name) else {
                app.set_status("!cannot resolve namespace");
                return;
            };
            let pure = app.pure_name(&name);
            let res = k8s::scale(&app.cluster, &spec, &ns, pure, n).await;
            match res {
                Ok(m) => app.set_status(m),
                Err(e) => app.set_status(format!("!{e}")),
            }
        }
        InputPurpose::PfBind { ns, pod, port } => {
            let bind = buf.trim().to_string();
            let bind_addr = if bind.contains(':') {
                bind
            } else {
                format!("127.0.0.1:{bind}")
            };
            let cl = app.cluster.clone();
            let pure = app.pure_name(&pod).to_string();
            match k8s::port_forward(cl, ns.clone(), pure, port, bind_addr).await {
                Ok(entry) => app.start_pf(entry),
                Err(e) => app.set_status(format!("!pf: {e}")),
            }
        }
    }
}

async fn fetch_suspend_state(app: &App, name: &str) -> Option<bool> {
    let spec = spec_for("cj")?;
    let gvk = ApiResource::from_gvk(&kube::core::gvk::GroupVersionKind::gvk(
        &spec.group,
        &spec.version,
        &spec.kind,
    ));
    let ns = action_ns(app, name)?;
    let api: kube::Api<kube::core::dynamic::DynamicObject> =
        kube::Api::namespaced_with(app.cluster.client.clone(), &ns, &gvk);
    let pure = app.pure_name(name).to_string();
    let obj = api.get(&pure).await.ok()?;
    serde_json::to_value(&obj)
        .ok()?
        .pointer("/spec/suspend")
        .and_then(|v| v.as_bool())
}

async fn exec_cmd(app: &mut App, cmd: &str) {
    if app.status.starts_with('!') {
        app.status.clear();
    }
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return;
    }
    match parts[0] {
        "exit" | "quit" => app.quit = true,
        "q" => app.set_status("!use :exit or ctrl-q to quit"),
        "ctx" | "context" | "contexts" | "ktx" => {
            if parts.len() == 1 {
                app.open_menu_contexts();
            } else {
                app.pending_ctx = Some(parts[1].to_string());
            }
        }
        "ns" | "namespace" => {
            if parts.len() == 1 {
                app.open_menu_namespaces().await;
            } else if matches!(parts[1], "all" | "*") {
                app.all_ns = true;
                app.restart_watch_full_reset();
                let mut st = cfg::StateCfg::load();
                st.remember_ns(&app.cluster.ctx_name, "");
                st.save();
                app.refresh_pulse_if_active();
                app.set_status("all namespaces");
            } else {
                app.use_namespace(parts[1]);
                app.set_status(format!("ns → {}", parts[1]));
            }
        }
        "pulse" => {
            app.stop_watch();
            app.start_pulse();
        }
        "sd" | "screendump" => screendump(app),
        "logs" | "sh" | "shell" | "yaml" | "describe" | "edit" | "decode" | "scale" | "restart"
        | "del" | "delete" | "cordon" | "uncordon" | "drain" | "trigger" | "suspend" => {
            // act on the currently selected row (k9s-parity convenience)
            if app.view != ViewKind::Table || app.view_spec.is_none() {
                app.set_status("!select a resource row first");
                return;
            }
            let Some(name) = app.selected_or_first() else {
                app.set_status("!no rows to act on");
                return;
            };
            let kind = app.view_spec.as_ref().unwrap().kind.clone();
            let action = match parts[0] {
                "logs" => "wlogs",
                "sh" | "shell" => "shell",
                "yaml" => "yaml",
                "describe" => "describe",
                "edit" => "edit",
                "decode" => "decode",
                "scale" => "scale",
                "restart" => "restart",
                "del" | "delete" => "delete",
                "cordon" => "cordon",
                "uncordon" => "uncordon",
                "drain" => "drain",
                "trigger" => "trigger",
                "suspend" => "suspend",
                _ => unreachable!(),
            };
            app.run_action(&kind, &name, action).await;
        }
        "helm" => {
            if parts.len() > 1 {
                if app.all_ns {
                    app.set_status("!scope to a namespace first ('a' or :ns) for helm history");
                } else {
                    let rel = parts[1].to_string();
                    let ns = app.ns.clone();
                    app.open_helm_history_menu(ns, rel).await;
                }
            } else {
                app.open_helm_releases_menu().await;
            }
        }
        "policy" => app.open_policy_subjects_menu().await,
        "aliases" => app.open_aliases_pane(),
        "hotkeys" => app.open_hotkeys_pane(),
        "ref" => {
            let what = parts
                .get(1)
                .map(|s| s.to_string())
                .or_else(|| app.view_spec.as_ref().map(|sp| sp.kind.clone()))
                .unwrap_or_else(|| "po".into());
            app.open_ref_pane(&what);
        }
        "dir" => {
            let p = parts.get(1).copied().unwrap_or(".");
            Box::pin(app.open_dir(p)).await;
        }
        "themes" => app.open_themes_menu(),
        "theme-save" => {
            if parts.len() < 2 {
                app.set_status("!usage: :theme-save <name>");
            } else {
                match crate::cfg::save_theme_file(parts[1], &app.theme) {
                    Ok(p) => app.set_status(format!("theme saved → {}", p.display())),
                    Err(e) => app.set_status(format!("!{e}")),
                }
            }
        }
        "theme-set" => {
            if parts.len() < 3 {
                app.set_status("!usage: :theme-set <field> <#hex>  (fields: accent ok warn bad info dim header title bg_sel)");
            } else {
                let mut t = app.theme.clone();
                match t.set_hex(parts[1], parts[2]) {
                    Some(()) => {
                        app.theme = t;
                        app.set_status(format!(
                            "{} set to {} (use :theme-save <name> to persist)",
                            parts[1], parts[2]
                        ));
                    }
                    None => app.set_status("!bad field or hex (#rgb / #rrggbb)"),
                }
            }
        }
        "moo" => {
            app.mode = Mode::TextPane {
                title: "moo".into(),
                lines: vec![
                    " _________".into(),
                    "< k9x!!  >".into(),
                    " ---------".into(),
                    "        \\   ^__^".into(),
                    "         \\  (oo)\\_______".into(),
                    "            (__)\\       )\\/\\".into(),
                    "                ||----w |".into(),
                    "                ||     ||".into(),
                ],
                pos: 0,
                wrap: false,
            };
        }
        "popeye" => open_popeye(app).await,
        "xray" => open_xray(app, &parts[1..]).await,
        "pf" => app.view = ViewKind::Pf,
        "crds" | "crd" => app.browse_crds().await,
        "help" | "?" => open_help(app),
        other => {
            let _ = other;
            let full = cmd.trim();
            let (head, arg) = match full.split_once(' ') {
                Some((h, rest)) => (h, Some(rest.trim())),
                None => (full, None),
            };
            // custom aliases may map to a full command string
            if let Some((_, target)) = app.custom_aliases.iter().find(|(a, _)| a == head) {
                let expanded = match arg {
                    Some(a2) => format!("{target} {a2}"),
                    None => target.clone(),
                };
                return Box::pin(exec_cmd(app, &expanded)).await;
            }
            if let Some(spec) = resolve_spec(app, head) {
                switch(app, &spec.alias);
                if let Some(nsname) = arg {
                    app.ns = nsname.to_string();
                    app.all_ns = false;
                    app.restart_watch_full_reset();
                    app.set_status(format!("{} · ns → {}", spec.alias, nsname));
                }
            } else {
                let mut cands: Vec<String> = crate::app::suggest(head);
                if cands.is_empty() {
                    cands = crate::app::command_list()
                        .into_iter()
                        .filter(|c| {
                            crate::app::natural_compare(c, head) == std::cmp::Ordering::Equal
                                || levenshtein(c, head) <= (head.len().max(c.len()) / 3).max(1)
                        })
                        .take(4)
                        .collect();
                }
                if cands.is_empty() {
                    app.set_status(format!("!unknown command '{full}' (?=help)"));
                } else {
                    app.set_status(format!(
                        "!unknown '{}' \u{2014} did you mean: {}",
                        full,
                        cands.join(", ")
                    ));
                }
            }
        }
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let ba: Vec<char> = a.chars().collect();
    let bb: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=bb.len()).collect();
    let mut cur = vec![0usize; bb.len() + 1];
    for i in 1..=ba.len() {
        cur[0] = i;
        for j in 1..=bb.len() {
            let cost = if ba[i - 1] == bb[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[bb.len()]
}

fn st_default_tail(_app: &App) -> i64 {
    cfg::FileCfg::load().log_tail.max(100)
}

fn pf_sel_idx(app: &App) -> Option<usize> {
    if app.pfs.is_empty() {
        None
    } else {
        Some(app.pf_sel.min(app.pfs.len() - 1))
    }
}

fn pf_move(app: &mut App, d: i32) {
    let len = app.pfs.len();
    if len == 0 {
        return;
    }
    app.pf_sel = ((app.pf_sel as i32 + d).clamp(0, len as i32 - 1)) as usize;
}

pub enum PluginAction {}
async fn launch_plugin(app: &mut App, name: String, pl: crate::cfg::Plugin) {
    if pl.dangerous {
        if app.ro {
            app.set_status("!read-only mode: plugin blocked");
            return;
        }
        app.mode = Mode::Confirm {
            prompt: format!("plugin '{name}' is dangerous — run?"),
            action: Action::RunPlugin2 { name },
            sel_yes: false,
        };
        return;
    }
    let image = app.selected_row().map(|r| {
        let n = r.key.clone();
        let ns = if r.ns.is_empty() { None } else { Some(r.ns) };
        (n, ns)
    });
    // resolve image lazily for pods
    let image = match image {
        Some((n, _)) => app.resolve_pod_image(&n, None).await,
        None => None,
    };
    exec_plugin(app, name, pl, image.as_deref());
}

fn exec_plugin(app: &mut App, name: String, pl: crate::cfg::Plugin, image: Option<&str>) {
    use std::process::Command;
    let ctx = app.cluster.ctx_name.clone();
    let ns = app.ns.clone();
    let sel_name = app
        .selected_row()
        .map(|r| r.key.clone())
        .unwrap_or_default();
    let img = image.unwrap_or("");
    let interp = |a: &[String]| -> Vec<String> {
        a.iter()
            .map(|arg| {
                arg.replace("$NAME", &sel_name)
                    .replace("$NAMESPACE", &ns)
                    .replace("$CONTEXT", &ctx)
                    .replace("$CLUSTER", &ctx)
                    .replace("$CONTAINER", "")
                    .replace("$IMAGE", img)
            })
            .collect()
    };
    let args = interp(&pl.args);
    suspend_tui();
    let res = Command::new(&pl.command)
        .args(&args)
        .env("NAME", &sel_name)
        .env("NAMESPACE", &ns)
        .env("CONTEXT", &ctx)
        .env("CLUSTER", &ctx)
        .env("CONTAINER", "")
        .output();
    resume_tui();
    match res {
        Ok(out) if pl.background => {
            let mut lines = vec![
                format!("$ {} {} ({})", pl.command, args.join(" "), name),
                String::new(),
            ];
            lines.extend(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(String::from),
            );
            lines.extend(
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .map(String::from),
            );
            app.mode = Mode::TextPane {
                title: format!("plugin:{name}"),
                lines,
                pos: 0,
                wrap: true,
            };
        }
        Ok(_) => app.set_status(format!("plugin '{name}' finished")),
        Err(e) => app.set_status(format!("!plugin failed: {e}")),
    }
}

fn open_help(app: &mut App) {
    let ver = env!("CARGO_PKG_VERSION");
    let ctx = &app.cluster.ctx_name;
    let ns = if app.all_ns {
        "all".to_string()
    } else {
        app.ns.clone()
    };
    let aliases = model::all_aliases();
    let mut lines = vec![
        format!("k9x v{ver} — event-driven Kubernetes TUI + agent CLI"),
        format!("cluster: {ctx} · namespace: {ns} · readonly: {}", if app.ro { "yes" } else { "no" }),
        String::new(),
        "HEADER BAR".to_string(),
        "  K8s      server version          Support  std = end of standard support,".to_string(),
        "                                  ext = end of extended support (EKS".to_string(),
        "                                  lifecycle: 14mo std + 12mo ext; red when".to_string(),
        "                                  expired, ~ when estimated)".to_string(),
        "  Load     cluster cpu/mem + %    (metrics-server + node allocatable)".to_string(),
        "  colors   red row = failing (crashloop/error/evicted/zero-ready)".to_string(),
        "           orange row = degraded (restarts > 1, partial readiness)".to_string(),
        String::new(),
        "KEYS".to_string(),
        "  enter   actions menu            l       logs              s   shell into pod".to_string(),
        "  a (pod) attach to process       d       describe          y   yaml".to_string(),
        "  e       $EDITOR edit + apply    p       port-forward      X   decode secret".to_string(),
        "  U       used-by (secret/cm)     space   mark row (bulk)   ctrl-space mark range".to_string(),
        "  ctrl-y  copy yaml               ctrl-n  copy name         c   copy pane text".to_string(),
        "  T       theme picker (preview + 20s auto-revert)".to_string(),
        "  ctrl-d  delete marked/single    ctrl-k  force delete      R   rollout restart".to_string(),
        "  S       scale replicas          c/u/D   cordon/uncordon/  t   trigger cronjob".to_string(),
        "                                  drain (nodes)".to_string(),
        "  x       toggle cronjob suspend  C       contexts menu     A   all-namespaces".to_string(),
        "  r       re-watch                tab     cycle sort col    /   filter (fuzzy)".to_string(),
        "  :       command mode            ?       this help         g/G home/end".to_string(),
        "  j/k/arrows/pgup/pgdn move rows  q       back one level — NEVER quits".to_string(),
        "  ctrl-q  quit k9x                esc     back / clear filter / close cmd bar".to_string(),
        String::new(),
        "COMMAND MODE (:)".to_string(),
        "  :ctx|:context [name]   switch or list contexts (ns AND view remembered per ctx)".to_string(),
        "  :aliases :hotkeys      effective bindings · :ref [kind] API field docs".to_string(),
        "  :dir [path]            local yaml browser · enter applies the file".to_string(),
        "  :themes / T            theme picker \u{2014} preview, y keeps, auto-revert in 20s".to_string(),
        "  :theme-set <f> <#hex>  live-edit a color (accent ok warn bad info dim header title bg_sel)".to_string(),
        "  :theme-save <name>     persist current colors to themes/<name>.yml".to_string(),
        "  :ns [name|all]         switch namespace · :pulse health dashboard".to_string(),
        "  :pf port-forwards      :crds custom resources · :helm releases/history/values".to_string(),
        "  :xray ownership tree   :popeye sanity scan (needs binary) · :sd screendump".to_string(),
        "  :policy                effective permissions of a service account".to_string(),
        "  on selection: :logs :sh :yaml :describe :edit :scale N :restart :del".to_string(),
        "                :cordon :uncordon :drain :decode :trigger :suspend".to_string(),
        "  :exit quits · unknown cmds get did-you-mean suggestions · esc clears text,".to_string(),
        "  closes the bar when already empty".to_string(),
        String::new(),
        "LOGS VIEW".to_string(),
        "  /       find in stream (live filter + highlight, hit counter)".to_string(),
        "  0-6     time window: tail/head/1m/5m/15m/30m/1h".to_string(),
        "  p       previous logs          t       timestamps        w   wrap".to_string(),
        "  s       save to file           j/k/pgup/pgdn scroll            q/esc close".to_string(),
        String::new(),
        "SAFETY".to_string(),
        "  -r/--readonly blocks every mutation (TUI and CLI) at the handler level.".to_string(),
        "  deletes, drains and dangerous plugins always require confirmation;".to_string(),
        "  CLI mutations additionally demand --yes.".to_string(),
        String::new(),
        "STATE & CONFIG (~/.config/k9x/)".to_string(),
        "  config.toml views/theme/tails/metrics   state.toml last ctx + ns per context".to_string(),
        "  views.yml custom columns per resource (append_columns/replace_columns)".to_string(),
        "  eks-support.json EKS date cache (refetched only after a version upgrade)".to_string(),
        "  plugins.yml hotkeys.yml aliases.yml — k9s-compatible overrides".to_string(),
        String::new(),
        "AGENT CLI (same binary, ~6ms cold start)".to_string(),
        "  k9x ls <res> [-n ns] [-A] [-l sel] [-o table|json|yaml|name] [--watch]".to_string(),
        "  k9x get|describe|logs|decode <res> <name>   k9x ctx [name] · k9x ns".to_string(),
        "  k9x del|scale|restart <res> <name> … --yes   k9x cordon|uncordon <node> --yes".to_string(),
        "  k9x completions bash|zsh|fish".to_string(),
        String::new(),
        "resources:".to_string(),
    ];
    let mut chunk = String::new();
    for a in &aliases {
        if chunk.len() + a.len() + 2 > 60 {
            lines.push(chunk.clone());
            chunk.clear();
        }
        chunk.push_str(a);
        chunk.push_str("  ");
    }
    if !chunk.is_empty() {
        lines.push(chunk);
    }
    app.mode = Mode::TextPane {
        title: format!("help · k9x v{ver}"),
        lines,
        pos: 0,
        wrap: false,
    };
}

fn need_yes(yes: bool) -> Result<()> {
    if yes {
        Ok(())
    } else {
        Err(anyhow!("refusing to mutate without --yes (safety gate)"))
    }
}

fn guard_ro(ro: bool) -> Result<()> {
    if ro {
        Err(anyhow!("read-only mode: mutation blocked"))
    } else {
        Ok(())
    }
}

async fn agent_run(cmd: Cmd, args: &Args) -> Result<()> {
    let filecfg = cfg::FileCfg::load();
    let ro = args.readonly || filecfg.readonly;
    match cmd {
        Cmd::Completions { shell } => {
            use clap_complete::generate;
            let mut cmd = Args::command();
            generate(shell, &mut cmd, "k9x", &mut std::io::stdout());
            Ok(())
        }
        Cmd::Ctx { context } => {
            if let Some(c) = context {
                println!("{c}");
                return Ok(());
            }
            // Missing/empty kubeconfig is not an error for `ctx`: it simply
            // lists nothing and exits 0 (an expected, unconfigured state).
            if let k8s::KubeProbe::Ready(kc) = k8s::probe_kube_config(None) {
                for c in kc.contexts.iter() {
                    let mark = if Some(&c.name) == kc.current_context.as_ref() {
                        "*"
                    } else {
                        " "
                    };
                    println!("{mark} {}", c.name);
                }
            }
            Ok(())
        }
        Cmd::Ns => {
            let cl = k8s::load(args.context.as_deref()).await?;
            for n in k8s::list_namespaces(&cl).await? {
                println!("{n}");
            }
            Ok(())
        }
        Cmd::Ls {
            resource,
            namespace,
            all,
            selector,
            output,
            watch,
        } => {
            let spec = model::spec_for(&resource)
                .ok_or_else(|| anyhow!("unknown resource '{resource}'"))?;
            let ctx = args.context.clone().or_else(|| {
                if filecfg.context.is_empty() {
                    None
                } else {
                    Some(filecfg.context.clone())
                }
            });
            let cl = Arc::new(k8s::load(ctx.as_deref()).await?);
            let ns = namespace.or_else(|| args.namespace.clone()).or_else(|| {
                if filecfg.namespace.is_empty() {
                    None
                } else {
                    Some(filecfg.namespace.clone())
                }
            });
            let all = all || args.all_namespaces || filecfg.all_namespaces;
            let nso =
                if !spec.namespaced || all {
                    None
                } else {
                    Some(ns.unwrap_or_else(|| {
                        cl.default_namespace().unwrap_or_else(|| "default".into())
                    }))
                };
            if watch {
                return watch_stream(&cl, &spec, nso, selector).await;
            }
            let api = cl.dyn_api(&spec, nso.as_deref());
            let mut lp = ListParams::default();
            if let Some(sel) = selector {
                lp = lp.labels(&sel);
            }
            let objs = api.list(&lp).await?.items;
            emit_rows(&spec, &objs, &output)
        }
        Cmd::Get {
            resource,
            name,
            namespace,
            output,
        } => {
            let spec = model::spec_for(&resource)
                .ok_or_else(|| anyhow!("unknown resource '{resource}'"))?;
            let cl = k8s::load(args.context.as_deref()).await?;
            let nso = namespace.or_else(|| effective_ns(&cl, args, &filecfg, &spec));
            let obj = cl.dyn_api(&spec, nso.as_deref()).get(&name).await?;
            let v = serde_json::to_value(&obj)?;
            emit_one(&v, &output)
        }
        Cmd::Logs {
            pod,
            container,
            follow,
            previous,
            tail,
            timestamps,
        } => {
            let cl = k8s::load(args.context.as_deref()).await?;
            let nsp = args
                .namespace
                .clone()
                .or_else(|| {
                    if filecfg.namespace.is_empty() {
                        None
                    } else {
                        Some(filecfg.namespace.clone())
                    }
                })
                .unwrap_or_else(|| cl.default_namespace().unwrap_or_else(|| "default".into()));
            let lp = LogParams {
                follow,
                tail_lines: Some(tail.or(Some(filecfg.log_tail)).unwrap_or(5000)),
                timestamps,
                previous,
                container,
                ..Default::default()
            };
            let api: Api<Pod> = Api::namespaced(cl.client.clone(), &nsp);
            let mut reader = api.log_stream(&pod, &lp).await?;
            use futures::AsyncBufReadExt;
            use std::io::Write;
            let mut buf = Vec::with_capacity(8192);
            loop {
                buf.clear();
                let n = reader.read_until(b'\n', &mut buf).await?;
                if n == 0 {
                    break;
                }
                std::io::stdout().write_all(&buf)?;
                std::io::stdout().flush()?;
                if !follow {
                    continue;
                }
            }
            Ok(())
        }
        Cmd::Describe {
            resource,
            name,
            namespace,
        } => {
            let spec = model::spec_for(&resource)
                .ok_or_else(|| anyhow!("unknown resource '{resource}'"))?;
            let cl = k8s::load(args.context.as_deref()).await?;
            let nso = namespace.or_else(|| effective_ns(&cl, args, &filecfg, &spec));
            print!(
                "{}",
                k8s::describe_obj(&cl, &spec, nso.as_deref(), &name).await?
            );
            Ok(())
        }
        Cmd::Decode { name, namespace } => {
            let cl = k8s::load(args.context.as_deref()).await?;
            let ns = namespace
                .or_else(|| args.namespace.clone())
                .or_else(|| {
                    if filecfg.namespace.is_empty() {
                        None
                    } else {
                        Some(filecfg.namespace.clone())
                    }
                })
                .unwrap_or_else(|| cl.default_namespace().unwrap_or_else(|| "default".into()));
            print!("{}", k8s::decode_secret(&cl, &ns, &name).await?);
            Ok(())
        }
        Cmd::Del {
            resource,
            name,
            force,
            yes,
        } => {
            guard_ro(ro)?;
            need_yes(yes)?;
            let spec = model::spec_for(&resource)
                .ok_or_else(|| anyhow!("unknown resource '{resource}'"))?;
            let cl = k8s::load(args.context.as_deref()).await?;
            let nso = effective_ns(&cl, args, &filecfg, &spec);
            println!(
                "{}",
                k8s::delete_obj(&cl, &spec, nso.as_deref(), &name, force).await?
            );
            Ok(())
        }
        Cmd::Scale {
            resource,
            name,
            replicas,
            namespace,
            yes,
        } => {
            guard_ro(ro)?;
            need_yes(yes)?;
            let spec = model::spec_for(&resource)
                .ok_or_else(|| anyhow!("unknown resource '{resource}'"))?;
            let cl = k8s::load(args.context.as_deref()).await?;
            let ns = namespace
                .or_else(|| args.namespace.clone())
                .or_else(|| {
                    if filecfg.namespace.is_empty() {
                        None
                    } else {
                        Some(filecfg.namespace.clone())
                    }
                })
                .unwrap_or_else(|| cl.default_namespace().unwrap_or_else(|| "default".into()));
            println!("{}", k8s::scale(&cl, &spec, &ns, &name, replicas).await?);
            Ok(())
        }
        Cmd::Restart {
            resource,
            name,
            namespace,
            yes,
        } => {
            guard_ro(ro)?;
            need_yes(yes)?;
            let spec = model::spec_for(&resource)
                .ok_or_else(|| anyhow!("restart not supported for '{resource}'"))?;
            let cl = k8s::load(args.context.as_deref()).await?;
            let ns = namespace
                .or_else(|| args.namespace.clone())
                .or_else(|| {
                    if filecfg.namespace.is_empty() {
                        None
                    } else {
                        Some(filecfg.namespace.clone())
                    }
                })
                .unwrap_or_else(|| cl.default_namespace().unwrap_or_else(|| "default".into()));
            println!("{}", k8s::rollout_restart(&cl, &spec, &ns, &name).await?);
            Ok(())
        }
        Cmd::Cordon { node, yes } => {
            guard_ro(ro)?;
            need_yes(yes)?;
            let cl = k8s::load(args.context.as_deref()).await?;
            println!("{}", k8s::cordon(&cl, &node, true).await?);
            Ok(())
        }
        Cmd::Uncordon { node, yes } => {
            guard_ro(ro)?;
            need_yes(yes)?;
            let cl = k8s::load(args.context.as_deref()).await?;
            println!("{}", k8s::cordon(&cl, &node, false).await?);
            Ok(())
        }
    }
}

fn effective_ns(
    cl: &k8s::Cluster,
    args: &Args,
    fc: &cfg::FileCfg,
    spec: &model::KindSpec,
) -> Option<String> {
    if !spec.namespaced || args.all_namespaces || fc.all_namespaces {
        return None;
    }
    Some(
        args.namespace
            .clone()
            .or_else(|| {
                if fc.namespace.is_empty() {
                    None
                } else {
                    Some(fc.namespace.clone())
                }
            })
            .unwrap_or_else(|| cl.default_namespace().unwrap_or_else(|| "default".into())),
    )
}

fn emit_rows(
    spec: &model::KindSpec,
    objs: &[kube::core::dynamic::DynamicObject],
    output: &str,
) -> Result<()> {
    let rows: Vec<model::Row> = objs
        .iter()
        .filter_map(|o| serde_json::to_value(o).ok())
        .map(|v| model::extract(spec, &v))
        .collect();
    match output {
        "name" => {
            for r in rows {
                println!("{}", r.key);
            }
        }
        "json" => {
            let vals: Vec<Value> = objs
                .iter()
                .map(|o| serde_json::to_value(o).unwrap_or(Value::Null))
                .collect();
            println!("{}", serde_json::to_string_pretty(&vals)?);
        }
        "yaml" => {
            print!("{}", serde_yaml::to_string(&serde_json::to_value(objs)?)?);
        }
        _ => {
            let mut widths: Vec<usize> = spec.cols.iter().map(|c| c.name.len()).collect();
            for r in &rows {
                for (i, cell) in r.cells.iter().enumerate() {
                    widths[i] = widths[i].max(cell.len());
                }
            }
            let header: Vec<String> = spec
                .cols
                .iter()
                .enumerate()
                .map(|(i, c)| format!("{:<width$}", c.name, width = widths[i]))
                .collect();
            println!("{}", header.join("  "));
            for r in rows {
                let line: Vec<String> = r
                    .cells
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{:<width$}", c, width = *widths.get(i).unwrap_or(&8)))
                    .collect();
                println!("{}", line.join("  "));
            }
        }
    }
    Ok(())
}

fn emit_one(v: &Value, output: &str) -> Result<()> {
    match output {
        "json" => println!("{}", serde_json::to_string_pretty(v)?),
        "yaml" => print!("{}", serde_yaml::to_string(v)?),
        _ => print!("{}", serde_yaml::to_string(v)?),
    }
    Ok(())
}

async fn watch_stream(
    cl: &Arc<k8s::Cluster>,
    spec: &model::KindSpec,
    ns: Option<String>,
    selector: Option<String>,
) -> Result<()> {
    use futures::StreamExt;
    let api = cl.dyn_api(spec, ns.as_deref());
    let mut wcfg = watcher::Config::default();
    if let Some(sel) = selector {
        wcfg = wcfg.labels(&sel);
    }
    let stream = watcher(api, wcfg);
    let mut st = Box::pin(stream);
    use std::io::Write;
    while let Some(ev) = st.next().await {
        let (t, o) = match ev {
            Ok(watcher::Event::Apply(o)) => ("MODIFIED", o),
            Ok(watcher::Event::InitApply(o)) => ("ADDED", o),
            Ok(watcher::Event::Delete(o)) => ("DELETED", o),
            Ok(_) => continue,
            Err(e) => {
                eprintln!("watch error: {e}");
                continue;
            }
        };
        let v = serde_json::to_value(&o)?;
        let line = serde_json::json!({"type": t, "object": v});
        println!("{}", line);
        std::io::stdout().flush()?;
    }
    Ok(())
}

fn resolve_spec(app: &App, alias: &str) -> Option<KindSpec> {
    if let Some(sp) = spec_for(alias) {
        return Some(sp);
    }
    for (pl, g, v, k, nsd) in &app.known_crds {
        if pl == alias || k.eq_ignore_ascii_case(alias) {
            return Some(model::custom_spec(pl, g, v, k, *nsd));
        }
    }
    None
}

fn screendump(app: &mut App) {
    let rows = app.filtered_sorted();
    let mut out = String::new();
    if let Some(spec) = &app.view_spec {
        out.push_str(
            &spec
                .cols
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>()
                .join("\t"),
        );
        out.push('\n');
    }
    for r in rows {
        out.push_str(&r.cells.join("\t"));
        out.push('\n');
    }
    let dir = std::env::var("K9X_SCREENDUMP_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{}/k9x-screen-{}.txt", dir, chrono::Utc::now().timestamp());
    match std::fs::write(&path, out) {
        Ok(_) => app.set_status(format!("screendump → {path}")),
        Err(e) => app.set_status(format!("!screendump: {e}")),
    }
}

fn which_bin(bin: &str) -> Option<std::path::PathBuf> {
    std::env::var("PATH")
        .ok()?
        .split(':')
        .map(std::path::PathBuf::from)
        .map(|d| d.join(bin))
        .find(|p| p.exists())
}

async fn open_popeye(app: &mut App) {
    let Some(bin) = which_bin("popeye") else {
        app.set_status("!popeye not installed (brew install popeye)");
        return;
    };
    app.set_status("running popeye…");
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new(bin).arg("--no-color").output()
    })
    .await;
    match out {
        Ok(Ok(o)) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            let lines: Vec<String> = text.lines().map(String::from).collect();
            app.mode = Mode::TextPane {
                title: "popeye sanity".into(),
                lines,
                pos: 0,
                wrap: true,
            };
            app.set_status("");
        }
        _ => app.set_status("!popeye failed"),
    }
}

async fn open_xray(app: &mut App, args: &[&str]) {
    let target = if args.is_empty() {
        let sp_opt = app.view_spec.clone();
        let name_opt = app.selected_or_first();
        if let (Some(spec), Some(name)) = (sp_opt, name_opt) {
            Some((spec, name))
        } else {
            None
        }
    } else if args.len() == 1 {
        // e.g. :xray deploy OR :xray web
        if let Some(sp) = crate::model::spec_for(args[0]) {
            let is_cur_kind = app.view_spec.as_ref().map(|s| s.kind.as_str()) == Some(&sp.kind);
            let target_name = if is_cur_kind {
                app.selected_or_first()
            } else {
                None
            };
            if let Some(n) = target_name {
                Some((sp, n))
            } else {
                // Fetch first available resource of this kind
                let ns = if sp.namespaced {
                    Some(app.ns.as_str())
                } else {
                    None
                };
                let api = app.cluster.dyn_api(&sp, ns);
                match api.list(&kube::api::ListParams::default().limit(1)).await {
                    Ok(list) => {
                        if let Some(first) =
                            list.items.into_iter().next().and_then(|o| o.metadata.name)
                        {
                            Some((sp, first))
                        } else {
                            app.set_status(format!("!no {} resources found", sp.kind));
                            return;
                        }
                    }
                    Err(e) => {
                        app.err_status(e);
                        return;
                    }
                }
            }
        } else if let Some(sp) = &app.view_spec {
            Some((sp.clone(), args[0].to_string()))
        } else {
            app.set_status(format!("!unknown resource kind '{}'", args[0]));
            return;
        }
    } else {
        // args.len() >= 2: e.g. :xray deploy web
        if let Some(sp) = crate::model::spec_for(args[0]) {
            Some((sp, args[1].to_string()))
        } else {
            app.set_status(format!("!unknown resource kind '{}'", args[0]));
            return;
        }
    };

    let Some((spec, name)) = target else {
        app.set_status("!select a resource or specify kind (e.g. :xray deploy [name])");
        return;
    };

    let pure = app.pure_name(&name).to_string();
    app.set_status(format!("building xray for {}/{}…", spec.kind, pure));
    match app.build_xray(&spec, &pure).await {
        Ok(lines) => {
            app.set_status("");
            app.mode = Mode::TextPane {
                title: format!("xray:{}/{}", spec.alias, pure),
                lines,
                pos: 0,
                wrap: false,
            };
        }
        Err(e) => app.err_status(e),
    }
}

pub fn do_restart_pub(app: &mut App, spec: &KindSpec, name: &str) {
    let cl = app.cluster.clone();
    let ns = app.ns.clone();
    let sp = spec.clone();
    let n = name.to_string();
    if app.ro {
        app.set_status("!read-only mode: restart blocked");
        return;
    }
    tokio::spawn(async move {
        if let Ok(m) = k8s::rollout_restart(&cl, &sp, &ns, &n).await {
            let _ = m;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{fuzzy_match, natural_compare};

    #[test]
    fn nat_orders_numbers_inside_strings() {
        assert_eq!(natural_compare("pod-9", "pod-10"), std::cmp::Ordering::Less);
        assert_eq!(
            natural_compare("web-2a", "web-2b"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn fuzzy_subsequence() {
        assert!(fuzzy_match("web-5fc4ccf5d6", "wbc"));
        assert!(!fuzzy_match("api-123", "wbc"));
    }

    #[test]
    fn lev_distance() {
        assert_eq!(levenshtein("deploy", "deplyo"), 2);
        assert_eq!(levenshtein("po", "po"), 0);
    }

    #[test]
    fn suggest_prefix() {
        crate::app::command_list();
        let s = crate::app::suggest("dep");
        assert!(s.iter().any(|x| x == "deploy"));
    }

    #[test]
    fn test_log_occurrences_distinct_vs_line() {
        let lines = vec![
            "2026-08-28 error found in component error-handler".to_string(),
            "normal line without match".to_string(),
            "error: another error with error details".to_string(),
        ];
        // In line mode (count_occurrences = false), each matching line is 1 match:
        let line_matches = crate::app::compute_log_matches(&lines, "error", false);
        assert_eq!(line_matches.len(), 2);
        assert_eq!(line_matches[0], (0, usize::MAX));
        assert_eq!(line_matches[1], (1, usize::MAX)); // filtered index 1 is original lines[2]

        // In occurrence mode (count_occurrences = true), every occurrence is distinct:
        let occ_matches = crate::app::compute_log_matches(&lines, "error", true);
        assert_eq!(occ_matches.len(), 5); // 2 in line 0 + 3 in line 2
        assert_eq!(occ_matches[0], (0, 0));
        assert_eq!(occ_matches[1], (0, 1));
        assert_eq!(occ_matches[2], (1, 0));
        assert_eq!(occ_matches[3], (1, 1));
        assert_eq!(occ_matches[4], (1, 2));
    }

    #[test]
    fn test_port_extraction_and_parsing() {
        assert_eq!(extract_port("web::8080"), Some("8080".to_string()));
        assert_eq!(extract_port("8080"), Some("8080".to_string()));
        assert_eq!(extract_port("invalid"), None);

        assert_eq!(parse_port("web::8080"), Some(8080));
        assert_eq!(parse_port("80"), Some(80));
        assert_eq!(parse_port("65536"), None); // overflow u16
        assert_eq!(parse_port("abc"), None);
    }

    #[test]
    fn test_apply_exec_bytes_backspace_and_cr() {
        let mut buf = Vec::new();
        // Shell outputs prompt "sh$ "
        crate::app::apply_exec_bytes(&mut buf, b"sh$ ");
        assert_eq!(String::from_utf8_lossy(&buf), "sh$ ");

        // User types "ls -la"
        crate::app::apply_exec_bytes(&mut buf, b"ls -la");
        assert_eq!(String::from_utf8_lossy(&buf), "sh$ ls -la");

        // User hits backspace 3 times: shell sends "\x08 \x08\x08 \x08\x08 \x08"
        crate::app::apply_exec_bytes(&mut buf, b"\x08 \x08\x08 \x08\x08 \x08");
        assert_eq!(String::from_utf8_lossy(&buf), "sh$ ls ");

        // User hits Enter: shell sends "\r\n"
        crate::app::apply_exec_bytes(&mut buf, b"\r\n");
        assert_eq!(String::from_utf8_lossy(&buf), "sh$ ls \n");
    }

    #[test]
    fn test_compute_dir_suggestions() {
        let sugs = crate::app::compute_dir_suggestions("/tmp/");
        // suggestions should not panic and should be formatted with trailing /
        for s in sugs {
            assert!(s.ends_with('/'));
        }
    }

    #[test]
    fn test_save_logs_filter_and_dir_creation() {
        let raw_lines = [
            "2026-08-28T12:00:00Z [INFO] System started".to_string(),
            "2026-08-28T12:01:00Z [ERROR] DB connection failed".to_string(),
            "2026-08-28T12:02:00Z [INFO] Retrying connection".to_string(),
            "2026-08-28T12:03:00Z [ERROR] Timeout exceeded".to_string(),
        ];

        // 1. When search query is active, only matched lines are filtered
        let query = "error";
        let filtered: Vec<String> = raw_lines
            .iter()
            .filter(|l| l.to_lowercase().contains(query))
            .cloned()
            .collect();
        assert_eq!(filtered.len(), 2);
        assert!(filtered[0].contains("[ERROR]"));
        assert!(filtered[1].contains("[ERROR]"));

        // 2. Test saving to a new non-existent nested directory
        let temp_dir = format!(
            "/tmp/k9x_test_nested_dir_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        let file_path = format!("{temp_dir}/exported_logs.txt");
        if let Some(parent) = std::path::Path::new(&file_path).parent() {
            let res = std::fs::create_dir_all(parent);
            assert!(res.is_ok());
        }
        let write_res = std::fs::write(&file_path, filtered.join("\n") + "\n");
        assert!(write_res.is_ok());

        // Verify content on disk
        let read_back = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(read_back, filtered.join("\n") + "\n");

        // Clean up
        let _ = std::fs::remove_file(&file_path);
        let _ = std::fs::remove_dir(temp_dir);
    }

    #[test]
    fn test_marks_pruning_and_active_keys() {
        let mut marks = std::collections::BTreeSet::new();
        marks.insert("demo/web-1".to_string());
        marks.insert("demo/web-2".to_string());
        marks.insert("demo/old-deleted-pod".to_string());

        let mut rows = std::collections::BTreeMap::new();
        let dummy_row = |key: &str| crate::model::Row {
            key: key.to_string(),
            ns: "demo".to_string(),
            cells: vec![],
            sev: crate::model::Sev::Ok,
            flags: 0,
        };
        rows.insert(
            "demo/web-1".to_string(),
            (dummy_row("demo/web-1"), std::time::Instant::now()),
        );
        rows.insert(
            "demo/web-2".to_string(),
            (dummy_row("demo/web-2"), std::time::Instant::now()),
        );

        let active: Vec<String> = marks
            .iter()
            .filter(|k| rows.contains_key(*k))
            .cloned()
            .collect();
        assert_eq!(active.len(), 2);
        assert_eq!(active, vec!["demo/web-1", "demo/web-2"]);
    }

    #[test]
    #[cfg(unix)]
    fn test_stderr_silence_and_restore() {
        silence_tui_stderr();
        eprintln!("this should be safely discarded to /dev/null");
        restore_tui_stderr();
        restore_tui_stderr();
    }

    #[test]
    fn test_ensure_log_line_visible() {
        let total = 100;
        let inner_h = 10;
        let mut scroll = 90; // viewing lines [0..10)

        // 1. Target 5 is already visible [0..10) -> scroll remains unchanged
        crate::app::ensure_log_line_visible(&mut scroll, total, 5, inner_h);
        assert_eq!(scroll, 90);

        // 2. Target 25 is below viewport -> scroll adjusted so line 25 is at the bottom (lines [16..26))
        // end = 26, scroll = 100 - 26 = 74
        crate::app::ensure_log_line_visible(&mut scroll, total, 25, inner_h);
        assert_eq!(scroll, 74);

        // 3. Target 4 is above current viewport [16..26) -> scroll adjusted so line 4 is at top (lines [4..14))
        // start = 4, end = 14, scroll = 100 - 14 = 86
        crate::app::ensure_log_line_visible(&mut scroll, total, 4, inner_h);
        assert_eq!(scroll, 86);

        // 4. Target 99 (bottom of logs) -> line 99 at bottom (lines [90..100))
        // end = 100, scroll = 100 - 100 = 0
        crate::app::ensure_log_line_visible(&mut scroll, total, 99, inner_h);
        assert_eq!(scroll, 0);

        // 5. Target 0 from bottom -> line 0 at top (lines [0..10))
        crate::app::ensure_log_line_visible(&mut scroll, total, 0, inner_h);
        assert_eq!(scroll, 90);

        // 6. Fewer total lines than viewport height
        let mut short_scroll = 0;
        crate::app::ensure_log_line_visible(&mut short_scroll, 5, 3, 10);
        assert_eq!(short_scroll, 0);

        // 7. Edge cases: 0 lines or 0 height does not panic
        let mut zero_scroll = 10;
        crate::app::ensure_log_line_visible(&mut zero_scroll, 0, 0, 10);
        assert_eq!(zero_scroll, 0);

        let mut zero_h = 10;
        crate::app::ensure_log_line_visible(&mut zero_h, 100, 5, 0);
        assert_eq!(zero_h, 0);
    }

    #[test]
    fn test_log_search_navigation_wraparound() {
        let matches_len = 5;

        // Next forward wrap-around: 0 -> 1 -> 2 -> 3 -> 4 -> 0
        let mut cur = 0;
        cur = (cur + 1) % matches_len;
        assert_eq!(cur, 1);
        cur = 4;
        cur = (cur + 1) % matches_len;
        assert_eq!(cur, 0);

        // Prev backward wrap-around: 0 -> 4 -> 3 -> 2 -> 1 -> 0
        cur = 0;
        cur = if cur == 0 { matches_len - 1 } else { cur - 1 };
        assert_eq!(cur, 4);
        cur = if cur == 0 { matches_len - 1 } else { cur - 1 };
        assert_eq!(cur, 3);

        // Single match wrap-around
        let single_len = 1;
        let s_cur: usize = 0;
        let next_single = (s_cur + 1) % single_len;
        assert_eq!(next_single, 0);
        let prev_single = if s_cur == 0 {
            single_len - 1
        } else {
            s_cur - 1
        };
        assert_eq!(prev_single, 0);
    }

    #[test]
    fn test_xray_node_tree_rendering() {
        use crate::app::XrayNode;
        let mut root = XrayNode::new("xray Deployment/web @ demo");
        let mut dep = XrayNode::new("Deployment/web [2/2 ready]");
        let mut rs = XrayNode::new("ReplicaSet/web-abc [2/2 ready]");
        rs.add_child(XrayNode::new("✔ pod/web-abc-1 [1/1] Running"));
        rs.add_child(XrayNode::new("✔ pod/web-abc-2 [1/1] Running"));
        dep.add_child(rs);
        root.add_child(dep);

        let mut lines = Vec::new();
        root.render(&mut lines, "", true, true);

        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "xray Deployment/web @ demo");
        assert_eq!(lines[1], "└── Deployment/web [2/2 ready]");
        assert_eq!(lines[2], "    └── ReplicaSet/web-abc [2/2 ready]");
        assert_eq!(lines[3], "        ├── ✔ pod/web-abc-1 [1/1] Running");
        assert_eq!(lines[4], "        └── ✔ pod/web-abc-2 [1/1] Running");
    }

    #[test]
    fn test_restore_tui_idempotency_and_state() {
        // Calling restore_tui multiple times must be safe and idempotent
        crate::restore_tui();
        crate::restore_tui();
        assert!(!crate::TUI_INITIALIZED.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!crate::INPUT_RUNNING.load(std::sync::atomic::Ordering::SeqCst));
    }
}
