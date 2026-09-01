use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum ColSrc {
    P(&'static [&'static str]),
    /// user-defined JSON path (views.yml), dot-separated
    Path(String),
    Name,
    Ns,
    Age,
    PodReady,
    PodStatus,
    PodRestarts,
    /// live millicores from metrics cache (patched at render time)
    PodCpu,
    /// live memory bytes from metrics cache
    PodMem,
    /// node cpu % of allocatable (live)
    NodeCpuPct,
    /// node memory % of allocatable (live)
    NodeMemPct,
    SvcType,
    SvcPorts,
    SvcExternal,
    DeployReady,
    StsReady,
    DsCounts,
    JobCompl,
    NodeReady,
    NodeRoles,
    NodeVersion,
    SecretData,
    EventLast,
    EventCount,
    HpaRef,
    HpaTargets,
    HpaMin,
    HpaMax,
    HpaReplicas,
}

#[derive(Clone, Debug)]
pub struct Col {
    pub name: &'static str,
    pub weight: u16,
    pub src: ColSrc,
}

const fn c(name: &'static str, weight: u16, src: ColSrc) -> Col {
    Col { name, weight, src }
}

#[derive(Clone, Debug)]
pub struct KindSpec {
    pub alias: String,
    pub plural: String,
    pub group: String,
    pub version: String,
    pub kind: String,
    pub namespaced: bool,
    pub cols: Vec<Col>,
}

const PO: &[Col] = &[
    c("NAME", 5, ColSrc::Name),
    c("READY", 2, ColSrc::PodReady),
    c("STATUS", 3, ColSrc::PodStatus),
    c("RESTARTS", 2, ColSrc::PodRestarts),
    c("CPU", 2, ColSrc::PodCpu),
    c("MEM", 2, ColSrc::PodMem),
    c("AGE", 2, ColSrc::Age),
];
const DEPLOY: &[Col] = &[
    c("NAME", 5, ColSrc::Name),
    c("READY", 2, ColSrc::DeployReady),
    c("AGE", 2, ColSrc::Age),
];
const STS: &[Col] = &[
    c("NAME", 5, ColSrc::Name),
    c("READY", 2, ColSrc::StsReady),
    c("AGE", 2, ColSrc::Age),
];
const DS: &[Col] = &[
    c("NAME", 4, ColSrc::Name),
    c("DESIRED", 2, ColSrc::DsCounts),
    c("AGE", 2, ColSrc::Age),
];
const SVC: &[Col] = &[
    c("NAME", 4, ColSrc::Name),
    c("TYPE", 3, ColSrc::SvcType),
    c("CLUSTER-IP", 3, ColSrc::P(&["spec", "clusterIP"])),
    c("EXTERNAL-IP", 3, ColSrc::SvcExternal),
    c("PORTS", 3, ColSrc::SvcPorts),
    c("AGE", 2, ColSrc::Age),
];
const GENERIC: &[Col] = &[c("NAME", 6, ColSrc::Name), c("AGE", 2, ColSrc::Age)];
const HPA: &[Col] = &[
    c("NAME", 4, ColSrc::Name),
    c("REFERENCE", 4, ColSrc::HpaRef),
    c("TARGETS", 5, ColSrc::HpaTargets),
    c("MINPODS", 2, ColSrc::HpaMin),
    c("MAXPODS", 2, ColSrc::HpaMax),
    c("REPLICAS", 2, ColSrc::HpaReplicas),
    c("AGE", 2, ColSrc::Age),
];
const NODE: &[Col] = &[
    c("NAME", 5, ColSrc::Name),
    c("STATUS", 2, ColSrc::NodeReady),
    c("CPU%", 2, ColSrc::NodeCpuPct),
    c("MEM%", 2, ColSrc::NodeMemPct),
    c("ROLES", 3, ColSrc::NodeRoles),
    c("VERSION", 2, ColSrc::NodeVersion),
    c("AGE", 2, ColSrc::Age),
];
const SECRET: &[Col] = &[
    c("NAME", 6, ColSrc::Name),
    c("DATA", 1, ColSrc::SecretData),
    c("AGE", 2, ColSrc::Age),
];
const EVENT: &[Col] = &[
    c("LAST", 3, ColSrc::EventLast),
    c("TYPE", 2, ColSrc::P(&["type"])),
    c("REASON", 3, ColSrc::P(&["reason"])),
    c("OBJECT", 3, ColSrc::P(&["involvedObject", "kind"])),
    c("COUNT", 1, ColSrc::EventCount),
    c("MESSAGE", 6, ColSrc::P(&["message"])),
];

struct Raw {
    aliases: &'static [&'static str],
    plural: &'static str,
    group: &'static str,
    version: &'static str,
    kind: &'static str,
    namespaced: bool,
    cols: &'static [Col],
}
macro_rules! raw {
    ($al:expr, $pl:expr, $g:expr, $v:expr, $k:expr, $ns:expr, $co:expr) => {
        Raw {
            aliases: $al,
            plural: $pl,
            group: $g,
            version: $v,
            kind: $k,
            namespaced: $ns,
            cols: $co,
        }
    };
}

