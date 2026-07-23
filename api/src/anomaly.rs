//! Spend-anomaly detection over daily cost series — the pure, AWS-free core of
//! manifest's alerting. Given per-dimension daily amounts (a dimension is a
//! service or an account), it flags the most recent day when it deviates from a
//! spike-robust trailing baseline. The Lambda binary (`src/bin/anomaly.rs`) wires
//! this to Cost Explorer + SNS; everything interesting lives here so it can be
//! unit-tested without touching AWS.
//!
//! This is deliberately manifest's *own* detector, not a wrapper over AWS Cost
//! Anomaly Detection: the whole point is a spend watchdog that doesn't depend on
//! the same console it's meant to backstop — plain, inspectable arithmetic you
//! can reason about and tune.

use std::collections::BTreeMap;

/// Below this trailing baseline (dollars/day) a dimension counts as "not really
/// spending", so any material new spend reads as `NewSpend` rather than a
/// percentage spike off ~zero (which would be a meaningless huge percentage).
const NEW_SPEND_BASELINE_EPS: f64 = 0.01;

/// Alerting thresholds. A day must clear *both* gates to fire, which is what keeps
/// the signal quiet: a big percentage on a trivial service (the `min_dollars`
/// floor) and a big absolute jump on an already-large service (the `pct` gate)
/// are both filtered unless they coincide.
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    /// Minimum absolute daily increase over baseline, in dollars.
    pub min_dollars: f64,
    /// Minimum relative increase over baseline, in percent (spikes only).
    pub pct: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds { min_dollars: 5.0, pct: 50.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnomalyKind {
    /// An existing spend line that jumped past both gates.
    Spike,
    /// A dimension with ~no trailing baseline that started spending materially.
    NewSpend,
}

#[derive(Clone, Debug)]
pub struct Anomaly {
    /// Service or account the anomaly is on.
    pub key: String,
    /// Spike-robust trailing baseline (median of the prior days), dollars/day.
    pub expected: f64,
    /// The evaluated day's cost, dollars.
    pub latest: f64,
    /// `latest - expected` (always positive for a reported anomaly).
    pub delta: f64,
    /// Percent increase over baseline. `f64::INFINITY` for `NewSpend`.
    pub pct: f64,
    pub kind: AnomalyKind,
}

/// Median of a slice (spike-robust, unlike the mean — a single prior one-off day
/// doesn't inflate the baseline and mask a genuine jump). Empty slice ⇒ 0.
fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    }
}

