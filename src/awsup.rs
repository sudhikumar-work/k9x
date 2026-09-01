use crate::k8s::Cluster;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// support windows for the running k8s version
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SupDates {
    /// last day of standard support (extended support starts the next day)
    pub standard: String,
    /// last day of extended support (full EOL)
    pub extended: String,
    /// true when dates come from the upstream estimate table, not AWS
    #[serde(default)]
    pub estimated: bool,
}

pub fn cache_path() -> PathBuf {
    crate::cfg::cfg_dir().join("eks-support.json")
}

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    #[serde(default)]
    fetched_at: String,
    #[serde(default)]
    versions: BTreeMap<String, SupDates>,
}

fn load_cache() -> Cache {
    std::fs::read_to_string(cache_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_cache(c: &Cache) {
    if let Ok(j) = serde_json::to_string_pretty(c) {
        let _ = crate::cfg::secure_write(cache_path(), j);
    }
}

/// best-effort region detection: EKS ARN context name or *.eks.<region>.amazonaws.com server
pub fn eks_region(cluster: &Cluster) -> Option<String> {
    let ctx = &cluster.ctx_name;
    if let Some(rest) = ctx.strip_prefix("arn:") {
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() >= 4 && (parts[1] == "eks" || parts[0].contains("eks")) {
            return Some(parts[2].to_string());
        }
        return None;
    }
    // fall back to the API server host of the current context
    let cname = cluster
        .kubeconfig
        .contexts
        .iter()
        .find(|c| c.name == *ctx)
        .and_then(|c| c.context.as_ref())
        .map(|cx| cx.cluster.clone())?;
    let server = cluster
        .kubeconfig
        .clusters
        .iter()
        .find(|cl| cl.name == cname)
        .and_then(|cl| cl.cluster.as_ref())
        .and_then(|cc| cc.server.clone())?;
    let host = server
        .strip_prefix("https://")
        .or_else(|| server.strip_prefix("http://"))?
        .split('/')
        .next()?;
    let before_eks = host.split(".eks.").next()?; // e.g. "ABC.gr7.eu-central-2"
    let region = before_eks.rsplit('.').next()?.to_string();
    if host.contains(".eks.") && host.contains("amazonaws.com") && region.contains('-') {
        Some(region)
    } else {
        None
    }
}

/// Official Amazon EKS release dates per the EKS user guide
/// (docs.aws.amazon.com/eks/latest/userguide/kubernetes-versions.html).
/// Versions outside this list are projected from the ~4-month release cadence.
fn eks_release_date(minor: u64) -> Option<chrono::NaiveDate> {
    use chrono::NaiveDate;
    Some(match minor {
        31 => NaiveDate::from_ymd_opt(2024, 9, 26)?,
        32 => NaiveDate::from_ymd_opt(2025, 1, 23)?,
        33 => NaiveDate::from_ymd_opt(2025, 5, 29)?,
        34 => NaiveDate::from_ymd_opt(2025, 10, 2)?,
        35 => NaiveDate::from_ymd_opt(2026, 1, 27)?,
        36 => NaiveDate::from_ymd_opt(2026, 6, 2)?,
        _ => return None,
    })
}

const ANCHOR_MINOR: u64 = 36;
/// observed upstream/EKS cadence: ~4 months between minor releases
const CADENCE_DAYS: i64 = 122;
/// EKS lifecycle policy: 14 months of standard support, then 12 months extended
const STD_SUPPORT_MONTHS: u32 = 14;
const EXT_SUPPORT_MONTHS: u32 = 12;

fn add_months(d: chrono::NaiveDate, m: u32) -> chrono::NaiveDate {
    use chrono::{Datelike, NaiveDate};
    let total = d.year() * 12 + d.month0() as i32 + m as i32;
    let y = total.div_euclid(12);
    let mo = (total.rem_euclid(12)) as u32 + 1;
    let dim = |y: i32, mo: u32| -> u32 {
        NaiveDate::from_ymd_opt(
            if mo == 12 { y + 1 } else { y },
            if mo == 12 { 1 } else { mo + 1 },
            1,
        )
        .unwrap()
        .signed_duration_since(NaiveDate::from_ymd_opt(y, mo, 1).unwrap())
        .num_days() as u32
    };
    NaiveDate::from_ymd_opt(y, mo, d.day().min(dim(y, mo))).unwrap_or(d)
}

/// project the EKS GA date for any minor version off the official anchor list
fn projected_release(minor: u64) -> chrono::NaiveDate {
    use chrono::Duration;
    let anchor = eks_release_date(ANCHOR_MINOR).unwrap();
    let delta = minor as i64 - ANCHOR_MINOR as i64;
    anchor + Duration::days(delta * CADENCE_DAYS)
}

/// The EKS version lifecycle:
///   end of standard support = release + 14 months
///   end of extended support = end of standard support + 12 months (26 total)
/// Reproduces every published row of the AWS calendar exactly.
pub fn lifecycle(release: chrono::NaiveDate) -> (chrono::NaiveDate, chrono::NaiveDate) {
    let std_end = add_months(release, STD_SUPPORT_MONTHS);
    let ext_end = add_months(std_end, EXT_SUPPORT_MONTHS);
    (std_end, ext_end)
}

/// offline fallback: policy-computed dates for any minor; `estimated` is false only
/// for minors whose release dates are officially published in the AWS docs.
pub fn static_fallback(minor: u64) -> Option<SupDates> {
    if !(22..=60).contains(&minor) {
        return None; // nothing sensible to say about ancient/far-future versions
    }
    let published = eks_release_date(minor).is_some();
    let release = eks_release_date(minor).unwrap_or_else(|| projected_release(minor));
    let (std, ext) = lifecycle(release);
    Some(SupDates {
        standard: std.format("%Y-%m-%d").to_string(),
        extended: ext.format("%Y-%m-%d").to_string(),
        estimated: !published,
    })
}

/// resolve support dates for a running version:
/// 1. file cache (~/.config/k9x/eks-support.json, keyed by minor — survives restarts,
///    refetched only when the cluster runs a version not yet in the cache)
/// 2. live `aws eks describe-cluster-versions` (EKS contexts only)
/// 3. policy-computed fallback from the published release calendar
pub async fn resolve(cluster: &Cluster, version: &str) -> Option<SupDates> {
    let minor_raw = version.trim_start_matches('v').split('.').nth(1)?;
    let minor: u64 = minor_raw.parse().unwrap_or(0);
    if minor == 0 {
        return None;
    }
    let key = format!("{minor}");
    let is_eks = eks_region(cluster).is_some();

    // 1. cache
    let mut cache = load_cache();
    if let Some(d) = cache.versions.get(&key).cloned() {
        return Some(mark(d, is_eks));
    }

    // 2. live fetch — only meaningful for EKS
    if let Some(region) = eks_region(cluster)
        && let Some(d) = fetch_from_aws(&region)
            .await
            .and_then(|m| m.get(&key).cloned())
    {
        cache.fetched_at = chrono::Utc::now().to_rfc3339();
        cache.versions.insert(key, d.clone());
        save_cache(&cache);
        return Some(d); // AWS-sourced: exact
    }

    // 3. policy-computed calendar fallback
    static_fallback(minor).map(|d| mark(d, is_eks))
}

/// non-EKS clusters get estimate-marked dates even when the numbers are exact,
/// because the EKS lifecycle technically does not govern upstream/kind clusters
fn mark(mut d: SupDates, is_eks: bool) -> SupDates {
    if !is_eks {
        d.estimated = true;
    }
    d
}

/// run `aws eks describe-cluster-versions --region R -o json` and map minor → SupDates
async fn fetch_from_aws(region: &str) -> Option<BTreeMap<String, SupDates>> {
    let region = region.to_string();
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("aws")
                .args([
                    "eks",
                    "describe-cluster-versions",
                    "--region",
                    &region,
                    "--output",
                    "json",
                ])
                .env("AWS_CLI_AUTO_PROMPT", "off")
                .stderr(std::process::Stdio::null())
                .output()
        }),
    )
    .await
    .ok()?
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_aws_versions(&String::from_utf8_lossy(&out.stdout))
}