const TABLE: &[Raw] = &[
    raw!(&["po", "pod", "pods"], "pods", "", "v1", "Pod", true, PO),
    raw!(
        &["deploy", "deployments", "deployment"],
        "deployments",
        "apps",
        "v1",
        "Deployment",
        true,
        DEPLOY
    ),
    raw!(
        &["sts", "statefulsets", "statefulset"],
        "statefulsets",
        "apps",
        "v1",
        "StatefulSet",
        true,
        STS
    ),
    raw!(
        &["ds", "daemonsets", "daemonset"],
        "daemonsets",
        "apps",
        "v1",
        "DaemonSet",
        true,
        DS
    ),
    raw!(
        &["rs", "replicasets"],
        "replicasets",
        "apps",
        "v1",
        "ReplicaSet",
        true,
        DEPLOY
    ),
    raw!(&["job", "jobs"], "jobs", "batch", "v1", "Job", true, DEPLOY),
    raw!(
        &["cj", "cronjobs", "cronjob"],
        "cronjobs",
        "batch",
        "v1",
        "CronJob",
        true,
        DEPLOY
    ),
    raw!(
        &["svc", "services", "service"],
        "services",
        "",
        "v1",
        "Service",
        true,
        SVC
    ),
    raw!(
        &["cm", "configmaps"],
        "configmaps",
        "",
        "v1",
        "ConfigMap",
        true,
        GENERIC
    ),
    raw!(
        &["sec", "secrets", "secret"],
        "secrets",
        "",
        "v1",
        "Secret",
        true,
        SECRET
    ),
    raw!(
        &["ing", "ingresses", "ingress"],
        "ingresses",
        "networking.k8s.io",
        "v1",
        "Ingress",
        true,
        GENERIC
    ),
    raw!(
        &["np", "netpol", "networkpolicies"],
        "networkpolicies",
        "networking.k8s.io",
        "v1",
        "NetworkPolicy",
        true,
        GENERIC
    ),
    raw!(
        &["ep", "endpointslices"],
        "endpointslices",
        "discovery.k8s.io",
        "v1",
        "EndpointSlice",
        true,
        GENERIC
    ),
    raw!(
        &["ev", "events", "event"],
        "events",
        "",
        "v1",
        "Event",
        true,
        EVENT
    ),
    raw!(
        &["sa", "serviceaccounts"],
        "serviceaccounts",
        "",
        "v1",
        "ServiceAccount",
        true,
        GENERIC
    ),
    raw!(
        &["role", "roles"],
        "roles",
        "rbac.authorization.k8s.io",
        "v1",
        "Role",
        true,
        GENERIC
    ),
    raw!(
        &["cr", "clusterroles", "clusterrole"],
        "clusterroles",
        "rbac.authorization.k8s.io",
        "v1",
        "ClusterRole",
        false,
        GENERIC
    ),
    raw!(
        &["rb", "rolebindings"],
        "rolebindings",
        "rbac.authorization.k8s.io",
        "v1",
        "RoleBinding",
        true,
        GENERIC
    ),
    raw!(
        &["crb", "clusterrolebindings"],
        "clusterrolebindings",
        "rbac.authorization.k8s.io",
        "v1",
        "ClusterRoleBinding",
        false,
        GENERIC
    ),
    raw!(
        &["pvc", "persistentvolumeclaims"],
        "persistentvolumeclaims",
        "",
        "v1",
        "PersistentVolumeClaim",
        true,
        GENERIC
    ),
    raw!(
        &["pv", "persistentvolumes"],
        "persistentvolumes",
        "",
        "v1",
        "PersistentVolume",
        false,
        GENERIC
    ),
    raw!(
        &["hpa", "horizontalpodautoscalers"],
        "horizontalpodautoscalers",
        "autoscaling",
        "v2",
        "HorizontalPodAutoscaler",
        true,
        HPA
    ),
    raw!(
        &["no", "nodes", "node"],
        "nodes",
        "",
        "v1",
        "Node",
        false,
        NODE
    ),
    raw!(
        &["ns", "namespaces", "namespace"],
        "namespaces",
        "",
        "v1",
        "Namespace",
        false,
        GENERIC
    ),
    raw!(
        &["crd", "crds", "customresourcedefinitions"],
        "customresourcedefinitions",
        "apiextensions.k8s.io",
        "v1",
        "CustomResourceDefinition",
        false,
        GENERIC
    ),
];

pub fn spec_for(alias: &str) -> Option<KindSpec> {
    let a = alias.to_lowercase();
    for r in TABLE {
        if r.aliases.contains(&a.as_str()) {
            return Some(KindSpec {
                alias: r.aliases[0].to_string(),
                plural: r.plural.into(),
                group: r.group.into(),
                version: r.version.into(),
                kind: r.kind.into(),
                namespaced: r.namespaced,
                cols: r.cols.to_vec(),
            });
        }
    }
    None
}

pub fn custom_spec(plural: &str, group: &str, version: &str, kind: &str, nsd: bool) -> KindSpec {
    KindSpec {
        alias: plural.to_string(),
        plural: plural.into(),
        group: group.into(),
        version: version.into(),
        kind: kind.into(),
        namespaced: nsd,
        cols: GENERIC.to_vec(),
    }
}

pub fn all_aliases() -> Vec<&'static str> {
    TABLE
        .iter()
        .flat_map(|r| r.aliases.iter().copied())
        .collect()
}

