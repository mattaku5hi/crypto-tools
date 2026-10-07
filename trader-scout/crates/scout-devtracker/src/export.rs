//! Wallet-tracker exports of the derived lists (B7).
//!
//! One file per category, chain and terminal format, best-ranked devs first.
//! Formats (pinned 2026-10-07 from the owner's own exports, see
//! docs/DEV-TRACKER.md §4):
//! - `gmgn`: JSON array of `{"address", "name", "emoji"}` (GMGN docs, "Wallets
//!   Import Export"; at most 2,000 tracked wallets);
//! - `axiom`: JSON array shaped like an Axiom wallet-tracker export:
//!   `trackedWalletAddress`, `name`, `emoji`, `createdAt`, the five alert flags,
//!   `groupNames`, `sound`, `transferAudio`, `highlightColor`;
//! - `basedbot`: text, one wallet per line, tab-separated
//!   `address \t emoji \t name \t group` (BasedBot wallet-tracker export).
//!
//! A list "changes" when its set of wallets changes ([`ExportFile::members_hash`]);
//! labels carry live stats and change more often, without triggering a send.
//! [`change_report`] says what changed and why. Creators still behind an
//! unresolved shared intermediary (`contract:…`) are not wallets and are never
//! exported.

use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{Category, DevStats, DevTrackerConfig, DevVerdict};

/// Group every exported wallet is filed under in the trackers.
pub const TRACKER_GROUP: &str = "Devs";

/// A terminal import format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Gmgn,
    Axiom,
    BasedBot,
}

impl Format {
    /// Parse a config name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "gmgn" => Some(Self::Gmgn),
            "axiom" => Some(Self::Axiom),
            "basedbot" => Some(Self::BasedBot),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Gmgn => "gmgn",
            Self::Axiom => "axiom",
            Self::BasedBot => "basedbot",
        }
    }

    const fn extension(self) -> &'static str {
        match self {
            Self::Gmgn | Self::Axiom => "json",
            Self::BasedBot => "txt",
        }
    }
}

/// One rendered file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFile {
    pub category: Category,
    pub chain: String,
    pub format: Format,
    /// Stable name for the export directory: `top-runners_solana_gmgn.json`.
    pub file_name: String,
    pub content: Vec<u8>,
    /// Wallets in rank order.
    pub members: Vec<String>,
    /// sha256 (hex) of the sorted wallet addresses: the change detector.
    pub members_hash: String,
}

impl ExportFile {
    /// Delivery key (`deliveries.category`): `top-runners:solana`.
    #[must_use]
    pub fn delivery_key(&self) -> String {
        format!("{}:{}", self.category.label(), self.chain)
    }

    /// Name of a sent copy: `top-migr_solana_2026-10-07T18-30Z_axiom.json`.
    #[must_use]
    pub fn timestamped_name(&self, now: i64) -> String {
        format!(
            "{}_{}_{}_{}.{}",
            self.category.label(),
            self.chain,
            utc_stamp(now, false),
            self.format.name(),
            self.format.extension()
        )
    }
}

/// Civil date of a unix day (proleptic Gregorian, H. Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe.div_euclid(1_460) + doe.div_euclid(36_524) - doe.div_euclid(146_096))
        .div_euclid(365);
    let doy = doe - (365 * yoe + yoe.div_euclid(4) - yoe.div_euclid(100));
    let mp = (5 * doy + 2).div_euclid(153);
    let d = doy - (153 * mp + 2).div_euclid(5) + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `2026-10-07T18-30Z` (file names) or `2026-10-07T18:30:05.000Z` (ISO).
fn utc_stamp(unix: i64, iso: bool) -> String {
    let (y, mo, d) = civil(unix.div_euclid(86_400));
    let s = unix.rem_euclid(86_400);
    let (h, mi, sec) = (s.div_euclid(3_600), (s % 3_600).div_euclid(60), s % 60);
    if iso {
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}.000Z")
    } else {
        format!("{y:04}-{mo:02}-{d:02}T{h:02}-{mi:02}Z")
    }
}

const fn emoji(c: Category) -> &'static str {
    match c {
        Category::TopRunners => "🚀",
        Category::TopMigr => "🎓",
        Category::WinStreak => "🔥",
    }
}