/// leniently parse the describe-cluster-versions payload (key-name tolerant)
fn parse_aws_versions(json: &str) -> Option<BTreeMap<String, SupDates>> {
    let v: Value = serde_json::from_str(json).ok()?;
    let items = v
        .get("clusterVersions")
        .or_else(|| v.get("ClusterVersions"))
        .or_else(|| v.get("items"))
        .and_then(|x| x.as_array())?;
    let mut out = BTreeMap::new();
    for item in items {
        let ver = item
            .get("clusterVersion")
            .or_else(|| item.get("version"))
            .and_then(|x| x.as_str())?;
        let mut standard = String::new();
        let mut extended = String::new();
        if let Some(obj) = item.as_object() {
            for (k, val) in obj {
                let kl = k.to_lowercase();
                let sv = match val.as_str() {
                    Some(s) => s,
                    None => continue,
                };
                if kl.contains("standardsupport") && standard.is_empty() {
                    standard = short_date(sv);
                } else if kl.contains("extendedsupport") && extended.is_empty() {
                    extended = short_date(sv);
                }
            }
        }
        if !standard.is_empty() {
            if extended.is_empty() {
                extended = standard.clone();
            }
            let minor = ver.split('.').nth(1)?.to_string();
            out.insert(
                minor,
                SupDates {
                    standard,
                    extended,
                    estimated: false,
                },
            );
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn short_date(s: &str) -> String {
    // AWS returns RFC3339 ("2025-10-28T00:00:00Z"); keep just the date part
    s.split('T').next().unwrap_or(s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_payload_parse() {
        let j = r#"{"clusterVersions":[
            {"clusterVersion":"1.31","endOfStandardSupportDate":"2025-11-25T16:00:00-08:00","endOfExtendedSupportDate":"2026-11-25T16:00:00-08:00","status":"STANDARD_SUPPORT"},
            {"clusterVersion":"1.32","endOfStandardSupportDate":"2026-03-22T16:00:00-08:00","endOfExtendedSupportDate":"2027-03-22T16:00:00-08:00","status":"STANDARD_SUPPORT"},
            {"clusterVersion":"1.33"}
        ]}"#;
        let m = parse_aws_versions(j).unwrap();
        assert_eq!(m["31"].standard, "2025-11-25");
        assert_eq!(m["31"].extended, "2026-11-25");
        assert!(!m["31"].estimated);
        assert_eq!(m["32"].standard, "2026-03-22");
        assert!(!m.contains_key("33")); // no dates published yet → skipped
    }

    #[test]
    fn lifecycle_reproduces_official_calendar() {
        // exact rows from docs.aws.amazon.com kubernetes-versions.html (UTC+0)
        let official: [(u64, &str, &str); 6] = [
            (31, "2025-11-26", "2026-11-26"),
            (32, "2026-03-23", "2027-03-23"),
            (33, "2026-07-29", "2027-07-29"),
            (34, "2026-12-02", "2027-12-02"),
            (35, "2027-03-27", "2028-03-27"),
            (36, "2027-08-02", "2028-08-02"),
        ];
        for (minor, std, ext) in official {
            let d = static_fallback(minor).unwrap();
            assert_eq!(d.standard, std, "std mismatch for 1.{minor}");
            assert_eq!(d.extended, ext, "ext mismatch for 1.{minor}");
            assert!(!d.estimated, "published row must not be estimated");
        }
    }

    #[test]
    fn projected_and_legacy_versions() {
        // future minor: follows the same policy, flagged as estimate
        let d37 = static_fallback(37).unwrap();
        assert!(d37.estimated);
        assert!(d37.standard.as_str() > "2027-08-02"); // strictly newer than 1.36's window
        assert!(d37.extended > d37.standard);

        // legacy minor: fully past (both dates expired), still sensible
        let d24 = static_fallback(24).unwrap();
        assert!(crate::model::k8s_support_expired(&d24.extended));

        // out-of-range guards
        assert!(static_fallback(20).is_none());
        assert!(static_fallback(99).is_none());
    }
}