fn jget<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    Some(cur)
}

fn jstr(v: &Value, path: &[&str]) -> String {
    jget(v, path)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn jint(v: &Value, path: &[&str]) -> i64 {
    jget(v, path).and_then(|x| x.as_i64()).unwrap_or(0)
}

pub fn age_of(v: &Value) -> String {
    let ts = jstr(v, &["metadata", "creationTimestamp"]);
    if ts.is_empty() {
        return "-".into();
    }
    match DateTime::parse_from_rfc3339(&ts) {
        Ok(t) => hum((Utc::now() - t.with_timezone(&Utc)).num_seconds().max(0)),
        Err(_) => "-".into(),
    }
}

fn hum(secs: i64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{s}s")
    }
}

pub fn pod_status(v: &Value) -> String {
    let phase = jstr(v, &["status", "phase"]);
    let mut reason = String::new();
    if let Some(cs) = jget(v, &["status", "containerStatuses"]).and_then(|x| x.as_array()) {
        for c in cs {
            if let Some(w) = c.get("state").and_then(|s| s.get("waiting"))
                && let Some(r) = w.get("reason").and_then(|x| x.as_str())
                && matches!(
                    r,
                    "CrashLoopBackOff"
                        | "ImagePullBackOff"
                        | "ErrImagePull"
                        | "CreateContainerConfigError"
                        | "CreateContainerError"
                )
            {
                reason = r.to_string();
                break;
            }
        }
    }
    if reason.is_empty() {
        reason = jstr(v, &["status", "reason"]);
    }
    match phase.as_str() {
        "Running" if !reason.is_empty() => reason,
        "Running" => "Running".into(),
        "Pending" if !reason.is_empty() => reason,
        other => {
            if other.is_empty() {
                reason
            } else if !reason.is_empty() && other != "Succeeded" && other != "Failed" {
                format!("{other}({reason})")
            } else {
                other.into()
            }
        }
    }
}

/// total restart count across app + init containers
pub fn pod_restarts(v: &Value) -> i64 {
    let sum = |path: &[&str]| -> i64 {
        jget(v, path)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|c| c.get("restartCount").and_then(|r| r.as_i64()))
                    .sum()
            })
            .unwrap_or(0)
    };
    sum(&["status", "containerStatuses"]) + sum(&["status", "initContainerStatuses"])
}

/// parse a k8s CPU quantity ("250m", "2", "123456789n", "500u") into millicores
pub fn qty_cpu_m(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }
    if let Some(stripped) = s.strip_suffix('m') {
        stripped.parse::<f64>().unwrap_or(0.0)
    } else if let Some(stripped) = s.strip_suffix('u') {
        stripped.parse::<f64>().unwrap_or(0.0) / 1000.0
    } else if let Some(stripped) = s.strip_suffix('n') {
        stripped.parse::<f64>().unwrap_or(0.0) / 1_000_000.0
    } else {
        s.parse::<f64>().unwrap_or(0.0) * 1000.0
    }
}

/// parse a k8s memory quantity ("1024Ki", "8Gi", "128974848") into bytes
pub fn qty_mem_b(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    let (num_str, mult) = if let Some(stripped) = s.strip_suffix("Ki") {
        (stripped, 1024u64)
    } else if let Some(stripped) = s.strip_suffix("Mi") {
        (stripped, 1024u64 * 1024)
    } else if let Some(stripped) = s.strip_suffix("Gi") {
        (stripped, 1024u64 * 1024 * 1024)
    } else if let Some(stripped) = s.strip_suffix("Ti") {
        (stripped, 1024u64 * 1024 * 1024 * 1024)
    } else if let Some(stripped) = s.strip_suffix("Pi") {
        (stripped, 1024u64 * 1024 * 1024 * 1024 * 1024)
    } else if let Some(stripped) = s.strip_suffix("Ei") {
        (stripped, 1024u64 * 1024 * 1024 * 1024 * 1024 * 1024)
    } else if let Some(stripped) = s.strip_suffix('K') {
        (stripped, 1_000u64)
    } else if let Some(stripped) = s.strip_suffix('M') {
        (stripped, 1_000_000u64)
    } else if let Some(stripped) = s.strip_suffix('G') {
        (stripped, 1_000_000_000u64)
    } else if let Some(stripped) = s.strip_suffix('T') {
        (stripped, 1_000_000_000_000u64)
    } else if let Some(stripped) = s.strip_suffix('P') {
        (stripped, 1_000_000_000_000_000u64)
    } else if let Some(stripped) = s.strip_suffix('E') {
        (stripped, 1_000_000_000_000_000_000u64)
    } else {
        (s, 1u64)
    };
    if let Ok(val) = num_str.trim().parse::<u64>() {
        val.saturating_mul(mult)
    } else if let Ok(val) = num_str.trim().parse::<f64>() {
        (val * mult as f64) as u64
    } else {
        0
    }
}

/// format millicores for table cells ("123m")
pub fn fmt_cpu_m(m: f64) -> String {
    if m >= 1000.0 {
        format!("{:.0}m", m)
    } else {
        format!("{m:.0}m")
    }
}