const fn abbr(c: Category) -> &'static str {
    match c {
        Category::TopRunners => "TR",
        Category::TopMigr => "TM",
        Category::WinStreak => "WS",
    }
}

/// Short label: `TR m5/40 s1 r3` (migrated / resolved, streak, runners).
#[must_use]
pub fn label(c: Category, s: &DevStats) -> String {
    format!(
        "{} m{}/{} s{} r{}",
        abbr(c),
        s.migrated,
        s.resolved,
        s.current_streak,
        s.runners
    )
}

/// Ranking key of a member of `c` (smaller sorts first).
fn rank_key(c: Category, s: &DevStats) -> (Reverse<u32>, Reverse<u32>, Reverse<u32>, Reverse<i64>) {
    let rate = s.migration_rate_bp.unwrap_or(0);
    match c {
        Category::TopRunners => (
            Reverse(s.big_runners),
            Reverse(s.runners),
            Reverse(rate),
            Reverse(s.last_launch_at),
        ),
        Category::TopMigr => (
            Reverse(rate),
            Reverse(s.migrated),
            Reverse(s.current_streak),
            Reverse(s.last_launch_at),
        ),
        Category::WinStreak => (
            Reverse(s.current_streak),
            Reverse(s.migrated),
            Reverse(rate),
            Reverse(s.last_launch_at),
        ),
    }
}

/// Members of `category` on `chain`, best first, at most `max`.
#[must_use]
pub fn members<'a>(
    verdicts: &'a [DevVerdict],
    category: Category,
    chain: &str,
    max: usize,
) -> Vec<&'a DevStats> {
    let mut m: Vec<&DevStats> = verdicts
        .iter()
        .filter(|v| v.stats.chain == chain && v.categories.contains(&category))
        .map(|v| &v.stats)
        .filter(|s| !s.creator.starts_with("contract:"))
        .collect();
    m.sort_by(|a, b| {
        rank_key(category, a)
            .cmp(&rank_key(category, b))
            .then_with(|| a.creator.cmp(&b.creator))
    });
    m.truncate(max);
    m
}

/// Render one file (`now` stamps Axiom's `createdAt`).
#[must_use]
pub fn render(
    verdicts: &[DevVerdict],
    category: Category,
    chain: &str,
    format: Format,
    max: usize,
    now: i64,
) -> ExportFile {
    let m = members(verdicts, category, chain, max);
    let e = emoji(category);
    let content = match format {
        Format::Gmgn => {
            let rows: Vec<_> = m
                .iter()
                .map(|s| json!({"address": s.creator, "name": label(category, s), "emoji": e}))
                .collect();
            serde_json::to_vec_pretty(&rows).unwrap_or_default()
        }
        Format::Axiom => {
            let created = utc_stamp(now, true);
            let rows: Vec<_> = m
                .iter()
                .map(|s| {
                    json!({
                        "trackedWalletAddress": s.creator,
                        "name": label(category, s),
                        "emoji": e,
                        "createdAt": created,
                        "alertsOnToast": false,
                        "alertsOnBubble": true,
                        "alertsOnFeed": true,
                        "alertsOnTransfer": true,
                        "toastOnTransfer": false,
                        "groupNames": [TRACKER_GROUP, category.label()],
                        "sound": "",
                        "transferAudio": "",
                        "highlightColor": null,
                    })
                })
                .collect();
            serde_json::to_vec(&rows).unwrap_or_default()
        }
        Format::BasedBot => m
            .iter()
            .map(|s| {
                format!(
                    "{}\t{e}\t{}\t{TRACKER_GROUP}\n",
                    s.creator,
                    label(category, s)
                )
            })
            .collect::<String>()
            .into_bytes(),
    };
    let members: Vec<String> = m.iter().map(|s| s.creator.clone()).collect();
    ExportFile {
        category,
        chain: chain.to_string(),
        format,
        file_name: format!(
            "{}_{chain}_{}.{}",
            category.label(),
            format.name(),
            format.extension()
        ),
        content,
        members_hash: members_hash(&members),
        members,
    }
}

