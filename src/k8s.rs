use crate::model::{KindSpec, Row, extract};
use anyhow::{Context as _, Result, anyhow};
use chrono::Utc;
use futures::AsyncBufReadExt;
use kube::{
    Client, Config,
    api::{DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams},
    core::ApiResource,
    core::dynamic::DynamicObject,
    core::gvk::GroupVersionKind,
    runtime::watcher,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

type Pod = k8s_openapi::api::core::v1::Pod;

pub struct Cluster {
    pub client: Client,
    pub ctx_name: String,
    pub contexts: Vec<String>,
    pub kubeconfig: Kubeconfig,
}
use kube::Api;
use kube::config::Kubeconfig;
use std::collections::BTreeMap;
use std::time::Instant;

/// In-memory LRU connection pool caching active `Cluster` instances (HTTP/2 TLS connections)
#[derive(Default)]
pub struct ClusterPool {
    pool: BTreeMap<String, (Arc<Cluster>, Instant)>,
}

impl ClusterPool {
    pub fn new() -> Self {
        Self {
            pool: BTreeMap::new(),
        }
    }

    pub fn get(&mut self, ctx_name: &str) -> Option<Arc<Cluster>> {
        // 10 minutes idle TTL for connection reuse
        let now = Instant::now();
        if let Some((cluster, last_used)) = self.pool.get_mut(ctx_name)
            && now.duration_since(*last_used).as_secs() < 600
        {
            *last_used = now;
            return Some(cluster.clone());
        }
        None
    }

    pub fn insert(&mut self, ctx_name: String, cluster: Arc<Cluster>) {
        if self.pool.len() >= 6
            && let Some(oldest_key) = self
                .pool
                .iter()
                .min_by_key(|(_, (_, ts))| *ts)
                .map(|(k, _)| k.clone())
        {
            self.pool.remove(&oldest_key);
        }
        self.pool.insert(ctx_name, (cluster, Instant::now()));
    }
}

pub async fn load(context_override: Option<&str>) -> Result<Cluster> {
    let kc = Kubeconfig::read().context("kubeconfig read")?;
    let ctx_name = context_override
        .map(String::from)
        .or_else(|| kc.current_context.clone())
        .ok_or_else(|| anyhow!("no current-context set"))?;
    let mut kcc = kc.clone();
    kcc.current_context = Some(ctx_name.clone());
    let cfg = Config::from_custom_kubeconfig(kcc, &Default::default()).await?;
    let client = Client::try_from(cfg)?;
    let contexts = kc.contexts.iter().map(|c| c.name.clone()).collect();
    Ok(Cluster {
        client,
        ctx_name,
        contexts,
        kubeconfig: kc,
    })
}

pub async fn load_pooled(
    pool: &std::sync::Mutex<ClusterPool>,
    context_override: Option<&str>,
) -> Result<Arc<Cluster>> {
    let kc = Kubeconfig::read().context("kubeconfig read")?;
    let ctx_name = context_override
        .map(String::from)
        .or_else(|| kc.current_context.clone())
        .ok_or_else(|| anyhow!("no current-context set"))?;

    if let Ok(mut g) = pool.lock()
        && let Some(c) = g.get(&ctx_name)
    {
        return Ok(c);
    }

    let mut kcc = kc.clone();
    kcc.current_context = Some(ctx_name.clone());
    let cfg = Config::from_custom_kubeconfig(kcc, &Default::default()).await?;
    let client = Client::try_from(cfg)?;
    let contexts = kc.contexts.iter().map(|c| c.name.clone()).collect();
    let cluster = Arc::new(Cluster {
        client,
        ctx_name: ctx_name.clone(),
        contexts,
        kubeconfig: kc,
    });

    if let Ok(mut g) = pool.lock() {
        g.insert(ctx_name, cluster.clone());
    }
    Ok(cluster)
}

impl Cluster {
    pub fn default_namespace(&self) -> Option<String> {
        self.kubeconfig
            .contexts
            .iter()
            .find(|c| c.name == self.ctx_name)
            .and_then(|c| c.context.as_ref())
            .and_then(|cx| cx.namespace.clone())
            .or_else(|| Some("default".into()))
    }

    pub fn dyn_api(&self, spec: &KindSpec, ns: Option<&str>) -> Api<DynamicObject> {
        let gvk = gvk_of(spec);
        if spec.namespaced {
            match ns {
                Some(n) => Api::namespaced_with(self.client.clone(), n, &gvk),
                None => Api::all_with(self.client.clone(), &gvk),
            }
        } else {
            Api::all_with(self.client.clone(), &gvk)
        }
    }

    pub fn pod_api(&self, ns: &str) -> Api<Pod> {
        Api::namespaced(self.client.clone(), ns)
    }

    /// dynamic api straight from GVK parts (for kinds outside the KindSpec registry)
    pub fn dyn_api_kind(
        &self,
        group: &str,
        version: &str,
        kind: &str,
        ns: Option<&str>,
    ) -> Api<DynamicObject> {
        let gvk = ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, kind));
        match ns {
            Some(n) => Api::namespaced_with(self.client.clone(), n, &gvk),
            None => Api::all_with(self.client.clone(), &gvk),
        }
    }
}

fn gvk_of(spec: &KindSpec) -> ApiResource {
    ApiResource::from_gvk(&GroupVersionKind::gvk(
        &spec.group,
        &spec.version,
        &spec.kind,
    ))
}

#[derive(Debug)]
pub enum Msg {
    Reset,
    Up(Row),
    Down(String),
    Err(String),
    /// status-line text produced by background tasks
    Status(String),
    /// open a read-only text pane from a background task
    Pane {
        title: String,
        lines: Vec<String>,
        wrap: bool,
    },
    /// periodic cluster cpu/memory sample
    Res(ClusterRes),
    /// startup: CRD list (background)
    Crds(Vec<crate::k8s::CrdInfo>),
    /// startup: namespace shortcuts (background)
    Nss(Vec<String>),
    /// startup: apiserver version string
    Ver(String),
    /// startup: support dates resolved from the version
    Sup(crate::awsup::SupDates),
}

/// per-pod usage sample (keyed "ns/name")
#[derive(Clone, Debug, Default)]
pub struct PodUsage {
    pub cpu_m: f64,
    pub mem_b: u64,
    /// (container, cpu_m, mem_b)
    pub containers: Vec<(String, f64, u64)>,
}

/// on-demand per-pod usage (container drill-down)
pub async fn pod_metrics_one(cluster: &Cluster, ns: &str, pod: &str) -> Result<PodUsage> {
    let pgvk = ApiResource::from_gvk(&GroupVersionKind::gvk("metrics.k8s.io", "v1beta1", "Pod"));
    let api: Api<DynamicObject> = Api::namespaced_with(cluster.client.clone(), ns, &pgvk);
    let obj = api.get(pod).await?;
    let v = serde_json::to_value(&obj)?;
    let mut pu = PodUsage::default();
    if let Some(conts) = v.pointer("/containers").and_then(|x| x.as_array()) {
        for c in conts {
            let cn = c
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
                .to_string();
            let cm = c
                .pointer("/usage/cpu")
                .and_then(|x| x.as_str())
                .map(crate::model::qty_cpu_m)
                .unwrap_or(0.0);
            let mb = c
                .pointer("/usage/memory")
                .and_then(|x| x.as_str())
                .map(crate::model::qty_mem_b)
                .unwrap_or(0);
            pu.cpu_m += cm;
            pu.mem_b += mb;
            pu.containers.push((cn, cm, mb));
        }
    }
    Ok(pu)
}

/// shares the current namespace scope with the background sampler
pub struct ScopeSync {
    pub all: std::sync::atomic::AtomicBool,
    pub ns: std::sync::Mutex<String>,
}

impl ScopeSync {
    pub fn get(&self) -> Option<String> {
        use std::sync::atomic::Ordering;
        if self.all.load(Ordering::Relaxed) {
            None
        } else {
            Some(self.ns.lock().unwrap().clone())
        }
    }
}

/// aggregate cluster-wide cpu/mem: usage from metrics.k8s.io, capacity from node allocatable.
/// `pod_scope` = Some(ns) limits the pod-metrics fetch to one namespace (cheap on big
/// clusters); None fetches all namespaces.
#[derive(Clone, Debug, Default)]
pub struct ClusterRes {
    /// used millicores (None when metrics-server absent)
    pub cpu_used_m: Option<f64>,
    /// allocatable millicores
    pub cpu_cap_m: f64,
    /// used bytes (None when metrics-server absent)
    pub mem_used: Option<u64>,
    /// allocatable bytes
    pub mem_cap: u64,
    /// per-pod usage samples (empty when metrics-server absent)
    pub pods: std::collections::BTreeMap<String, PodUsage>,
    /// per-node (cpu_m_used?, cpu_cap_m, mem_b_used?, mem_cap_b) keyed by node name
    pub nodes: std::collections::BTreeMap<String, NodeUsage>,
}