/// format bytes for pod memory cells (MiB scale, matching kubectl top)
pub fn fmt_mem_mi(b: u64) -> String {
    format!("{}Mi", b / (1024 * 1024))
}

/// embedded field reference for common kinds (`:ref <kind>`)
pub fn reference_for(kind: &str) -> Option<&'static [(&'static str, &'static str)]> {
    Some(match kind {
        "Pod" => &[
            ("metadata.name", "pod identity"),
            (
                "spec.containers[]",
                "app containers (image, command, env, ports)",
            ),
            (
                "spec.initContainers[]",
                "run-to-completion helpers before app start",
            ),
            ("spec.nodeName", "node the pod is scheduled on"),
            (
                "spec.volumes[]",
                "attached volumes (secret/configMap/pvc/hostPath)",
            ),
            (
                "status.phase",
                "Pending | Running | Succeeded | Failed | Unknown",
            ),
            (
                "status.containerStatuses[].restartCount",
                "restart counter per container",
            ),
            ("status.podIP", "cluster-internal IP"),
            (
                "spec.securityContext",
                "pod-level user/fsGroup/RunAsNonRoot settings",
            ),
        ],
        "Deployment" => &[
            ("spec.replicas", "desired replica count"),
            ("spec.selector.matchLabels", "labels selecting owned pods"),
            ("spec.template", "pod template rolled out on change"),
            ("spec.strategy.type", "RollingUpdate or Recreate"),
            ("status.readyReplicas", "replicas currently ready"),
            ("status.unavailableReplicas", "replicas not yet available"),
        ],
        "StatefulSet" => &[
            ("spec.serviceName", "headless svc governing pod DNS"),
            ("spec.volumeClaimTemplates[]", "per-replica stable PVCs"),
            (
                "spec.podManagementPolicy",
                "OrderedReady (default) or Parallel",
            ),
            ("status.currentRevision", "active controller revision"),
        ],
        "DaemonSet" => &[
            ("spec.selector", "labels selecting managed pods"),
            (
                "status.desiredNumberScheduled",
                "nodes that should run the daemon",
            ),
            ("status.numberReady", "nodes with a ready daemon pod"),
        ],
        "Service" => &[
            (
                "spec.type",
                "ClusterIP | NodePort | LoadBalancer | ExternalName",
            ),
            ("spec.selector", "label match directing traffic to pods"),
            ("spec.ports[]", "port/targetPort/protocol/nodePort triples"),
            ("spec.clusterIP", "virtual VIP (None = headless)"),
            ("status.loadBalancer.ingress", "LB address once provisioned"),
        ],
        "Node" => &[
            ("metadata.labels", "roles/zones/instance-type labels"),
            (
                "spec.taints[]",
                "scheduling repel rules (effect NoSchedule…)",
            ),
            ("spec.unschedulable", "true when cordoned"),
            (
                "status.conditions[]",
                "Ready/MemoryPressure/DiskPressure states",
            ),
            ("status.allocatable", "schedulable cpu/memory/pods capacity"),
            ("status.nodeInfo.kubeletVersion", "kubelet version string"),
        ],
        "PersistentVolumeClaim" => &[
            ("spec.accessModes[]", "RWO | ROX | RWX | RWOP"),
            ("spec.resources.requests.storage", "requested size"),
            ("spec.storageClassName", "storage class backing the claim"),
            ("spec.volumeName", "bound PV (empty until bound)"),
            ("status.phase", "Bound | Pending | Lost"),
        ],
        "CronJob" => &[
            (
                "spec.schedule",
                "cron expression (TZ-aware w/ timeZone field)",
            ),
            ("spec.jobTemplate.spec", "Job spec spawned per trigger"),
            ("spec.suspend", "pause future schedules"),
            ("spec.concurrencyPolicy", "Allow | Forbid | Replace"),
            (
                "status.lastScheduleTime",
                "last successful schedule instant",
            ),
        ],
        "Job" => &[
            ("spec.completions", "target successful pod count"),
            ("spec.parallelism", "max pods running concurrently"),
            ("spec.backoffLimit", "retries before marking failed"),
            ("status.succeeded/failed", "completed / failed pod counters"),
        ],
        "Secret" => &[
            ("type", "Opaque | kubernetes.io/tls | docker-config …"),
            ("data.<key>", "base64-encoded values (X decodes)"),
            ("stringData.<key>", "write-only plaintext input"),
            ("immutable", "true = cannot be updated, only deleted"),
        ],
        "ConfigMap" => &[
            ("data.<key>", "utf-8 config entries"),
            ("binaryData.<key>", "base64 binary blobs"),
            ("immutable", "true = cannot be updated, only deleted"),
        ],
        "Ingress" => &[
            ("spec.ingressClassName", "controller selection"),
            ("spec.rules[].host/paths", "host + path routing table"),
            ("spec.tls[].secretName", "TLS cert secret per host"),
            (
                "status.loadBalancer.ingress",
                "assigned entrypoint addresses",
            ),
        ],
        "Role" | "ClusterRole" => &[
            ("rules[].apiGroups", "API groups covered ('' = core)"),
            ("rules[].resources", "resource names incl. subresources"),
            ("rules[].verbs", "get/list/watch/create/update/patch/delete"),
            ("rules[].nonResourceURLs", "URL-path permissions"),
        ],
        _ => return None,
    })
}