/// Detect anomalies on the evaluated day for each dimension.
///
/// `series` maps a dimension key (service / account) to its daily amounts, all
/// vectors the same length `n` and aligned by date, with index `n-1` the day
/// being evaluated (the caller passes the most recent *complete* day — never the
/// running, partial "today"). The baseline is the median of days `0..n-1`.
///
/// Results are sorted by absolute dollar delta, largest first.
pub fn detect(series: &BTreeMap<String, Vec<f64>>, t: &Thresholds) -> Vec<Anomaly> {
    let mut out = Vec::new();
    for (key, vals) in series {
        // Need at least one baseline day plus the evaluated day.
        if vals.len() < 2 {
            continue;
        }
        let latest = *vals.last().unwrap();
        let baseline = &vals[..vals.len() - 1];
        let expected = median(baseline);
        let delta = latest - expected;

        // Absolute floor: filters noise and every decrease in one comparison.
        if delta < t.min_dollars {
            continue;
        }

        if expected < NEW_SPEND_BASELINE_EPS {
            out.push(Anomaly {
                key: key.clone(),
                expected,
                latest,
                delta,
                pct: f64::INFINITY,
                kind: AnomalyKind::NewSpend,
            });
        } else {
            let pct = delta / expected * 100.0;
            if pct >= t.pct {
                out.push(Anomaly {
                    key: key.clone(),
                    expected,
                    latest,
                    delta,
                    pct,
                    kind: AnomalyKind::Spike,
                });
            }
        }
    }
    out.sort_by(|a, b| b.delta.partial_cmp(&a.delta).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// One labeled group of anomalies in a digest (e.g. all the service anomalies).
pub struct Section<'a> {
    pub label: &'a str,
    pub anomalies: &'a [Anomaly],
}

fn money(v: f64) -> String {
    format!("${v:.2}")
}

fn line(a: &Anomaly) -> String {
    match a.kind {
        AnomalyKind::NewSpend => {
            format!("  • {}: new spend {}/day  (no recent baseline)", a.key, money(a.latest))
        }
        AnomalyKind::Spike => format!(
            "  • {}: {}/day → {}/day  (+{}, +{:.0}%)",
            a.key,
            money(a.expected),
            money(a.latest),
            money(a.delta),
            a.pct
        ),
    }
}

/// Render the alert email for a day's anomalies. Returns `None` when every
/// section is empty — the caller sends nothing, so a quiet day is a silent one.
pub fn render_digest(date: &str, app_url: &str, sections: &[Section]) -> Option<(String, String)> {
    let n: usize = sections.iter().map(|s| s.anomalies.len()).sum();
    if n == 0 {
        return None;
    }
    let noun = if n == 1 { "anomaly" } else { "anomalies" };
    let subject = format!("manifest: {n} cost {noun} on {date}");

    let mut body = format!(
        "manifest detected {n} spend {noun} for {date}, comparing each day's cost \
         against a spike-robust trailing baseline.\n"
    );
    for s in sections {
        if s.anomalies.is_empty() {
            continue;
        }
        body.push_str(&format!("\n{}\n", s.label));
        for a in s.anomalies {
            body.push_str(&line(a));
            body.push('\n');
        }
    }
    if !app_url.is_empty() {
        body.push_str(&format!("\nDashboard: {app_url}\n"));
    }
    body.push_str(
        "\nThis is manifest's own detector, independent of AWS Cost Anomaly Detection.\n",
    );
    Some((subject, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(pairs: &[(&str, &[f64])]) -> BTreeMap<String, Vec<f64>> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect()
    }

    #[test]
    fn steady_spend_is_not_an_anomaly() {
        let s = series(&[("EC2", &[10.0, 10.0, 9.5, 10.5, 10.0])]);
        assert!(detect(&s, &Thresholds::default()).is_empty());
    }

    #[test]
    fn a_clear_spike_fires() {
        // Baseline median 10, today 25: +$15 (≥$5) and +150% (≥50%).
        let s = series(&[("EC2", &[10.0, 10.0, 9.5, 10.5, 25.0])]);
        let a = detect(&s, &Thresholds::default());
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].key, "EC2");
        assert_eq!(a[0].kind, AnomalyKind::Spike);
        assert!((a[0].expected - 10.0).abs() < 1e-9);
        assert!((a[0].delta - 15.0).abs() < 1e-9);
    }

    #[test]
    fn brand_new_spend_fires_as_new_spend() {
        // Nothing, then $8/day. No baseline to take a percentage of.
        let s = series(&[("Bedrock", &[0.0, 0.0, 0.0, 0.0, 8.0])]);
        let a = detect(&s, &Thresholds::default());
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, AnomalyKind::NewSpend);
        assert!(a[0].pct.is_infinite());
    }

    #[test]
    fn big_percentage_but_tiny_dollars_is_suppressed() {
        // $0.02 → $0.10 is +400% but only +$0.08 — below the $5 floor.
        let s = series(&[("SNS", &[0.02, 0.02, 0.02, 0.02, 0.10])]);
        assert!(detect(&s, &Thresholds::default()).is_empty());
    }

    #[test]
    fn big_dollars_but_small_percentage_is_suppressed() {
        // $1000 → $1030 is +$30 (≥$5) but only +3% — below the 50% gate.
        let s = series(&[("EC2", &[1000.0, 1000.0, 1000.0, 1000.0, 1030.0])]);
        assert!(detect(&s, &Thresholds::default()).is_empty());
    }

    #[test]
    fn a_decrease_never_fires() {
        let s = series(&[("EC2", &[50.0, 50.0, 50.0, 50.0, 5.0])]);
        assert!(detect(&s, &Thresholds::default()).is_empty());
    }

    #[test]
    fn a_prior_one_off_does_not_mask_a_new_spike() {
        // One $80 day in the baseline; the median stays at 10, so a return to 30
        // today still reads as a spike (a mean baseline would have hidden it).
        let s = series(&[("EC2", &[10.0, 80.0, 10.0, 10.0, 30.0])]);
        let a = detect(&s, &Thresholds::default());
        assert_eq!(a.len(), 1);
        assert!((a[0].expected - 10.0).abs() < 1e-9);
    }

    #[test]
    fn results_are_sorted_by_dollar_delta_desc() {
        let s = series(&[
            ("EC2", &[10.0, 10.0, 10.0, 40.0]),   // +$30
            ("S3", &[5.0, 5.0, 5.0, 100.0]),      // +$95
            ("RDS", &[20.0, 20.0, 20.0, 60.0]),   // +$40
        ]);
        let a = detect(&s, &Thresholds::default());
        let keys: Vec<&str> = a.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, vec!["S3", "RDS", "EC2"]);
    }

    #[test]
    fn thresholds_are_honored() {
        let s = series(&[("EC2", &[10.0, 10.0, 10.0, 13.0])]); // +$3, +30%
        // Default ($5 / 50%) suppresses it; a looser policy catches it.
        assert!(detect(&s, &Thresholds::default()).is_empty());
        let loose = Thresholds { min_dollars: 1.0, pct: 20.0 };
        assert_eq!(detect(&s, &loose).len(), 1);
    }

    #[test]
    fn too_short_a_series_is_skipped() {
        let s = series(&[("EC2", &[42.0])]);
        assert!(detect(&s, &Thresholds::default()).is_empty());
    }

    #[test]
    fn empty_digest_renders_nothing() {
        let sections = [Section { label: "SERVICE", anomalies: &[] }];
        assert!(render_digest("2026-07-21", "https://m.example.com", &sections).is_none());
    }

    #[test]
    fn digest_renders_subject_and_both_anomaly_kinds() {
        let spike = Anomaly {
            key: "Amazon EC2".into(),
            expected: 10.0,
            latest: 25.0,
            delta: 15.0,
            pct: 150.0,
            kind: AnomalyKind::Spike,
        };
        let new = Anomaly {
            key: "Amazon Bedrock".into(),
            expected: 0.0,
            latest: 8.0,
            delta: 8.0,
            pct: f64::INFINITY,
            kind: AnomalyKind::NewSpend,
        };
        let svc = [spike, new];
        let sections = [Section { label: "SERVICE", anomalies: &svc }];
        let (subject, body) =
            render_digest("2026-07-21", "https://m.example.com", &sections).unwrap();
        assert_eq!(subject, "manifest: 2 cost anomalies on 2026-07-21");
        assert!(body.contains("Amazon EC2: $10.00/day → $25.00/day  (+$15.00, +150%)"));
        assert!(body.contains("Amazon Bedrock: new spend $8.00/day"));
        assert!(body.contains("Dashboard: https://m.example.com"));
        assert!(body.contains("independent of AWS Cost Anomaly Detection"));
    }
}