/// sha256 (hex) of the sorted addresses.
#[must_use]
pub fn members_hash(members: &[String]) -> String {
    let mut sorted: Vec<&str> = members.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    Sha256::digest(sorted.join("\n").as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn short(addr: &str) -> String {
    let n = addr.chars().count();
    if n <= 12 {
        return addr.to_string();
    }
    let head: String = addr.chars().take(6).collect();
    let tail: String = addr.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

fn pct(bp: Option<u32>) -> String {
    bp.map_or_else(|| "–".to_string(), |b| format!("{}%", b.div_euclid(100)))
}

/// One line of stats: `m5/6 83% · streak 3 · runners 1 (1 big)`.
fn stats_line(s: &DevStats) -> String {
    format!(
        "m{}/{} {} · streak {} · runners {} ({} big)",
        s.migrated,
        s.resolved,
        pct(s.migration_rate_bp),
        s.current_streak,
        s.runners,
        s.big_runners
    )
}

/// Why a dev is not (any more) in `category`: the first rule it fails.
#[must_use]
pub fn why_not(
    category: Category,
    v: Option<&DevVerdict>,
    cfg: &DevTrackerConfig,
    now: i64,
    max: usize,
) -> String {
    let Some(v) = v else {
        return format!("no launch in the last {} days", cfg.schedule.since_days);
    };
    let s = &v.stats;
    if v.categories.contains(&category) {
        return format!("still qualifies, ranked below the top {max}");
    }
    let idle_days = now.saturating_sub(s.last_launch_at).div_euclid(86_400);
    let inactive = |max_days: i64| idle_days > max_days;
    let rate = s.migration_rate_bp.unwrap_or(0);
    let bp = |p: u32| p.saturating_mul(100);
    match category {
        Category::TopRunners => {
            let tr = &cfg.top_runners;
            if s.big_runners < tr.min_big_runners {
                format!("big runners {} < {}", s.big_runners, tr.min_big_runners)
            } else if s.runners.saturating_sub(tr.min_big_runners) < tr.min_other_runners {
                format!("runners {} too few", s.runners)
            } else if s.curve_launches > 0 && rate < bp(tr.min_migration_rate_pct) {
                format!(
                    "rate {} < {}%",
                    pct(s.migration_rate_bp),
                    tr.min_migration_rate_pct
                )
            } else if inactive(tr.max_inactive_days) {
                format!("inactive {idle_days} d")
            } else {
                "no longer qualifies".to_string()
            }
        }
        Category::TopMigr => {
            let tm = &cfg.top_migr;
            if s.resolved < tm.min_launches {
                format!("resolved launches {} < {}", s.resolved, tm.min_launches)
            } else if rate < bp(tm.min_migration_rate_pct) {
                format!(
                    "rate {} < {}%",
                    pct(s.migration_rate_bp),
                    tm.min_migration_rate_pct
                )
            } else if inactive(tm.max_inactive_days) {
                format!("inactive {idle_days} d")
            } else {
                "no longer qualifies".to_string()
            }
        }
        Category::WinStreak => {
            let ws = &cfg.win_streak;
            if let Some(other) = v.categories.first() {
                format!("moved to {}", other.label())
            } else if s.current_streak < ws.min_streak {
                format!("streak broken ({} < {})", s.current_streak, ws.min_streak)
            } else if inactive(ws.max_inactive_days) {
                format!("inactive {idle_days} d")
            } else {
                "no longer qualifies".to_string()
            }
        }
    }
}

/// Human report of one list's change, split into Telegram-sized parts: the
/// first part fits a document caption (1,024 chars), the rest are messages
/// (4,096 chars). `prev = None` = first delivery of this list.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn change_report(
    file: &ExportFile,
    prev: Option<&[String]>,
    verdicts: &[DevVerdict],
    cfg: &DevTrackerConfig,
    now: i64,
    max: usize,
    test: bool,
) -> Vec<String> {
    let cat = file.category;
    let find = |addr: &str| {
        verdicts
            .iter()
            .find(|v| v.stats.chain == file.chain && v.stats.creator == addr)
    };
    let cur: BTreeSet<&str> = file.members.iter().map(String::as_str).collect();
    let old: BTreeSet<&str> = prev
        .unwrap_or_default()
        .iter()
        .map(String::as_str)
        .collect();
    let added: Vec<&str> = file
        .members
        .iter()
        .map(String::as_str)
        .filter(|a| !old.contains(a))
        .collect();
    let dropped: Vec<&str> = prev
        .unwrap_or_default()
        .iter()
        .map(String::as_str)
        .filter(|a| !cur.contains(a))
        .collect();
    let mut head = String::new();
    if test {
        head.push_str("[TEST — not recorded]\n");
    }
    let _ = write!(
        head,
        "{} {} · {} · {}\n{} wallet(s)",
        emoji(cat),
        cat.label(),
        file.chain,
        utc_stamp(now, false),
        file.members.len()
    );
    if prev.is_some() {
        let _ = write!(head, " (+{} / −{})", added.len(), dropped.len());
    } else {
        head.push_str(" (first list)");
    }
    let mut lines = Vec::new();
    for a in &added {
        let detail = find(a).map_or_else(String::new, |v| stats_line(&v.stats));
        lines.push(format!("+ {} {detail}", short(a)));
    }
    for a in &dropped {
        lines.push(format!(
            "− {} {}",
            short(a),
            why_not(cat, find(a), cfg, now, max)
        ));
    }
    let mut parts = vec![head];
    for line in lines {
        let limit = if parts.len() == 1 { 1_024 } else { 4_096 };
        let last = parts.last_mut().map_or(0, |p| p.chars().count());
        if last + 1 + line.chars().count() <= limit {
            if let Some(p) = parts.last_mut() {
                p.push('\n');
                p.push_str(&line);
            }
        } else {
            parts.push(line);
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    const NOW: i64 = 1_791_400_000; // 2026-10-07T…Z

    fn cfg() -> DevTrackerConfig {
        DevTrackerConfig::from_toml(include_str!("../../../config/dev-tracker.example.toml"))
            .unwrap()
    }

    fn stats(creator: &str, migrated: u32, resolved: u32, streak: u32, runners: u32) -> DevStats {
        DevStats {
            chain: "solana".to_string(),
            creator: creator.to_string(),
            launches: resolved,
            curve_launches: resolved,
            migrated,
            resolved,
            pending: 0,
            migration_rate_bp: (resolved > 0)
                .then(|| (migrated * 10_000).checked_div(resolved).unwrap()),
            current_streak: streak,
            runners,
            big_runners: u32::from(runners > 2),
            last_launch_at: NOW - 86_400,
        }
    }

    fn verdicts() -> Vec<DevVerdict> {
        vec![
            DevVerdict {
                stats: stats("B", 4, 5, 2, 0),
                categories: vec![Category::TopMigr],
            },
            DevVerdict {
                stats: stats("A", 9, 10, 9, 3),
                categories: vec![Category::TopMigr, Category::TopRunners],
            },
            DevVerdict {
                stats: stats("contract:0xabc", 9, 9, 9, 0),
                categories: vec![Category::TopMigr],
            },
            DevVerdict {
                stats: stats("C", 1, 50, 0, 0),
                categories: vec![],
            },
        ]
    }

    #[test]
    fn stamps_are_utc() {
        assert_eq!(utc_stamp(0, true), "1970-01-01T00:00:00.000Z");
        assert_eq!(utc_stamp(1_782_475_430, false), "2026-06-26T12-03Z");
        assert_eq!(utc_stamp(951_782_400, false), "2000-02-29T00-00Z");
    }

    #[test]
    fn gmgn_and_axiom_shapes() {
        let v = verdicts();
        let f = render(&v, Category::TopMigr, "solana", Format::Gmgn, 10, NOW);
        assert_eq!(f.file_name, "top-migr_solana_gmgn.json");
        assert!(
            f.timestamped_name(NOW)
                .starts_with("top-migr_solana_2026-10-0")
        );
        assert!(f.timestamped_name(NOW).ends_with("Z_gmgn.json"));
        let j: serde_json::Value = serde_json::from_slice(&f.content).unwrap();
        assert_eq!(
            f.members,
            ["A", "B"],
            "unresolved intermediaries are not wallets"
        );
        assert_eq!(j[0]["address"], "A", "90 % before 80 %");
        assert_eq!(j[0]["name"], "TM m9/10 s9 r3");
        assert_eq!(j[0]["emoji"], "🎓");
        let a = render(&v, Category::TopMigr, "solana", Format::Axiom, 1, NOW);
        let j: serde_json::Value = serde_json::from_slice(&a.content).unwrap();
        assert_eq!(j.as_array().unwrap().len(), 1, "max wallets per file");
        let keys: Vec<&str> = j[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        // the owner's Axiom export (2026-10-07) has exactly these keys
        let mut want = vec![
            "trackedWalletAddress",
            "name",
            "emoji",
            "createdAt",
            "alertsOnToast",
            "alertsOnBubble",
            "alertsOnFeed",
            "alertsOnTransfer",
            "toastOnTransfer",
            "groupNames",
            "sound",
            "transferAudio",
            "highlightColor",
        ];
        want.sort_unstable();
        let mut keys = keys;
        keys.sort_unstable();
        assert_eq!(keys, want);
        assert_eq!(j[0]["groupNames"], json!(["Devs", "top-migr"]));
        assert!(j[0]["highlightColor"].is_null());
    }

    #[test]
    fn basedbot_tsv_and_membership_hash() {
        let v = verdicts();
        let b = render(&v, Category::TopMigr, "solana", Format::BasedBot, 10, NOW);
        assert_eq!(
            String::from_utf8(b.content.clone()).unwrap(),
            "A\t🎓\tTM m9/10 s9 r3\tDevs\nB\t🎓\tTM m4/5 s2 r0\tDevs\n"
        );
        let g = render(&v, Category::TopMigr, "solana", Format::Gmgn, 10, NOW);
        assert_eq!(
            b.members_hash, g.members_hash,
            "the hash is membership only"
        );
        let mut changed = v.clone();
        changed[0].stats.migrated = 5; // label changes, members do not
        assert_eq!(
            render(&changed, Category::TopMigr, "solana", Format::Gmgn, 10, NOW).members_hash,
            g.members_hash
        );
        changed[0].categories.clear();
        assert_ne!(
            render(&changed, Category::TopMigr, "solana", Format::Gmgn, 10, NOW).members_hash,
            g.members_hash
        );
        let empty = render(&v, Category::WinStreak, "bsc", Format::Gmgn, 10, NOW);
        assert!(empty.members.is_empty());
        assert_eq!(empty.content, b"[]");
    }

    #[test]
    fn change_report_lists_additions_and_reasons() {
        let mut v = verdicts();
        let f = render(&v, Category::TopMigr, "solana", Format::Gmgn, 10, NOW);
        let first = change_report(&f, None, &v, &cfg(), NOW, 10, true);
        assert!(first[0].starts_with("[TEST"));
        assert!(
            first[0].contains("2 wallet(s) (first list)"),
            "{}",
            first[0]
        );
        assert!(first[0].contains("+ A m9/10 90%"), "{}", first[0]);
        // B drops (rate fell), C was in the previous list and still fails
        v[0].stats.migrated = 3;
        v[0].stats.migration_rate_bp = Some(6_000);
        v[0].categories.clear();
        let f2 = render(&v, Category::TopMigr, "solana", Format::Gmgn, 10, NOW);
        let prev = vec![
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
            "D".to_string(),
        ];
        let r = change_report(&f2, Some(prev.as_slice()), &v, &cfg(), NOW, 10, false);
        let text = r.join("\n");
        assert!(text.contains("1 wallet(s) (+0 / −3)"), "{text}");
        assert!(text.contains("− B rate 60% < 80%"), "{text}");
        assert!(text.contains("− C rate 2% < 80%"), "{text}");
        assert!(
            text.contains("− D no launch in the last 365 days"),
            "{text}"
        );
    }

    #[test]
    fn long_reports_split_at_telegram_limits() {
        let v: Vec<DevVerdict> = (0..300)
            .map(|i| DevVerdict {
                stats: stats(
                    &format!("Wallet{i:04}xxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
                    9,
                    10,
                    9,
                    0,
                ),
                categories: vec![Category::TopMigr],
            })
            .collect();
        let f = render(&v, Category::TopMigr, "solana", Format::Gmgn, 500, NOW);
        let parts = change_report(&f, Some(&[][..]), &v, &cfg(), NOW, 500, false);
        assert!(parts.len() > 2);
        assert!(parts[0].chars().count() <= 1_024);
        assert!(parts[1..].iter().all(|p| p.chars().count() <= 4_096));
        assert_eq!(
            parts
                .iter()
                .map(|p| p.matches("\n+ ").count() + usize::from(p.starts_with("+ ")))
                .sum::<usize>(),
            300
        );
    }
}
