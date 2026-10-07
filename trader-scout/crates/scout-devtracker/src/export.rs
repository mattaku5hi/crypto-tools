//! Wallet-tracker exports of the derived lists (B7).
//!
//! One file per category, chain and terminal format, best-ranked devs first.
//! Formats (pinned 2026-10-07, see docs/DEV-TRACKER.md §4):
//! - `gmgn`: JSON array of `{"address", "name", "emoji"}` (GMGN docs, "Wallets
//!   Import Export"; at most 2,000 tracked wallets);
//! - `axiom`: JSON array of `{"trackedWalletAddress", "name", "emoji",
//!   "alertsOn"}`, all four required (Axiom import, third-party guides);
//! - `basedbot`: text, one wallet per line `address  emoji  name` (BasedBot's
//!   plain-text import; it also reads GMGN JSON).
//!
//! A list "changes" when its set of wallets changes ([`ExportFile::members_hash`]);
//! labels carry live stats and change more often, without triggering a send.
//! Creators still behind an unresolved shared intermediary (`contract:…`) are
//! not wallets and are never exported.

use std::cmp::Reverse;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{Category, DevStats, DevVerdict};

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
    /// `top-runners_solana_gmgn.json`.
    pub file_name: String,
    pub content: Vec<u8>,
    pub wallets: usize,
    /// sha256 (hex) of the sorted wallet addresses: the change detector.
    pub members_hash: String,
}

impl ExportFile {
    /// Delivery key (`deliveries.category`): `top-runners:solana`.
    #[must_use]
    pub fn delivery_key(&self) -> String {
        format!("{}:{}", self.category.label(), self.chain)
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

/// Render one file.
#[must_use]
pub fn render(
    verdicts: &[DevVerdict],
    category: Category,
    chain: &str,
    format: Format,
    max: usize,
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
            let rows: Vec<_> = m
                .iter()
                .map(|s| {
                    json!({
                        "trackedWalletAddress": s.creator,
                        "name": label(category, s),
                        "emoji": e,
                        "alertsOn": true,
                    })
                })
                .collect();
            serde_json::to_vec_pretty(&rows).unwrap_or_default()
        }
        Format::BasedBot => m
            .iter()
            .map(|s| format!("{}  {e}  {}\n", s.creator, label(category, s)))
            .collect::<String>()
            .into_bytes(),
    };
    let mut sorted: Vec<&str> = m.iter().map(|s| s.creator.as_str()).collect();
    sorted.sort_unstable();
    let members_hash = Sha256::digest(sorted.join("\n").as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
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
        wallets: m.len(),
        members_hash,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

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
            last_launch_at: 1_000,
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
    fn gmgn_and_axiom_shapes() {
        let v = verdicts();
        let f = render(&v, Category::TopMigr, "solana", Format::Gmgn, 10);
        assert_eq!(f.file_name, "top-migr_solana_gmgn.json");
        let j: serde_json::Value = serde_json::from_slice(&f.content).unwrap();
        assert_eq!(f.wallets, 2, "unresolved intermediaries are not wallets");
        assert_eq!(j[0]["address"], "A", "90 % before 80 %");
        assert_eq!(j[0]["name"], "TM m9/10 s9 r3");
        assert_eq!(j[0]["emoji"], "🎓");
        assert_eq!(j[1]["address"], "B");
        let a = render(&v, Category::TopMigr, "solana", Format::Axiom, 1);
        let j: serde_json::Value = serde_json::from_slice(&a.content).unwrap();
        assert_eq!(j.as_array().unwrap().len(), 1, "max wallets per file");
        assert_eq!(j[0]["trackedWalletAddress"], "A");
        assert_eq!(j[0]["alertsOn"], true);
    }

    #[test]
    fn basedbot_lines_and_membership_hash() {
        let v = verdicts();
        let b = render(&v, Category::TopMigr, "solana", Format::BasedBot, 10);
        assert_eq!(
            String::from_utf8(b.content.clone()).unwrap(),
            "A  🎓  TM m9/10 s9 r3\nB  🎓  TM m4/5 s2 r0\n"
        );
        let g = render(&v, Category::TopMigr, "solana", Format::Gmgn, 10);
        assert_eq!(
            b.members_hash, g.members_hash,
            "the hash is membership only"
        );
        let mut changed = v.clone();
        changed[0].stats.migrated = 5; // label changes, members do not
        assert_eq!(
            render(&changed, Category::TopMigr, "solana", Format::Gmgn, 10).members_hash,
            g.members_hash
        );
        changed[0].categories.clear();
        assert_ne!(
            render(&changed, Category::TopMigr, "solana", Format::Gmgn, 10).members_hash,
            g.members_hash
        );
        let empty = render(&v, Category::WinStreak, "bsc", Format::Gmgn, 10);
        assert_eq!(empty.wallets, 0);
        assert_eq!(empty.content, b"[]");
    }
}