#[derive(Clone, Debug, Default)]
pub struct NodeUsage {
    pub cpu_m: Option<f64>,
    pub cpu_cap_m: f64,
    pub mem_b: Option<u64>,
    pub mem_cap_b: u64,
}

type Node = k8s_openapi::api::core::v1::Node;

pub async fn cluster_resources(
    cluster: &Cluster,
    scope: &ScopeSync,
    include_pods: bool,
) -> Result<ClusterRes> {
    let nodes: Api<Node> = Api::all(cluster.client.clone());
    let list = nodes.list(&ListParams::default()).await?;
    let mut res = ClusterRes::default();
    for n in &list.items {
        let nname = n.metadata.name.clone().unwrap_or_default();
        let entry = res.nodes.entry(nname).or_default();
        if let Some(alloc) = n.status.as_ref().and_then(|s| s.allocatable.as_ref()) {
            if let Some(c) = alloc.get("cpu") {
                res.cpu_cap_m += crate::model::qty_cpu_m(&c.0);
                entry.cpu_cap_m += crate::model::qty_cpu_m(&c.0);
            }
            if let Some(m) = alloc.get("memory") {
                res.mem_cap += crate::model::qty_mem_b(&m.0);
                entry.mem_cap_b += crate::model::qty_mem_b(&m.0);
            }
        }
    }
    // usage via the metrics API (metrics-server); absent on bare kind clusters
    let gvk = ApiResource::from_gvk(&GroupVersionKind::gvk("metrics.k8s.io", "v1beta1", "Node"));
    let mapi: Api<DynamicObject> = Api::all_with(cluster.client.clone(), &gvk);
    if let Ok(mlist) = mapi.list(&ListParams::default()).await {
        let mut cpu = 0f64;
        let mut mem = 0u64;
        for item in &mlist.items {
            let v = match serde_json::to_value(item) {
                Ok(v) => v,
                Err(_) => continue,
            };
            cpu += v
                .pointer("/usage/cpu")
                .and_then(|x| x.as_str())
                .map(crate::model::qty_cpu_m)
                .unwrap_or(0.0);
            mem += v
                .pointer("/usage/memory")
                .and_then(|x| x.as_str())
                .map(crate::model::qty_mem_b)
                .unwrap_or(0);
            let nname = v
                .pointer("/metadata/name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if !nname.is_empty() {
                let e = res.nodes.entry(nname).or_default();
                e.cpu_m = Some(cpu);
                e.mem_b = Some(mem);
            }
        }
        res.cpu_used_m = Some(cpu);
        res.mem_used = Some(mem);
    }

    // per-pod usage — scoped so large clusters stay cheap
    if include_pods {
        let pgvk =
            ApiResource::from_gvk(&GroupVersionKind::gvk("metrics.k8s.io", "v1beta1", "Pod"));
        let papi: Api<DynamicObject> = match scope.get() {
            Some(ns) => Api::namespaced_with(cluster.client.clone(), &ns, &pgvk),
            None => Api::all_with(cluster.client.clone(), &pgvk),
        };
        if let Ok(plist) = papi.list(&ListParams::default()).await {
            for item in &plist.items {
                let v = match serde_json::to_value(item) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let ns = v
                    .pointer("/metadata/namespace")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = v
                    .pointer("/metadata/name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let mut pu = PodUsage::default();
                if let Some(conts) = v.pointer("/containers").and_then(|x| x.as_array()) {
                    for c in conts {
                        let cn = c
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("?")
                            .to_string();
                        let cm = c
                            .pointer("/usage/cpu")
                            .and_then(|x| x.as_str())
                            .map(crate::model::qty_cpu_m)
                            .unwrap_or(0.0);
                        let mb = c
                            .pointer("/usage/memory")
                            .and_then(|x| x.as_str())
                            .map(crate::model::qty_mem_b)
                            .unwrap_or(0);
                        pu.cpu_m += cm;
                        pu.mem_b += mb;
                        pu.containers.push((cn, cm, mb));
                    }
                }
                res.pods.insert(format!("{ns}/{name}"), pu);
            }
        }
    }
    Ok(res)
}

pub fn spawn_watch(
    cluster: Arc<Cluster>,
    spec: KindSpec,
    ns: Option<String>,
    selector: Option<String>,
    tx: mpsc::UnboundedSender<Msg>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let api = cluster.dyn_api(&spec, ns.as_deref());
        let mut wcfg = watcher::Config::default();
        if let Some(sel) = selector {
            wcfg = wcfg.labels(&sel);
        }
        let wc = watcher(api, wcfg);
        let mut stream = Box::pin(wc);
        while let Some(res) = stream.next().await {
            let msg = match res {
                Ok(watcher::Event::Apply(o)) => dyn_row(&spec, o),
                Ok(watcher::Event::InitApply(o)) => dyn_row(&spec, o),
                Ok(watcher::Event::Init | watcher::Event::InitDone) => Msg::Reset,
                Ok(watcher::Event::Delete(o)) => Msg::Down(dyn_key(&spec, &o)),
                Err(e) => Msg::Err(format!("watch {}: {e}", spec.plural)),
            };
            if tx.send(msg).is_err() {
                break;
            }
        }
    })
}

use futures::StreamExt;

pub async fn list_namespaces(cluster: &Cluster) -> Result<Vec<String>> {
    let api = cluster.dyn_api(&crate::model::spec_for("ns").unwrap(), None);
    let objs = api.list(&ListParams::default()).await?;
    let mut v: Vec<String> = objs
        .items
        .iter()
        .filter_map(|o| o.metadata.name.clone())
        .collect();
    v.sort();
    Ok(v)
}

pub type CrdInfo = (String, String, String, String, bool);

pub async fn list_crds(cluster: &Cluster) -> Result<Vec<CrdInfo>> {
    let api = cluster.dyn_api(&crate::model::spec_for("crd").unwrap(), None);
    let objs = api.list(&ListParams::default()).await?;
    let mut out = vec![];
    for o in objs.items {
        let kind = str_at(&o.data, "/spec/names/kind");
        let plural = str_at(&o.data, "/spec/names/plural");
        let group = str_at(&o.data, "/spec/group");
        let nsd = str_at(&o.data, "/spec/scope") == "Namespaced";
        if let Some(ver) = o
            .data
            .pointer("/spec/versions")
            .and_then(|v| v.as_array())
            .and_then(|a| {
                a.iter()
                    .find(|v| v.get("storage").and_then(|s| s.as_bool()) == Some(true))
            })
            .and_then(|v| v.get("name"))
            .and_then(|n| n.as_str())
        {
            out.push((plural, group, ver.to_string(), kind, nsd));
        }
    }
    out.sort();
    Ok(out)
}