/// kinds known to be cluster-scoped beyond the static list in k8s.rs
pub fn all_kinds_cluster_scoped() -> &'static [&'static str] {
    &["APIService", "CSIDriver", "CSINode", "RuntimeClass"]
}

/// column index of a metric source in a spec, if present
pub fn metric_cols(spec: &KindSpec) -> (Option<usize>, Option<usize>) {
    let cpu = spec.cols.iter().position(|c| c.src == ColSrc::PodCpu);
    let mem = spec.cols.iter().position(|c| c.src == ColSrc::PodMem);
    (cpu, mem)
}

impl KindSpec {
    /// build a user-defined column from a views.yml entry.
    /// names/paths are leaked to 'static — loaded once at startup, tiny count.
    pub fn dyn_col(name: &str, path: &str) -> Col {
        let name: &'static str = Box::leak(name.to_string().into_boxed_str());
        Col {
            name,
            weight: 2,
            src: ColSrc::Path(path.to_string()),
        }
    }
}

fn hpa_targets(v: &Value) -> String {
    let specs = match jget(v, &["spec", "metrics"]).and_then(|x| x.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => return "<none>".into(),
    };
    let statuses = jget(v, &["status", "currentMetrics"]).and_then(|x| x.as_array());
    let mut parts: Vec<String> = vec![];
    for spec in specs.iter().take(2) {
        let mtype = spec.get("type").and_then(|x| x.as_str()).unwrap_or("");
        match mtype {
            "Resource" => {
                let name = spec
                    .pointer("/resource/name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("?");
                let target_util = spec
                    .pointer("/resource/target/averageUtilization")
                    .and_then(|x| x.as_i64());
                let target_val = spec
                    .pointer("/resource/target/averageValue")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                // find matching current
                let mut cur_util: Option<i64> = None;
                let mut cur_val: Option<String> = None;
                if let Some(arr) = statuses {
                    for st in arr {
                        if st.get("type").and_then(|x| x.as_str()) != Some("Resource") {
                            continue;
                        }
                        if st.pointer("/resource/name").and_then(|x| x.as_str()) != Some(name) {
                            continue;
                        }
                        if let Some(u) = st
                            .pointer("/resource/current/averageUtilization")
                            .and_then(|x| x.as_i64())
                        {
                            cur_util = Some(u);
                        }
                        if let Some(s) = st
                            .pointer("/resource/current/averageValue")
                            .and_then(|x| x.as_str())
                        {
                            cur_val = Some(s.to_string());
                        }
                    }
                }
                if let Some(tu) = target_util {
                    let cur = cur_util
                        .map(|c| format!("{c}%"))
                        .unwrap_or("<unknown>".into());
                    parts.push(format!("{name}: {cur}/{tu}%"));
                } else if let Some(tv) = target_val {
                    let cur = cur_val.unwrap_or("<unknown>".into());
                    parts.push(format!("{name}: {cur}/{tv}"));
                } else {
                    parts.push(format!("{name}: <unknown>/<?>"));
                }
            }
            "ContainerResource" => {
                let name = spec
                    .pointer("/containerResource/name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("?");
                let container = spec
                    .pointer("/containerResource/container")
                    .and_then(|x| x.as_str())
                    .unwrap_or("?");
                let tu = spec
                    .pointer("/containerResource/target/averageUtilization")
                    .and_then(|x| x.as_i64());
                let tv = spec
                    .pointer("/containerResource/target/averageValue")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                let mut cur_util: Option<i64> = None;
                let mut cur_val: Option<String> = None;
                if let Some(arr) = statuses {
                    for st in arr {
                        if st.get("type").and_then(|x| x.as_str()) != Some("ContainerResource") {
                            continue;
                        }
                        if let Some(s) = st
                            .pointer("/containerResource/current/averageValue")
                            .and_then(|x| x.as_str())
                        {
                            cur_val = Some(s.to_string());
                        }
                        if let Some(u) = st
                            .pointer("/containerResource/current/averageUtilization")
                            .and_then(|x| x.as_i64())
                        {
                            cur_util = Some(u);
                        }
                    }
                }
                if let Some(t) = tu {
                    let cur = cur_util
                        .map(|c| format!("{c}%"))
                        .unwrap_or("<unknown>".into());
                    parts.push(format!("{container}/{name}: {cur}/{t}%"));
                } else if let Some(t) = tv {
                    let cur = cur_val.unwrap_or("<unknown>".into());
                    parts.push(format!("{container}/{name}: {cur}/{t}"));
                }
            }
            "Pods" => {
                let tv = spec
                    .pointer("/pods/target/averageValue")
                    .and_then(|x| x.as_str())
                    .unwrap_or("?");
                let mut cur: Option<String> = None;
                if let Some(arr) = statuses {
                    for st in arr {
                        if st.get("type").and_then(|x| x.as_str()) != Some("Pods") {
                            continue;
                        }
                        if let Some(s) = st
                            .pointer("/pods/current/averageValue")
                            .and_then(|x| x.as_str())
                        {
                            cur = Some(s.to_string());
                        }
                    }
                }
                parts.push(format!(
                    "{}/{} (avg)",
                    cur.unwrap_or("<unknown>".into()),
                    tv
                ));
            }
            "Object" => {
                let tv = spec
                    .pointer("/object/target/value")
                    .and_then(|x| x.as_str())
                    .or_else(|| {
                        spec.pointer("/object/target/averageValue")
                            .and_then(|x| x.as_str())
                    })
                    .unwrap_or("?");
                parts.push(format!("<unknown>/{tv}"));
            }
            "External" => {
                let tv = spec
                    .pointer("/external/target/value")
                    .and_then(|x| x.as_str())
                    .or_else(|| {
                        spec.pointer("/external/target/averageValue")
                            .and_then(|x| x.as_str())
                    })
                    .unwrap_or("?");
                parts.push(format!("<unknown>/{tv}"));
            }
            other => parts.push(other.to_string()),
        }
    }
    if parts.is_empty() {
        "<none>".into()
    } else {
        let mut s = parts.join(", ");
        if specs.len() > 2 {
            s.push_str(&format!(" + {} more...", specs.len() - 2));
        }
        s
    }
}

fn svc_ports(v: &Value) -> String {
    match jget(v, &["spec", "ports"]).and_then(|x| x.as_array()) {
        Some(a) if !a.is_empty() => a
            .iter()
            .map(|p| {
                let port = p.get("port").and_then(|x| x.as_i64()).unwrap_or(0);
                let tp = p.get("protocol").and_then(|x| x.as_str()).unwrap_or("TCP");
                let np = p.get("nodePort").and_then(|x| x.as_i64()).unwrap_or(0);
                if np > 0 {
                    format!("{port}:{np}/{tp}")
                } else {
                    format!("{port}/{tp}")
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        _ => "-".into(),
    }
}

fn svc_external(v: &Value) -> String {
    let mut ips: Vec<String> = vec![];
    if let Some(a) = jget(v, &["spec", "externalIPs"]).and_then(|x| x.as_array()) {
        ips.extend(a.iter().filter_map(|i| i.as_str()).map(String::from));
    }
    if let Some(a) = jget(v, &["status", "loadBalancer", "ingress"]).and_then(|x| x.as_array()) {
        for i in a {
            if let Some(ip) = i.get("ip").and_then(|x| x.as_str()) {
                ips.push(ip.into());
            } else if let Some(h) = i.get("hostname").and_then(|x| x.as_str()) {
                ips.push(h.into());
            }
        }
    }
    if ips.is_empty() {
        "<none>".into()
    } else {
        ips.join(",")
    }
}

pub fn extract(spec: &KindSpec, v: &Value) -> Row {
    let name = jstr(v, &["metadata", "name"]);
    let mut cells = Vec::with_capacity(spec.cols.len());
    for col in &spec.cols {
        cells.push(match &col.src {
            ColSrc::P(p) => {
                let s = jstr(v, p);
                if s.is_empty() { "-".into() } else { s }
            }
            ColSrc::Name => name.clone(),
            ColSrc::Ns => jstr(v, &["metadata", "namespace"]),
            ColSrc::Age => age_of(v),
            ColSrc::PodReady => {
                let cs = jget(v, &["status", "containerStatuses"]).and_then(|x| x.as_array());
                let total = jget(v, &["spec", "containers"])
                    .and_then(|x| x.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let ready = cs
                    .map(|a| {
                        a.iter()
                            .filter(|c| c.get("ready").and_then(|x| x.as_bool()).unwrap_or(false))
                            .count()
                    })
                    .unwrap_or(0);
                format!("{ready}/{total}")
            }
            ColSrc::PodStatus => pod_status(v),
            ColSrc::PodRestarts => pod_restarts(v).to_string(),
            ColSrc::PodCpu | ColSrc::PodMem | ColSrc::NodeCpuPct | ColSrc::NodeMemPct => {
                // patched from the metrics cache in app::filtered_sorted; "-" until sampled
                "-".into()
            }
            ColSrc::Path(p) => {
                let parts: Vec<&str> = p.split('.').collect();
                let s = jget(v, &parts)
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if s.is_empty() { "-".into() } else { s }
            }
            ColSrc::SvcType => jstr(v, &["spec", "type"]),
            ColSrc::SvcPorts => svc_ports(v),
            ColSrc::SvcExternal => svc_external(v),
            ColSrc::DeployReady => format!(
                "{}/{}",
                jint(v, &["status", "readyReplicas"]),
                jint(v, &["spec", "replicas"])
            ),
            ColSrc::StsReady => format!(
                "{}/{}",
                jint(v, &["status", "readyReplicas"]),
                jint(v, &["spec", "replicas"])
            ),
            ColSrc::DsCounts => jint(v, &["status", "desiredNumberScheduled"]).to_string(),
            ColSrc::JobCompl => format!(
                "{}/{}",
                jint(v, &["status", "succeeded"]),
                jint(v, &["spec", "completions"])
            ),
            ColSrc::NodeReady => {
                let ok = jget(v, &["status", "conditions"])
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter().any(|cd| {
                            cd.get("type").and_then(|t| t.as_str()) == Some("Ready")
                                && cd.get("status").and_then(|s| s.as_str()) == Some("True")
                        })
                    })
                    .unwrap_or(false);
                if ok { "Ready" } else { "NotReady" }.to_string()
            }
            ColSrc::NodeRoles => {
                let roles: Vec<String> = jget(v, &["metadata", "labels"])
                    .and_then(|l| l.as_object())
                    .map(|o| {
                        o.keys()
                            .filter_map(|k| {
                                k.strip_prefix("node-role.kubernetes.io/").map(String::from)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if roles.is_empty() {
                    "<none>".into()
                } else {
                    roles.join(",")
                }
            }
            ColSrc::NodeVersion => jstr(v, &["status", "nodeInfo", "kubeletVersion"]),
            ColSrc::SecretData => jget(v, &["data"])
                .and_then(|d| d.as_object())
                .map(|o| o.len().to_string())
                .unwrap_or_else(|| "0".into()),
            ColSrc::EventLast => {
                let t = jstr(v, &["lastTimestamp"]);
                let t = if t.is_empty() {
                    jstr(v, &["eventTime"])
                } else {
                    t
                };
                DateTime::parse_from_rfc3339(&t)
                    .map(|d| hum((Utc::now() - d.with_timezone(&Utc)).num_seconds().max(0)))
                    .unwrap_or("-".into())
            }
            ColSrc::EventCount => jint(v, &["count"]).to_string(),
            ColSrc::HpaRef => {
                let kind = jstr(v, &["spec", "scaleTargetRef", "kind"]);
                let name = jstr(v, &["spec", "scaleTargetRef", "name"]);
                if kind.is_empty() && name.is_empty() {
                    "-".into()
                } else {
                    format!("{kind}/{name}")
                }
            }
            ColSrc::HpaTargets => hpa_targets(v),
            ColSrc::HpaMin => jget(v, &["spec", "minReplicas"])
                .and_then(|x| x.as_i64())
                .map(|n| n.to_string())
                .unwrap_or("<unset>".into()),
            ColSrc::HpaMax => jget(v, &["spec", "maxReplicas"])
                .and_then(|x| x.as_i64())
                .map(|n| n.to_string())
                .unwrap_or("-".into()),
            ColSrc::HpaReplicas => jget(v, &["status", "currentReplicas"])
                .and_then(|x| x.as_i64())
                .map(|n| n.to_string())
                .unwrap_or("0".into()),
        });
    }
    let kind_str = spec.kind.as_str();
    let mut flags = 0u8;
    if kind_str == "Node"
        && jget(v, &["spec", "unschedulable"]).and_then(|x| x.as_bool()) == Some(true)
    {
        flags |= FLAG_CORDONED;
    }
    if kind_str == "CronJob"
        && jget(v, &["spec", "suspend"]).and_then(|x| x.as_bool()) == Some(true)
    {
        flags |= FLAG_SUSPENDED;
    }
    let ns = jstr(v, &["metadata", "namespace"]);
    let key = if spec.namespaced && !ns.is_empty() {
        format!("{ns}/{name}")
    } else {
        name
    };
    Row {
        key,
        ns,
        cells,
        sev: severity(spec.kind.as_str(), v),
        flags,
    }
}

fn severity(kind: &str, v: &Value) -> Sev {
    match kind {
        "Pod" => {
            let status = pod_status(v);
            let failing = matches!(status.as_str(), "Evicted" | "Failed")
                || status.contains("BackOff")
                || status.contains("Error")
                || status.starts_with("Err");
            if failing {
                return Sev::Bad;
            }
            // churning pod: healthy right now but restarted more than once
            if pod_restarts(v) > 1 {
                return Sev::Warn;
            }
            match status.as_str() {
                "Running" | "Succeeded" | "Completed" => Sev::Ok,
                "Pending" | "ContainerCreating" | "Terminating" => Sev::Warn,
                _ => Sev::Info,
            }
        }
        "Node" => {
            let ok = jget(v, &["status", "conditions"])
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter().any(|cd| {
                        cd.get("type").and_then(|t| t.as_str()) == Some("Ready")
                            && cd.get("status").and_then(|s| s.as_str()) == Some("True")
                    })
                })
                .unwrap_or(false);
            if ok { Sev::Ok } else { Sev::Bad }
        }
        "Deployment" | "StatefulSet" | "ReplicaSet" | "DaemonSet" | "Job" => {
            let want = jint(v, &["spec", "replicas"]);
            let got = jint(v, &["status", "readyReplicas"]);
            let failed = jint(v, &["status", "failed"]);
            if failed > 0 {
                Sev::Bad
            } else if want == got && want >= 0 {
                Sev::Ok
            } else if got == 0 && want > 0 {
                // nothing ready at all — treat as failing rather than merely degraded
                Sev::Bad
            } else {
                Sev::Warn
            }
        }
        "Event" => {
            if jstr(v, &["type"]) == "Warning" {
                Sev::Warn
            } else {
                Sev::Info
            }
        }
        _ => Sev::Info,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Sev {
    Ok,
    Warn,
    Bad,
    Info,
}

pub const FLAG_CORDONED: u8 = 1;
pub const FLAG_SUSPENDED: u8 = 2;

#[derive(Clone, Debug)]
pub struct Row {
    pub key: String,
    pub ns: String,
    pub cells: Vec<String>,
    pub sev: Sev,
    pub flags: u8,
}

impl Row {
    pub fn name(&self) -> &str {
        if let Some((_, name)) = self.key.split_once('/') {
            name
        } else {
            &self.key
        }
    }
}

pub fn selector_labels(v: &Value) -> Vec<(String, String)> {
    jget(v, &["spec", "selector", "matchLabels"])
        .and_then(|m| m.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// true when a support-end date has already passed (version unsupported/degraded)
pub fn k8s_support_expired(date: &str) -> bool {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| d < chrono::Utc::now().date_naive())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_cordoned_flag() {
        let spec = spec_for("no").unwrap();
        let v: Value = serde_json::json!({
            "metadata": {"name": "n1"},
            "spec": {"unschedulable": true},
            "status": {"conditions": [{"type": "Ready", "status": "True"}]}
        });
        let r = extract(&spec, &v);
        assert_eq!(r.flags & FLAG_CORDONED, FLAG_CORDONED);
    }

    #[test]
    fn cronjob_suspended_flag() {
        let spec = spec_for("cj").unwrap();
        let v: Value = serde_json::json!({
            "metadata": {"name": "c1"},
            "spec": {"suspend": true},
            "status": {}
        });
        let r = extract(&spec, &v);
        assert_eq!(r.flags & FLAG_SUSPENDED, FLAG_SUSPENDED);
    }

    #[test]
    fn k8s_support_dates() {
        assert!(k8s_support_expired("2025-06-28"));
        assert!(!k8s_support_expired("2999-01-01"));
    }

    #[test]
    fn quantities_parse() {
        assert_eq!(qty_cpu_m("250m"), 250.0);
        assert_eq!(qty_cpu_m("2"), 2000.0);
        assert_eq!(qty_cpu_m("123456789n"), 123.456789);
        assert_eq!(qty_cpu_m("500u"), 0.5);
        assert_eq!(qty_mem_b("1024Ki"), 1024 * 1024);
        assert_eq!(qty_mem_b("8Gi"), 8 * 1024 * 1024 * 1024);
        assert_eq!(qty_mem_b("128974848"), 128_974_848);
        assert_eq!(qty_mem_b("1G"), 1_000_000_000);
    }

    #[test]
    fn restart_severity() {
        let spec = spec_for("po").unwrap();
        let healthy_restarted: Value = serde_json::json!({
            "metadata": {"name": "p"},
            "status": {
                "phase": "Running",
                "containerStatuses": [{"ready": true, "restartCount": 3, "state": {"running": {}}}]
            }
        });
        assert_eq!(extract(&spec, &healthy_restarted).sev, Sev::Warn);

        let crashloop: Value = serde_json::json!({
            "metadata": {"name": "p"},
            "status": {
                "phase": "Running",
                "containerStatuses": [{"ready": false, "restartCount": 9,
                    "state": {"waiting": {"reason": "CrashLoopBackOff"}}}]
            }
        });
        assert_eq!(extract(&spec, &crashloop).sev, Sev::Bad);

        let one_restart: Value = serde_json::json!({
            "metadata": {"name": "p"},
            "status": {
                "phase": "Running",
                "containerStatuses": [{"ready": true, "restartCount": 1, "state": {"running": {}}}]
            }
        });
        assert_eq!(extract(&spec, &one_restart).sev, Sev::Ok);
    }

    #[test]
    fn workload_severity() {
        let spec = spec_for("deploy").unwrap();
        let all_ready: Value = serde_json::json!({
            "metadata": {"name": "d"}, "spec": {"replicas": 3}, "status": {"readyReplicas": 3}
        });
        assert_eq!(extract(&spec, &all_ready).sev, Sev::Ok);
        let partial: Value = serde_json::json!({
            "metadata": {"name": "d"}, "spec": {"replicas": 3}, "status": {"readyReplicas": 2}
        });
        assert_eq!(extract(&spec, &partial).sev, Sev::Warn);
        let none_ready: Value = serde_json::json!({
            "metadata": {"name": "d"}, "spec": {"replicas": 3}, "status": {"readyReplicas": 0}
        });
        assert_eq!(extract(&spec, &none_ready).sev, Sev::Bad);
    }
}

#[cfg(test)]
mod hv_tests {
    use super::*;

    #[test]
    fn dyn_col_extracts_path() {
        let mut spec = spec_for("po").unwrap();
        spec.cols.push(KindSpec::dyn_col("NODE", "spec.nodeName"));
        let v: Value = serde_json::json!({
            "metadata": {"name": "p"},
            "spec": {"nodeName": "node-a"},
            "status": {}
        });
        let r = extract(&spec, &v);
        assert_eq!(r.cells.last().unwrap(), "node-a");
        let (cpu, mem) = metric_cols(&spec);
        assert!(cpu.is_some() && mem.is_some());
    }

    #[test]
    fn reference_data_exists() {
        assert!(reference_for("Pod").is_some());
        assert!(reference_for("ClusterRole").is_some());
        assert!(reference_for("Bogus").is_none());
    }

    #[test]
    fn fmt_helpers() {
        assert_eq!(fmt_cpu_m(95.4), "95m");
        assert_eq!(fmt_mem_mi(12 * 1024 * 1024), "12Mi");
    }
}