fn str_at(v: &Value, ptr: &str) -> String {
    v.pointer(ptr)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

pub async fn get_yaml(
    cluster: &Cluster,
    spec: &KindSpec,
    ns: Option<&str>,
    name: &str,
) -> Result<String> {
    let api = cluster.dyn_api(spec, ns);
    let mut obj = api.get(name).await?;
    strip_noise(&mut obj);
    serde_yaml::to_string(&serde_json::to_value(&obj)?).map_err(Into::into)
}

fn strip_noise(obj: &mut DynamicObject) {
    obj.metadata.managed_fields = None;
    if let Value::Object(map) = &mut obj.data {
        map.remove("status");
    }
}

pub async fn describe_obj(
    cluster: &Cluster,
    spec: &KindSpec,
    ns: Option<&str>,
    name: &str,
) -> Result<String> {
    let api = cluster.dyn_api(spec, ns);
    let mut obj = api.get(name).await?;
    strip_noise(&mut obj);
    let uid = obj.metadata.uid.clone().unwrap_or_default();
    let mut text = String::new();
    text.push_str(&format!("Name:       {name}\n"));
    if spec.namespaced {
        text.push_str(&format!("Namespace:  {}\n", ns.unwrap_or("-")));
    }
    text.push_str(&format!("Kind:       {}\n\n", spec.kind));
    if let Ok(v) = serde_json::to_value(&obj) {
        if let Some(owners) = v
            .pointer("/metadata/ownerReferences")
            .and_then(|o| o.as_array())
        {
            for o in owners {
                text.push_str(&format!(
                    "Controlled by: {} ({})\n",
                    o.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
                    o.get("kind").and_then(|k| k.as_str()).unwrap_or("?")
                ));
            }
        }
        text.push_str("\nLabels:\n");
        let mut any = false;
        if let Some(l) = v.pointer("/metadata/labels").and_then(|l| l.as_object()) {
            for (k, val) in l {
                any = true;
                text.push_str(&format!("  {k}={}\n", val.as_str().unwrap_or("")));
            }
        }
        if !any {
            text.pop();
            text.push_str("  <none>\n");
        }
        text.push_str("\nAnnotations:\n");
        let mut any = false;
        if let Some(l) = v
            .pointer("/metadata/annotations")
            .and_then(|l| l.as_object())
        {
            for (k, val) in l {
                any = true;
                text.push_str(&format!(
                    "  {k}: {}\n",
                    serde_json::to_string(val).unwrap_or_default()
                ));
            }
        }
        if !any {
            text.pop();
            text.push_str("  <none>\n");
        }
        if let Some(sec) = kind_summary(spec.kind.as_str(), &v) {
            text.push_str(&sec);
        }
    }
    if !uid.is_empty()
        && let Some(nspc) = ns
    {
        let ev = related_events(cluster, nspc, &uid)
            .await
            .unwrap_or_else(|_| "(events unavailable)".into());
        text.push_str("\nEvents:\n");
        text.push_str(&ev);
    }
    Ok(text)
}

async fn related_events(cluster: &Cluster, ns: &str, uid: &str) -> Result<String> {
    let api = cluster.dyn_api(&crate::model::spec_for("ev").unwrap(), Some(ns));
    let lp = ListParams::default().fields(&format!("involvedObject.uid={uid}"));
    let evs = api.list(&lp).await?;
    if evs.items.is_empty() {
        return Ok("(no events)".into());
    }
    let ev_spec = crate::model::spec_for("ev").unwrap();
    let mut lines = vec![];
    for e in evs.items {
        let v = serde_json::to_value(&e)?;
        let row = extract(&ev_spec, &v);
        lines.push(format!(
            "  {}  {}  {}",
            row.cells[0], row.cells[1], row.cells[5]
        ));
    }
    lines.sort();
    lines.dedup();
    Ok(lines.join("\n"))
}

pub async fn delete_obj(
    cluster: &Cluster,
    spec: &KindSpec,
    ns: Option<&str>,
    name: &str,
    force: bool,
) -> Result<String> {
    let api = cluster.dyn_api(spec, ns);
    let dp = if force {
        DeleteParams::default().grace_period(0)
    } else {
        DeleteParams::default()
    };
    api.delete(name, &dp).await?;
    Ok(if force {
        format!("force-deleted {}/{}", spec.kind, name)
    } else {
        format!("deleted {}/{}", spec.kind, name)
    })
}

pub async fn patch_obj(
    cluster: &Cluster,
    spec: &KindSpec,
    ns: Option<&str>,
    name: &str,
    patch: Value,
) -> Result<String> {
    let api = cluster.dyn_api(spec, ns);
    // RFC-7386 merge patch: merges fields without clobbering siblings (kubectl scale/cordon semantics).
    // Server-Side Apply with a partial body would treat it as FULL desired state and null out the rest.
    let pp = PatchParams::default();
    let _: DynamicObject = api.patch(name, &pp, &Patch::Merge(&patch)).await?;
    Ok(format!("patched {}/{}", spec.kind, name))
}

pub async fn scale(
    cluster: &Cluster,
    spec: &KindSpec,
    ns: &str,
    name: &str,
    replicas: i64,
) -> Result<String> {
    patch_obj(
        cluster,
        spec,
        Some(ns),
        name,
        json!({"spec": {"replicas": replicas}}),
    )
    .await
}

pub async fn rollout_restart(
    cluster: &Cluster,
    spec: &KindSpec,
    ns: &str,
    name: &str,
) -> Result<String> {
    let now = Utc::now().to_rfc3339();
    patch_obj(cluster, spec, Some(ns), name, json!({"spec": {"template": {"metadata": {"annotations": {"kubectl.kubernetes.io/restartedAt": now}}}}})).await
}

pub async fn cordon(cluster: &Cluster, name: &str, unschedulable: bool) -> Result<String> {
    patch_obj(
        cluster,
        &crate::model::spec_for("no").unwrap(),
        None,
        name,
        json!({"spec": {"unschedulable": unschedulable}}),
    )
    .await?;
    Ok(if unschedulable {
        format!("cordoned node {name}")
    } else {
        format!("uncordoned node {name}")
    })
}

pub async fn drain_node(cluster: &Cluster, name: &str) -> Result<String> {
    cordon(cluster, name, true).await?;
    let api: Api<Pod> = Api::all(cluster.client.clone());
    let lp = ListParams::default().fields(&format!("spec.nodeName={name}"));
    let pods = api.list(&lp).await?;
    let mut evicted = 0usize;
    let mut skipped = 0usize;
    for p in pods.items {
        let owner = p
            .metadata
            .owner_references
            .as_ref()
            .and_then(|o| o.first())
            .map(|o| o.kind.clone())
            .unwrap_or_default();
        if p.metadata.namespace.is_none() || owner == "DaemonSet" {
            skipped += 1;
            continue;
        }
        if evict_pod(
            cluster,
            p.metadata.namespace.as_deref().unwrap_or(""),
            p.metadata.name.as_deref().unwrap_or(""),
        )
        .await
        .is_ok()
        {
            evicted += 1;
        } else {
            skipped += 1;
        }
    }
    Ok(format!(
        "drained node {name}: evicted={evicted} skipped={skipped} (cordoned)"
    ))
}

async fn evict_pod(cluster: &Cluster, ns: &str, pod: &str) -> Result<()> {
    let path = format!("/api/v1/namespaces/{ns}/pods/{pod}/eviction");
    let body = json!({
        "apiVersion": "policy/v1",
        "kind": "Eviction",
        "metadata": {"name": pod, "namespace": ns}
    });
    let req = http::Request::builder()
        .method(http::Method::POST)
        .uri(path)
        .body(body.to_string().into_bytes())?;
    cluster.client.request_text(req).await?;
    Ok(())
}

pub async fn trigger_cronjob(cluster: &Cluster, ns: &str, cron: &str) -> Result<String> {
    let cj = cluster
        .dyn_api(&crate::model::spec_for("cj").unwrap(), Some(ns))
        .get(cron)
        .await?;
    let v = serde_json::to_value(&cj)?;
    let template = v
        .pointer("/data/spec/jobTemplate/spec")
        .cloned()
        .or_else(|| v.pointer("/spec/jobTemplate/spec").cloned())
        .ok_or_else(|| anyhow!("cronjob has no jobTemplate.spec"))?;
    let name = format!("{cron}-manual-{}", Utc::now().timestamp());
    let job = json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {"name": name, "namespace": ns},
        "spec": template,
    });
    let req = http::Request::builder()
        .method(http::Method::POST)
        .uri(format!("/apis/batch/v1/namespaces/{ns}/jobs"))
        .body(job.to_string().into_bytes())?;
    cluster.client.request_text(req).await?;
    Ok(format!("triggered job {name} from cronjob/{cron}"))
}

pub async fn set_cronjob_suspend(
    cluster: &Cluster,
    ns: &str,
    cron: &str,
    suspend: bool,
) -> Result<String> {
    patch_obj(
        cluster,
        &crate::model::spec_for("cj").unwrap(),
        Some(ns),
        cron,
        json!({"spec": {"suspend": suspend}}),
    )
    .await?;
    Ok(format!("cronjob/{cron} suspend={suspend}"))
}

pub async fn decode_secret(cluster: &Cluster, ns: &str, name: &str) -> Result<String> {
    let obj = cluster
        .dyn_api(&crate::model::spec_for("sec").unwrap(), Some(ns))
        .get(name)
        .await?;
    let v = serde_json::to_value(&obj)?;
    use base64::Engine;
    let mut out = String::new();
    if let Some(data) = v.pointer("/data").and_then(|d| d.as_object()) {
        for (k, val) in data {
            let dec = val
                .as_str()
                .map(|s| base64::engine::general_purpose::STANDARD.decode(s))
                .transpose()
                .ok()
                .flatten();
            match dec.map(|b| String::from_utf8_lossy(&b).to_string()) {
                Some(t) => out.push_str(&format!("{k}:\n---\n{t}\n---\n")),
                None => out.push_str(&format!("{k}: <binary>\n")),
            }
        }
    }
    Ok(out)
}

pub enum LogMsg {
    Line(String),
    Done(String),
}

/// log fetch window: tail N lines, whole history, or a since-seconds window
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LogWindow {
    Tail(i64),
    Head,
    Since(i64),
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_logs(
    cluster: Arc<Cluster>,
    ns: String,
    pod: String,
    container: Option<String>,
    previous: bool,
    timestamps: bool,
    tail: i64,
    tx: mpsc::UnboundedSender<LogMsg>,
) -> JoinHandle<()> {
    spawn_logs_prefixed(
        cluster,
        ns,
        pod,
        container,
        previous,
        timestamps,
        LogWindow::Tail(tail),
        String::new(),
        tx,
    )
}

/// Same as spawn_logs but prefixes every line (used for aggregated workload logs).
#[allow(clippy::too_many_arguments)]
pub fn spawn_logs_prefixed(
    cluster: Arc<Cluster>,
    ns: String,
    pod: String,
    container: Option<String>,
    previous: bool,
    timestamps: bool,
    window: LogWindow,
    prefix: String,
    tx: mpsc::UnboundedSender<LogMsg>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let api = cluster.pod_api(&ns);
        let lp = match window {
            LogWindow::Tail(n) => LogParams {
                follow: true,
                tail_lines: Some(n),
                timestamps,
                previous,
                container,
                ..Default::default()
            },
            LogWindow::Head => LogParams {
                follow: true,
                timestamps,
                previous,
                container,
                ..Default::default()
            },
            LogWindow::Since(secs) => LogParams {
                follow: true,
                since_seconds: Some(secs),
                timestamps,
                previous,
                container,
                ..Default::default()
            },
        };
        match api.log_stream(&pod, &lp).await {
            Ok(mut reader) => {
                let mut buf = Vec::with_capacity(8192);
                loop {
                    buf.clear();
                    match reader.read_until(b'\n', &mut buf).await {
                        Ok(0) => break,
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&buf);
                            let out = if prefix.is_empty() {
                                line.trim_end_matches('\n').to_string()
                            } else {
                                format!("[{prefix}] {}", line.trim_end_matches('\n'))
                            };
                            if tx.send(LogMsg::Line(out)).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(LogMsg::Done(format!("stream error: {e}")));
                            return;
                        }
                    }
                }
                let _ = tx.send(LogMsg::Done("stream closed".into()));
            }
            Err(e) => {
                let _ = tx.send(LogMsg::Done(format!("logs unavailable: {e}")));
            }
        }
    })
}

/// Attach to a running container's stdin/stdout (no new process).
pub async fn start_attach(
    cluster: Arc<Cluster>,
    ns: String,
    pod: String,
    container: Option<String>,
) -> Result<ExecSession> {
    let api = cluster.pod_api(&ns);
    let ap = kube::core::subresource::AttachParams {
        container,
        stdin: true,
        stdout: true,
        stderr: false,
        tty: true,
        max_stdin_buf_size: None,
        max_stdout_buf_size: None,
        max_stderr_buf_size: None,
    };
    let attached = api.attach(&pod, &ap).await?;
    let mut proc = attached;
    let size_tx = proc.terminal_size();
    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel::<ExecCtl>();
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Result<Vec<u8>, String>>();
    let mut writer = proc.stdin();
    let stdout_handle = proc.stdout();
    let proc = Arc::new(proc);
    let proc_abort = proc.clone();

    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Some(ctl) = ctl_rx.recv().await {
            match ctl {
                ExecCtl::Input(bytes) => {
                    if let Some(w) = writer.as_mut()
                        && (w.write_all(&bytes).await.is_err() || w.flush().await.is_err())
                    {
                        break;
                    }
                }
                ExecCtl::Abort => break,
            }
        }
        proc_abort.abort();
    });

    if let Some(mut so) = stdout_handle {
        let ot = out_tx.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = [0u8; 8192];
            loop {
                match so.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if ot.send(Ok(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    drop(proc);
    Ok(ExecSession {
        ctl_tx,
        out_rx,
        size_tx,
    })
}

pub enum ExecCtl {
    Input(Vec<u8>),
    Abort,
}

pub struct ExecSession {
    pub ctl_tx: mpsc::UnboundedSender<ExecCtl>,
    pub out_rx: mpsc::UnboundedReceiver<Result<Vec<u8>, String>>,
    pub size_tx: Option<futures::channel::mpsc::Sender<kube::api::TerminalSize>>,
}

pub async fn start_exec(
    cluster: Arc<Cluster>,
    ns: String,
    pod: String,
    container: Option<String>,
    command: Vec<String>,
) -> Result<ExecSession> {
    let api = cluster.pod_api(&ns);
    let ap = kube::core::subresource::AttachParams {
        container,
        stdin: true,
        stdout: true,
        // with a TTY, stderr is multiplexed into stdout — kube rejects both
        stderr: false,
        tty: true,
        max_stdin_buf_size: None,
        max_stdout_buf_size: None,
        max_stderr_buf_size: None,
    };
    let attached = api.exec(&pod, command, &ap).await?;
    let mut proc = attached;
    let size_tx = proc.terminal_size();
    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel::<ExecCtl>();
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Result<Vec<u8>, String>>();

    let mut writer = proc.stdin();
    let stdout_handle = proc.stdout();
    let stderr_handle = proc.stderr();
    let proc = Arc::new(proc);
    let proc_abort = proc.clone();

    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Some(ctl) = ctl_rx.recv().await {
            match ctl {
                ExecCtl::Input(bytes) => {
                    if let Some(w) = writer.as_mut()
                        && (w.write_all(&bytes).await.is_err() || w.flush().await.is_err())
                    {
                        break;
                    }
                }
                ExecCtl::Abort => break,
            }
        }
        proc_abort.abort();
    });

    let ot = out_tx.clone();
    if let Some(mut so) = stdout_handle {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = [0u8; 8192];
            loop {
                match so.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if ot.send(Ok(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    let et = out_tx;
    if let Some(mut se) = stderr_handle {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = [0u8; 8192];
            loop {
                match se.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if et.send(Ok(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    drop(proc);
    Ok(ExecSession {
        ctl_tx,
        out_rx,
        size_tx,
    })
}

static PF_SEQ: AtomicU64 = AtomicU64::new(1);
fn next_pf_id() -> u64 {
    PF_SEQ.fetch_add(1, Ordering::Relaxed)
}

type PfTask = (u64, JoinHandle<()>);

pub struct PfEntry {
    pub id: u64,
    pub target: String,
    pub local_port: u16,
    pub remote_port: u16,
    pub handle: JoinHandle<()>,
    pub stop: Arc<AtomicBool>,
    pub conns: Arc<AtomicUsize>,
    /// active per-connection tasks — aborted on stop so clients disconnect immediately
    pub conns_tasks: Arc<std::sync::Mutex<Vec<PfTask>>>,
}

static CONN_SEQ: AtomicU64 = AtomicU64::new(1);
fn next_conn_id() -> u64 {
    CONN_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// One API portforward per inbound TCP connection (mirrors kubectl semantics).
pub async fn port_forward(
    cluster: Arc<Cluster>,
    ns: String,
    target: String,
    remote_port: u16,
    bind_addr: String,
) -> Result<PfEntry> {
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    let local_port = listener.local_addr()?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let conns = Arc::new(AtomicUsize::new(0));
    let stop2 = stop.clone();
    let conns2 = conns.clone();
    let ns_inner = ns.clone();
    let target_inner = target.clone();
    let tasks_reg: Arc<std::sync::Mutex<Vec<PfTask>>> = Arc::new(std::sync::Mutex::new(vec![]));

    let handle = tokio::spawn(async move {
        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            let accepted = tokio::select! {
                a = listener.accept() => a,
                _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {
                    continue;
                }
            };
            let Ok((mut sock, _)) = accepted else { break };
            conns2.fetch_add(1, Ordering::Relaxed);
            let cl = cluster.client.clone();
            let ns2 = ns_inner.clone();
            let tgt = target_inner.clone();
            let rp = remote_port;
            let reg = tasks_reg.clone();
            let cid = next_conn_id();
            let t = tokio::spawn(async move {
                let api: Api<Pod> = Api::namespaced(cl, &ns2);
                if let Ok(mut pf) = api.portforward(&tgt, &[rp]).await {
                    if let Some(mut up) = pf.take_stream(rp) {
                        let _ = tokio::io::copy_bidirectional(&mut sock, &mut up).await;
                    }
                    pf.abort();
                }
            });
            if let Ok(mut m) = reg.lock() {
                m.retain(|(_id, h)| !h.is_finished());
                m.push((cid, t));
            }
        }
    });
    Ok(PfEntry {
        id: next_pf_id(),
        target: format!("{ns}/{target}:{remote_port}"),
        local_port,
        remote_port,
        handle,
        stop,
        conns,
        conns_tasks: Arc::new(std::sync::Mutex::new(vec![])),
    })
}

fn dyn_row(spec: &KindSpec, o: DynamicObject) -> Msg {
    match serde_json::to_value(&o) {
        Ok(v) => Msg::Up(extract(spec, &v)),
        Err(e) => Msg::Err(format!("decode: {e}")),
    }
}

fn dyn_key(spec: &KindSpec, o: &DynamicObject) -> String {
    let name = o.metadata.name.clone().unwrap_or_default();
    if spec.namespaced
        && let Some(ns) = &o.metadata.namespace
        && !ns.is_empty()
    {
        return format!("{ns}/{name}");
    }
    name
}

#[derive(Clone, Debug)]
pub struct HelmRelease {
    pub name: String,
    pub namespace: String,
    pub revision: i64,
    pub status: String,
    pub chart: String,
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .ok()
}

fn gunzip(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = vec![];
    flate2::read::GzDecoder::new(data)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

/// List helm releases by reading sh.helm.release.v1.* secrets (kubectl-free parity).
pub async fn helm_releases(cluster: &Cluster, ns: Option<&str>) -> Result<Vec<HelmRelease>> {
    let api = cluster.dyn_api(&crate::model::spec_for("sec").unwrap(), ns);
    let lp = ListParams::default().labels("owner=helm");
    let secrets = api.list(&lp).await?.items;
    let mut best: std::collections::HashMap<(String, String), (i64, HelmRelease)> =
        Default::default();
    for sec in secrets {
        let name = sec.metadata.name.clone().unwrap_or_default();
        let ns2 = sec.metadata.namespace.clone().unwrap_or_default();
        let parts: Vec<&str> = name.split('.').collect();
        // sh.helm.release.v1.<release>.<revision>
        if parts.len() < 6
            || parts[0] != "sh"
            || parts[1] != "helm"
            || parts[2] != "release"
            || parts[3] != "v1"
        {
            continue;
        }
        let rel = parts[4].to_string();
        let rev: i64 = parts[5].trim_start_matches('v').parse().unwrap_or(0);
        // DynamicObject wraps the secret body: payload lives at data.data.{release|status}
        let data_b64 = sec
            .data
            .pointer("/data/release")
            .or_else(|| sec.data.pointer("/data/status"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let Some(b64) = data_b64 else { continue };
        let Some(status) = decode_release_payload(&b64) else {
            continue;
        };
        let rel_name = status
            .pointer("/name")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| rel.clone());
        let hr = HelmRelease {
            name: rel_name,
            namespace: ns2.clone(),
            revision: rev,
            status: status
                .pointer("/info/status")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string(),
            chart: status
                .pointer("/chart/metadata/name")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string(),
        };
        let key = (rel, ns2);
        best.entry(key)
            .and_modify(|e| {
                if rev > e.0 {
                    *e = (rev, hr.clone())
                }
            })
            .or_insert((rev, hr));
    }
    let mut v: Vec<HelmRelease> = best.into_values().map(|(_, h)| h).collect();
    v.sort_by(|a, b| natural_sort(&a.namespace, &b.namespace).then(natural_sort(&a.name, &b.name)));
    Ok(v)
}

/// Decode a helm release secret payload: base64 [ base64 [ gzip(json) ] ] variants.
pub fn decode_release_payload(b64: &str) -> Option<Value> {
    let mut decoded = b64_decode(b64)?;
    if decoded.first() != Some(&0x1f)
        && let Ok(txt) = String::from_utf8(decoded.clone())
        && let Some(inner) = b64_decode(txt.trim())
    {
        decoded = inner;
    }
    let json_bytes = if decoded.first() == Some(&0x1f) {
        gunzip(&decoded).unwrap_or(decoded)
    } else {
        decoded
    };
    serde_json::from_slice(&json_bytes).ok()
}

fn natural_sort(a: &str, b: &str) -> std::cmp::Ordering {
    crate::app::natural_compare(a, b)
}

/// one revision of a helm release history
#[derive(Clone, Debug)]
pub struct HelmRev {
    pub revision: i64,
    pub status: String,
    pub chart: String,
    pub chart_ver: String,
    pub updated: String,
    /// decoded values (config) as YAML text
    pub values_yaml: String,
}

/// full revision history for one release (native: decoded from release secrets)
pub async fn helm_history(cluster: &Cluster, ns: &str, release: &str) -> Result<Vec<HelmRev>> {
    let api = cluster.dyn_api(&crate::model::spec_for("sec").unwrap(), Some(ns));
    let lp = ListParams::default().labels(&format!("owner=helm,name={release}"));
    let secrets = api.list(&lp).await?.items;
    let mut out: Vec<HelmRev> = vec![];
    for sec in secrets {
        let sname = sec.metadata.name.clone().unwrap_or_default();
        let parts: Vec<&str> = sname.split('.').collect();
        if parts.len() < 6 || parts[3] != "v1" || parts[4] != release {
            continue;
        }
        let rev: i64 = parts[5].trim_start_matches('v').parse().unwrap_or(0);
        let data_b64 = sec
            .data
            .pointer("/data/release")
            .or_else(|| sec.data.pointer("/data/status"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let Some(b64) = data_b64 else { continue };
        let Some(status) = decode_release_payload(&b64) else {
            continue;
        };
        // values live at "config" in the payload
        let values_yaml = status
            .get("config")
            .and_then(|c| serde_yaml::to_string(c).ok())
            .unwrap_or_else(|| "# (no values stored)\n".into());
        out.push(HelmRev {
            revision: rev,
            status: status
                .pointer("/info/status")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .into(),
            chart: status
                .pointer("/chart/metadata/name")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .into(),
            chart_ver: status
                .pointer("/chart/metadata/version")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .into(),
            updated: str_at(&status, "/info/last_deployed"),
            values_yaml,
        });
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.revision));
    Ok(out)
}

/// rollback via the helm CLI when available (native re-render is a mini-helm engine —
/// deliberately delegated; error is actionable when the binary is missing)
pub async fn helm_rollback(
    ctx_name: &str,
    ns: &str,
    release: &str,
    revision: i64,
) -> Result<String> {
    let bin = which_bin("helm")
        .ok_or_else(|| anyhow!("rollback needs the helm CLI installed (brew install helm); history/values work without it"))?;
    let out = tokio::process::Command::new(bin)
        .args([
            "rollback",
            release,
            &revision.to_string(),
            "-n",
            ns,
            "--kube-context",
            ctx_name,
        ])
        .output()
        .await?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(anyhow!(
            "helm rollback failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

pub fn which_bin(bin: &str) -> Option<std::path::PathBuf> {
    std::env::var("PATH")
        .ok()?
        .split(':')
        .map(std::path::PathBuf::from)
        .map(|d| d.join(bin))
        .find(|p| p.exists())
}

// ---- RBAC / permission handling ----

/// outcome of a permission check
#[derive(Clone, Debug)]
pub struct PermCheck {
    pub allowed: bool,
    /// human reason from the API when denied (may be empty)
    pub reason: String,
}

/// SelfSubjectAccessReview: "may *I* do verb on resource in ns?".
/// Returns None when the review endpoint itself is unavailable — callers proceed anyway.
pub async fn can_i(
    cluster: &Cluster,
    verb: &str,
    group: &str,
    resource: &str,
    ns: Option<&str>,
) -> Option<PermCheck> {
    let payload = json!({
        "apiVersion": "authorization.k8s.io/v1",
        "kind": "SelfSubjectAccessReview",
        "spec": {"resourceAttributes": {
            "verb": verb,
            "group": group,
            "resource": resource,
            "namespace": ns.unwrap_or(""),
        }}
    });
    let gvk = ApiResource::from_gvk(&GroupVersionKind::gvk(
        "authorization.k8s.io",
        "v1",
        "SelfSubjectAccessReview",
    ));
    let api: Api<DynamicObject> = Api::all_with(cluster.client.clone(), &gvk);
    let obj: DynamicObject = serde_json::from_value(payload).ok()?;
    let created = api
        .create(&kube::api::PostParams::default(), &obj)
        .await
        .ok()?;
    let v = serde_json::to_value(&created).ok()?;
    let allowed = v.pointer("/status/allowed").and_then(|x| x.as_bool())?;
    let reason = v
        .pointer("/status/reason")
        .or_else(|| v.pointer("/status/denied"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(PermCheck { allowed, reason })
}

/// classify any kube error into a friendly one-liner.
/// Recognises the canonical apiserver Forbidden shape:
///   `<res> "name" is forbidden: User "u" cannot <verb> resource "<res>" in API group "g" in the namespace "ns"`
/// true when an error string indicates expired/invalid credentials (NOT plain RBAC denial)
pub fn is_auth_expired(e: &str) -> bool {
    let l = e.to_lowercase();
    if l.contains("forbidden") || l.contains("403") {
        return false;
    }
    [
        "token",
        "expired",
        "unauthorized",
        "401",
        "credential",
        "authentication failed",
    ]
    .iter()
    .any(|k| l.contains(k))
}

pub fn classify_err(e: &str) -> Option<String> {
    let lower = e.to_lowercase();
    if lower.contains("forbidden")
        || lower.contains("403")
        || (lower.contains("rbac") && lower.contains("denied"))
    {
        return classify_rbac(e);
    }
    if lower.contains("unauthorized") || lower.contains("401") {
        return Some("Unauthorized — credentials expired; ':ctx' to reconnect".into());
    }
    if lower.contains("timeout")
        || lower.contains("connection refused")
        || lower.ends_with("eof")
        || lower.contains("eof\n")
    {
        return Some("cluster unreachable — is the VPN/docker up? ':ctx' or 'r' to retry".into());
    }
    None
}

fn classify_rbac(e: &str) -> Option<String> {
    let mut verb = extract_word_after(e, &["cannot ", "can not "])
        .unwrap_or("?")
        .trim()
        .to_string();
    // impersonation-wrapped messages produce noisy tokens — fall back to generic wording
    if verb.contains(':') || verb.contains('(') || verb.len() > 24 {
        verb.clear();
    }
    let clean = |s: &str| -> String {
        s.trim_matches(|c: char| !c.is_alphanumeric() && c != '-')
            .to_string()
    };
    let resource = clean(extract_quoted_after(e, &["resource "]).unwrap_or("?"));
    let ns = e
        .match_indices("in the namespace")
        .next()
        .and_then(|(i, _)| e[i..].split_whitespace().nth(3).map(&clean))
        .unwrap_or_else(|| "-".into());
    let user_raw = extract_quoted_after(e, &["user "])
        .unwrap_or("")
        .trim_matches('"')
        .to_string();
    let user = clean(&user_raw);
    let head = if verb.is_empty() {
        format!("RBAC denied on {resource} in ns {ns}")
    } else {
        format!("RBAC denied: cannot {verb} {resource} in ns {ns}")
    };
    let mut s = head;
    if !user.is_empty() {
        let short = user.rsplit('/').next().unwrap_or(&user);
        s.push_str(&format!(" · as {short}"));
    }
    s.push_str(" — ask your admin for a Role binding");
    Some(s)
}

fn extract_word_after<'a>(s: &'a str, markers: &[&str]) -> Option<&'a str> {
    let low = s.to_lowercase();
    for m in markers {
        if let Some(i) = low.find(m) {
            let rest = &s[i + m.len()..];
            let w: &str = rest.split_whitespace().next()?;
            return Some(w);
        }
    }
    None
}

fn extract_quoted_after<'a>(s: &'a str, markers: &[&str]) -> Option<&'a str> {
    let low = s.to_lowercase();
    for m in markers {
        if let Some(i) = low.find(m) {
            let rest = &s[i + m.len()..];
            let a = rest.find('"')?;
            let b = rest[a + 1..].find('"').map(|j| j + a + 1)?;
            return Some(&rest[a + 1..b]);
        }
    }
    None
}

// ---- UsedBy reference scanner (k9s `U` parity) ----

#[derive(Clone, Debug)]
pub struct RefHit {
    pub kind: &'static str,
    pub ns: String,
    pub name: String,
    /// where the reference was found, e.g. "env.valueFrom.secretKeyRef"
    pub via: String,
}

const REF_KINDS: &[(&str, &str)] = &[
    ("Pod", "pods"),
    ("Deployment", "deployments"),
    ("StatefulSet", "statefulsets"),
    ("DaemonSet", "daemonsets"),
    ("Job", "jobs"),
    ("CronJob", "cronjobs"),
];

/// scan workloads (+ pods / SAs / ingress TLS) in scope for references to a configmap/secret.
/// On-demand only — nothing here runs in the background.
pub async fn used_by(
    cluster: &Cluster,
    target: &str,
    is_secret: bool,
    ns: Option<&str>,
) -> Result<Vec<RefHit>> {
    let mut hits: Vec<RefHit> = Vec::new();
    let lp = ListParams::default();
    for (kind, _plural) in REF_KINDS {
        let spec = crate::model::spec_for(match *kind {
            "Deployment" => "deploy",
            "StatefulSet" => "sts",
            "DaemonSet" => "ds",
            "Job" => "job",
            "CronJob" => "cj",
            _ => "po",
        })
        .unwrap();
        let api = cluster.dyn_api(&spec, ns);
        let Ok(list) = api.list(&lp).await else {
            continue;
        };
        for obj in list.items {
            let v = match serde_json::to_value(&obj) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let name = v
                .pointer("/metadata/name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let ons = v
                .pointer("/metadata/namespace")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let mut paths: Vec<String> = vec![];
            scan_value_for_ref(&v, target, is_secret, &mut vec![], &mut paths);
            if !paths.is_empty() {
                hits.push(RefHit {
                    kind,
                    ns: ons,
                    name,
                    via: dedup_via(paths),
                });
            }
        }
    }
    // ServiceAccounts list secrets by name
    {
        let spec = crate::model::spec_for("sa").unwrap();
        let api = cluster.dyn_api(&spec, ns);
        if let Ok(list) = api.list(&lp).await {
            for obj in list.items {
                let v = match serde_json::to_value(&obj) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let uses = v
                    .pointer("/secrets")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.get("name").and_then(|n| n.as_str()))
                            .any(|n| n == target)
                    })
                    .unwrap_or(false);
                if is_secret && uses {
                    hits.push(RefHit {
                        kind: "ServiceAccount",
                        ns: v
                            .pointer("/metadata/namespace")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .into(),
                        name: v
                            .pointer("/metadata/name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .into(),
                        via: "secrets[]".into(),
                    });
                }
            }
        }
    }
    // Ingress TLS references secrets by name
    if is_secret {
        let spec = crate::model::spec_for("ing").unwrap();
        let api = cluster.dyn_api(&spec, ns);
        if let Ok(list) = api.list(&lp).await {
            for obj in list.items {
                let v = match serde_json::to_value(obj) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let tls = v
                    .pointer("/spec/tls")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| t.get("secretName").and_then(|n| n.as_str()))
                            .any(|n| n == target)
                    })
                    .unwrap_or(false);
                if tls {
                    hits.push(RefHit {
                        kind: "Ingress",
                        ns: v
                            .pointer("/metadata/namespace")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .into(),
                        name: v
                            .pointer("/metadata/name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .into(),
                        via: "spec.tls[].secretName".into(),
                    });
                }
            }
        }
    }
    hits.sort_by(|a, b| natural_sort(a.kind, b.kind).then(natural_sort(&a.name, &b.name)));
    hits.dedup_by(|a, b| a.kind == b.kind && a.name == b.name && a.ns == b.ns);
    Ok(hits)
}

/// recursive JSON walk: a string leaf equal to `target` counts as a reference only
/// when it sits under reference-carrying keys (volumes/env/envFrom/imagePullSecrets/
/// projected sources/ingress tls)
fn scan_value_for_ref(
    v: &Value,
    target: &str,
    is_secret: bool,
    stack: &mut Vec<String>,
    out: &mut Vec<String>,
) {
    scan_walk(v, target, is_secret, "", "", stack, out);
}

const REF_PARENT_KEYS: &[&str] = &[
    "secretName",
    "secretKeyRef",
    "secretRef",
    "secret",
    "configMap",
    "configMapKeyRef",
    "configMapRef",
];

fn scan_walk(
    v: &Value,
    target: &str,
    is_secret: bool,
    parent_key: &str,
    grandparent_key: &str,
    stack: &mut Vec<String>,
    out: &mut Vec<String>,
) {
    match v {
        Value::Object(map) => {
            for (k2, val) in map {
                stack.push(k2.clone());
                scan_walk(val, target, is_secret, k2.as_str(), parent_key, stack, out);
                stack.pop();
            }
        }
        Value::Array(arr) => {
            for item in arr {
                scan_walk(
                    item,
                    target,
                    is_secret,
                    parent_key,
                    grandparent_key,
                    stack,
                    out,
                );
            }
        }
        Value::String(s) if s == target => {
            let hit = parent_key == "secretName"
                || (parent_key == "name"
                    && (REF_PARENT_KEYS.contains(&grandparent_key)
                        || (is_secret && grandparent_key == "imagePullSecrets")));
            if hit && stack.len() >= 2 {
                let start = stack.len().saturating_sub(4);
                let path = stack[start..].join(".");
                if !out.contains(&path) {
                    out.push(path);
                }
            }
        }
        _ => {}
    }
}

fn dedup_via(paths: Vec<String>) -> String {
    let mut uniq: Vec<String> = vec![];
    for p in paths {
        if !uniq.contains(&p) {
            uniq.push(p);
        }
    }
    uniq.join(", ")
}

fn kind_summary(kind: &str, v: &Value) -> Option<String> {
    let mut t = String::new();
    match kind {
        "Pod" => {
            t.push_str("\nContainers:\n");
            for c in v
                .pointer("/spec/containers")
                .and_then(|c| c.as_array())
                .unwrap_or(&vec![])
            {
                let name = c.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                let img = c.get("image").and_then(|n| n.as_str()).unwrap_or("?");
                t.push_str(&format!("  {name}: {img}\n"));
                if let Some(ports) = c.get("ports").and_then(|p| p.as_array()) {
                    for pp in ports {
                        t.push_str(&format!(
                            "    port {}: {}\n",
                            pp.get("containerPort")
                                .map(|x| x.to_string())
                                .unwrap_or_default(),
                            pp.get("name").and_then(|n| n.as_str()).unwrap_or("tcp")
                        ));
                    }
                }
            }
            if let Some(cs) = v
                .pointer("/status/containerStatuses")
                .and_then(|c| c.as_array())
            {
                t.push_str("\nStatus:\n");
                for c in cs {
                    t.push_str(&format!(
                        "  {}: ready={} restarts={}\n",
                        c.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
                        c.get("ready").and_then(|r| r.as_bool()).unwrap_or(false),
                        c.get("restartCount")
                            .map(|x| x.to_string())
                            .unwrap_or_default()
                    ));
                }
            }
        }
        "Node" => {
            for (label, ptr) in [
                ("kubelet", "/status/nodeInfo/kubeletVersion"),
                ("osImage", "/status/nodeInfo/osImage"),
                ("providerID", "/spec/providerID"),
            ] {
                let val = v.pointer(ptr).and_then(|x| x.as_str()).unwrap_or("-");
                t.push_str(&format!("\n{label}: {val}\n"));
            }
            if let Some(tw) = v.pointer("/spec/taints").and_then(|x| x.as_array()) {
                t.push_str("\nTaints:\n");
                for tt in tw {
                    t.push_str(&format!(
                        "  {}={}:{}\n",
                        tt.get("key").and_then(|x| x.as_str()).unwrap_or("?"),
                        tt.get("value").and_then(|x| x.as_str()).unwrap_or(""),
                        tt.get("effect").and_then(|x| x.as_str()).unwrap_or("?")
                    ));
                }
            }
            if let Some(cap) = v.pointer("/status/capacity").and_then(|x| x.as_object()) {
                t.push_str("\nCapacity:\n");
                for (k, val) in cap {
                    t.push_str(&format!("  {k}: {}\n", val.as_str().unwrap_or("?")));
                }
            }
        }
        "Service" => {
            if let Some(sel) = v.pointer("/spec/selector").and_then(|x| x.as_object()) {
                t.push_str("\nSelector:\n");
                for (k, val) in sel {
                    t.push_str(&format!("  {k}={}\n", val.as_str().unwrap_or("")));
                }
            }
        }
        _ => {
            if let Some(spec) = v.get("data").is_none().then(|| v.get("spec")).flatten()
                && !spec.is_null()
                && let Ok(js) = serde_json::to_string_pretty(spec)
            {
                let truncated: String = js.lines().take(40).collect::<Vec<_>>().join("\n");
                t.push_str(&format!("\nSpec (truncated):\n{truncated}\n"));
            }
        }
    }
    (!t.is_empty()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helm_decode_single_b64_gzip() {
        use std::io::Write as _;
        let json = br#"{"name":"web","info":{"status":"deployed"}}"#;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(json).unwrap();
        let compressed = gz.finish().unwrap();
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&compressed);
        let v = decode_release_payload(&b64).unwrap();
        assert_eq!(v.pointer("/name").and_then(|x| x.as_str()), Some("web"));
        assert_eq!(
            v.pointer("/info/status").and_then(|x| x.as_str()),
            Some("deployed")
        );
    }

    #[test]
    fn helm_decode_double_b64() {
        use std::io::Write as _;
        let json = br#"{"name":"double"}"#;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(json).unwrap();
        let inner = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            gz.finish().unwrap(),
        );
        let outer =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, inner.as_bytes());
        let v = decode_release_payload(&outer).unwrap();
        assert_eq!(v.pointer("/name").and_then(|x| x.as_str()), Some("double"));
    }

    #[test]
    fn helm_decode_rejects_junk() {
        assert!(decode_release_payload("!!!not-base64!!!").is_none());
    }

    #[test]
    fn watch_msg_variants_debug() {
        let _ = format!("{:?}", Msg::Reset);
        let _ = format!("{:?}", Msg::Status("hi".into()));
        let _ = format!(
            "{:?}",
            Msg::Pane {
                title: "t".into(),
                lines: vec![],
                wrap: false
            }
        );
    }
}

#[cfg(test)]
mod usedby_tests {
    use super::*;

    #[test]
    fn secret_ref_scan() {
        let pod: Value = serde_json::json!({
            "metadata": {"name": "app"},
            "spec": {
                "containers": [{
                    "name": "main",
                    "env": [{"name": "K", "valueFrom": {"secretKeyRef": {"name": "mysecret"}}}]
                }],
                "volumes": [{"name": "v", "secret": {"secretName": "other"}}],
                "imagePullSecrets": [{"name": "mysecret"}]
            }
        });
        let mut out = vec![];
        scan_value_for_ref(&pod, "mysecret", true, &mut vec![], &mut out);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out.iter().any(|p| p.contains("secretKeyRef")));
        assert!(out.iter().any(|p| p.contains("imagePullSecrets")));
        // non-ref "name" must NOT match
        let decoy: Value = serde_json::json!({"spec": {"containers": [{"name": "mysecret"}]}});
        let mut out2 = vec![];
        scan_value_for_ref(&decoy, "mysecret", true, &mut vec![], &mut out2);
        assert!(out2.is_empty());
    }

    #[test]
    fn forbidden_classifier() {
        let e = "deployments.apps \"web\" is forbidden: User \"arn:aws:iam::1:user/bob\" cannot patch resource \"deployments\" in API group \"apps\" in the namespace \"demo\"";
        let m = classify_err(e).unwrap();
        assert!(
            m.starts_with("RBAC denied: cannot patch deployments in ns demo"),
            "{m}"
        );
        assert!(m.contains("as bob"));
        assert!(classify_err("pod not found").is_none());
        assert!(classify_err("Unauthorized: 401").unwrap().contains(":ctx"));
    }

    #[test]
    fn cm_ref_scan_and_tls() {
        let pod: Value = serde_json::json!({
            "spec": {"containers": [{"envFrom": [{"configMapRef": {"name": "mycm"}}]}]}
        });
        let mut out = vec![];
        scan_value_for_ref(&pod, "mycm", false, &mut vec![], &mut out);
        assert_eq!(out.len(), 1);
        let ing: Value = serde_json::json!({"spec":{"tls":[{"secretName":"tlscert"}]}});
        let mut out2 = vec![];
        scan_value_for_ref(&ing, "tlscert", true, &mut vec![], &mut out2);
        assert_eq!(out2.len(), 1);
    }
}

// ---- container env viewer ----

/// on-demand env inspection of every container in a pod
pub async fn pod_env(cluster: &Cluster, ns: &str, pod: &str) -> Result<Vec<(String, Vec<String>)>> {
    let api = cluster.pod_api(ns);
    let p = api.get(pod).await?;
    let mut out = vec![];
    let conts = p.spec.map(|s| s.containers).unwrap_or_default();
    for c in &conts {
        {
            let mut lines: Vec<String> = vec![];
            if let Some(envs) = c.env.as_ref() {
                for e in envs {
                    let name = e.name.clone();
                    let val = if let Some(v) = &e.value {
                        format!("{name}={v}")
                    } else if let Some(vf) = &e.value_from {
                        if let Some(s) = &vf.secret_key_ref {
                            format!("{name} ← secret/{}/{}", s.name, s.key)
                        } else if let Some(c2) = &vf.config_map_key_ref {
                            format!("{name} ← configmap/{}/{}", c2.name, c2.key)
                        } else if let Some(f) = &vf.field_ref {
                            format!("{name} ← field:{}", f.field_path)
                        } else {
                            format!("{name}=?(complex)")
                        }
                    } else {
                        format!("{name}=(unset)")
                    };
                    lines.push(val);
                }
            }
            if let Some(ef) = c.env_from.as_ref() {
                for e in ef {
                    if let Some(cm) = &e.config_map_ref {
                        lines.push(format!("envFrom ← configmap/{}", cm.name));
                    }
                    if let Some(s) = &e.secret_ref {
                        lines.push(format!("envFrom ← secret/{}", s.name));
                    }
                }
            }
            out.push((c.name.clone(), lines));
        }
    }
    Ok(out)
}

// ---- multi-doc YAML apply (:dir / file apply) ----

/// kinds that are never namespaced
fn is_cluster_scoped(kind: &str) -> bool {
    matches!(
        kind,
        "Namespace"
            | "Node"
            | "PersistentVolume"
            | "ClusterRole"
            | "ClusterRoleBinding"
            | "CustomResourceDefinition"
            | "StorageClass"
            | "IngressClass"
            | "PriorityClass"
            | "CSIDriver"
            | "CSINode"
            | "RuntimeClass"
            | "APIService"
            | "ValidatingWebhookConfiguration"
            | "MutatingWebhookConfiguration"
            | "CertificateSigningRequest"
            | "NodeMetrics"
            | "VolumeAttachment"
            | "Lease"
            | "SelfSubjectAccessReview"
    ) || crate::model::all_kinds_cluster_scoped().contains(&kind)
}

/// apply every document in a YAML file (create-or-patch each).
/// returns human summary "N applied (a created, b patched)".
pub async fn apply_file(cluster: &Cluster, default_ns: &str, path: &str) -> Result<String> {
    use serde::Deserialize as _;
    let txt = std::fs::read_to_string(path)?;
    let mut created = 0usize;
    let mut patched = 0usize;
    let mut skipped = 0usize;
    let mut errs: Vec<String> = vec![];
    for doc in serde_yaml::Deserializer::from_str(&txt) {
        let v = match Value::deserialize(doc) {
            Ok(v) => v,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if v.is_null() {
            skipped += 1;
            continue;
        }
        let kind = v
            .get("kind")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let api_version = v
            .get("apiVersion")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let name = v
            .pointer("/metadata/name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if kind.is_empty() || name.is_empty() || api_version.is_empty() {
            skipped += 1;
            continue;
        }
        let (group, version) = match api_version.split_once('/') {
            Some((g, ver)) => (g.to_string(), ver.to_string()),
            None => (String::new(), api_version.clone()),
        };
        let namespaced = !is_cluster_scoped(&kind);
        let ns_owned = v
            .pointer("/metadata/namespace")
            .and_then(|x| x.as_str())
            .map(String::from)
            .unwrap_or_else(|| default_ns.to_string());
        let gvk = ApiResource::from_gvk(&GroupVersionKind::gvk(&group, &version, &kind));
        let api: Api<DynamicObject> = if namespaced {
            Api::namespaced_with(cluster.client.clone(), &ns_owned, &gvk)
        } else {
            Api::all_with(cluster.client.clone(), &gvk)
        };
        let mut obj: DynamicObject =
            serde_json::from_value(v.clone()).map_err(|e| anyhow!("decode {kind}/{name}: {e}"))?;
        obj.metadata.namespace = if namespaced {
            Some(ns_owned.clone())
        } else {
            None
        };
        // strip server-managed fields before write
        if let Some(md) = obj.data.get_mut("metadata").and_then(|m| m.as_object_mut()) {
            for k in [
                "resourceVersion",
                "uid",
                "creationTimestamp",
                "managedFields",
                "generation",
                "selfLink",
            ] {
                md.remove(k);
            }
        }
        let scope_ns = if namespaced {
            Some(ns_owned.as_str())
        } else {
            None
        };
        let get_api = cluster.dyn_api_kind(&group, &version, &kind, scope_ns);
        match get_api.get(&name).await {
            Ok(_) => {
                let pp = PatchParams::default();
                let patch = Patch::Merge(&obj.data);
                match api.patch(&name, &pp, &patch).await {
                    Ok(_) => patched += 1,
                    Err(e) => errs.push(format!("{kind}/{name}: {e}")),
                }
            }
            Err(_) => match api.create(&PostParams::default(), &obj).await {
                Ok(_) => created += 1,
                Err(e) => errs.push(format!("{kind}/{name}: {e}")),
            },
        }
    }
    let mut s = format!(
        "{} applied ({created} created, {patched} patched)",
        created + patched
    );
    if skipped > 0 {
        s.push_str(&format!(" · {skipped} skipped"));
    }
    if !errs.is_empty() {
        s.push_str(&format!(" · !{}", errs.join("; ")));
    }
    Ok(s)
}

/// ephemeral privileged nsenter pod for node shells
pub async fn create_node_shell_pod(
    cluster: &Cluster,
    node: &str,
    ns: &str,
    name: &str,
    image: &str,
) -> Result<()> {
    let payload = json!({
        "apiVersion": "v1", "kind": "Pod",
        "metadata": {"name": name, "namespace": ns, "labels": {"k9x.io/role": "node-shell"}},
        "spec": {
            "hostPID": true,
            "nodeName": node,
            "restartPolicy": "Never",
            "tolerations": [
                {"key": "node-role.kubernetes.io/control-plane", "operator": "Exists", "effect": "NoSchedule"},
                {"key": "node-role.kubernetes.io/master", "operator": "Exists", "effect": "NoSchedule"}
            ],
            "containers": [{
                "name": "shell",
                "image": image,
                "securityContext": {"privileged": true},
                "command": ["sh", "-c", "sleep 3600"]
            }]
        }
    });
    let gvk = ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Pod"));
    let api: Api<DynamicObject> = Api::namespaced_with(cluster.client.clone(), ns, &gvk);
    let obj: DynamicObject = serde_json::from_value(payload)?;
    api.create(&PostParams::default(), &obj).await?;
    Ok(())
}

/// block until the pod reports Running (bounded); used by the node-shell flow
pub async fn wait_pod_running(cluster: &Cluster, ns: &str, name: &str, secs: u64) -> Result<()> {
    let gvk = ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Pod"));
    let api: Api<DynamicObject> = Api::namespaced_with(cluster.client.clone(), ns, &gvk);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(anyhow!("pod {name} not Running after {secs}s"));
        }
        let obj = api.get(name).await?;
        let phase = serde_json::to_value(&obj)
            .ok()
            .and_then(|v| {
                v.pointer("/status/phase")
                    .and_then(|x| x.as_str())
                    .map(String::from)
            })
            .unwrap_or_default();
        match phase.as_str() {
            "Running" => return Ok(()),
            "Failed" | "Unknown" => return Err(anyhow!("pod {name} entered {phase}")),
            _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
        }
    }
}

/// fire-and-forget delete (used to clean up node-shell pods)
pub async fn delete_pod_quiet(cluster: Arc<Cluster>, ns: String, name: String) {
    let gvk = ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Pod"));
    let api: Api<DynamicObject> = Api::namespaced_with(cluster.client.clone(), &ns, &gvk);
    let _ = api.delete(&name, &DeleteParams::default()).await;
}

/// batched counts for the pulse dashboard: (label, total, healthy) in PULSE_CARDS order.
/// Called only while the pulse view is open — never polled in the background.
pub async fn pulse_counts(
    cluster: &Cluster,
    ns: Option<&str>,
) -> Result<Vec<(&'static str, usize, usize)>> {
    const WORKLOADS: &[&str] = &["Pods", "Deployments", "Statefulsets", "Daemonsets", "Jobs"];
    let mut out = Vec::with_capacity(crate::app::PULSE_CARDS.len());
    for (label, alias) in crate::app::PULSE_CARDS {
        let spec = crate::model::spec_for(alias).ok_or_else(|| anyhow!("no spec for {alias}"))?;
        let api = cluster.dyn_api(&spec, if spec.namespaced { ns } else { None });
        let list = api.list(&ListParams::default()).await?;
        // only workload kinds derive health from readiness — everything else
        // (pvc/ing/netpol/sa/pv/hpa/cj) is "healthy = exists"
        let is_workload = WORKLOADS.contains(label);
        let mut total = 0usize;
        let mut healthy = 0usize;
        for obj in &list.items {
            if let Ok(v) = serde_json::to_value(obj) {
                total += 1;
                if !is_workload {
                    healthy += 1;
                } else {
                    let r = crate::model::extract(&spec, &v);
                    if r.sev == crate::model::Sev::Ok {
                        healthy += 1;
                    }
                }
            }
        }
        out.push((*label, total, healthy));
    }
    Ok(out)
}

#[cfg(test)]
mod auth_tests {
    use super::*;

    #[test]
    fn auth_expiry_detection() {
        assert!(is_auth_expired("watch pods: Unauthorized"));
        assert!(is_auth_expired("aws sso token expired"));
        assert!(is_auth_expired("401 Unauthorized: invalid token"));
        assert!(is_auth_expired("get failed: credentials rejected"));
        // RBAC denials must NOT trigger the auth modal
        assert!(!is_auth_expired(
            "deployments.apps is forbidden: User cannot patch resource"
        ));
        assert!(!is_auth_expired("403 forbidden"));
        assert!(!is_auth_expired("connection refused"));
    }
}
