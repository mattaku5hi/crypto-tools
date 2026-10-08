//! Dev tracker: configuration and the category derivation (docs/DEV-TRACKER.md,
//! ADR-021).
//!
//! Categories are derived from stored facts on every cycle, never stored as
//! verdicts, so a config edit takes effect on the next cycle without a rescan.
//! All arithmetic is integer: money in USD cents, rates in basis points.

pub mod ath;
pub mod evm_ingest;
pub mod export;
pub mod identity;
pub mod solana_fallback;
pub mod solana_ingest;
pub mod telegram;

use std::collections::BTreeMap;

use scout_devdb::DevLaunchRow;
use serde::Deserialize;

const DAY: i64 = 86_400;

/// `top_runners` thresholds.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopRunnersConfig {
    pub runner_ath_usd: i64,
    pub big_runner_ath_usd: i64,
    pub min_big_runners: u32,
    pub min_other_runners: u32,
    /// ATH above this never makes a runner (manipulated price on a huge supply).
    pub max_plausible_ath_usd: i64,
    /// A runner needs at least this many holders (Codex, at observation time):
    /// one tiny trade in an empty pool can show a multi-million ATH (live:
    /// Zora coins with 1–8 holders at $1.4M–$99M).
    #[serde(default = "default_min_holders")]
    pub min_holders: i64,
    /// Launchpads whose ATH FDV can be an artifact of a huge supply priced by
    /// one tiny trade (live: Bankr tokens at $380M–$6B with $0–$3.5k
    /// liquidity): there a runner also needs the highest liquidity seen to be
    /// at least `min_liquidity_bp_of_ath` of its ATH.
    #[serde(default = "default_liquidity_checked")]
    pub liquidity_checked_launchpads: Vec<String>,
    /// Basis points of the ATH (100 = 1 %).
    #[serde(default = "default_min_liquidity_bp")]
    pub min_liquidity_bp_of_ath: i64,
    pub min_migration_rate_pct: u32,
    pub max_inactive_days: i64,
}

const fn default_min_holders() -> i64 {
    100
}

fn default_liquidity_checked() -> Vec<String> {
    vec!["bankr".to_string(), "noice".to_string()]
}

const fn default_min_liquidity_bp() -> i64 {
    100
}

/// `top_migr` thresholds.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopMigrConfig {
    pub min_migration_rate_pct: u32,
    pub min_launches: u32,
    pub max_inactive_days: i64,
}

/// `win_streak` thresholds.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WinStreakConfig {
    pub min_streak: u32,
    pub max_inactive_days: i64,
}

/// Daemon cadence and per-pass budgets (`[schedule]`, all optional).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScheduleConfig {
    /// Chains ingested by the daemon.
    pub chains: Vec<String>,
    /// Minutes between ingestion passes (launches, migrations, identities, ATH).
    pub ingest_every_minutes: u64,
    /// Minutes between category derivations (and deliveries).
    pub derive_every_minutes: u64,
    /// Where a source without a cursor starts.
    pub start_hours_back: u64,
    /// Codex requests (200 tokens each) per chain and pass.
    pub ath_requests_per_chain: usize,
    /// Helius pages (≤ 1000 txs) per Solana source and pass.
    pub solana_max_pages: u32,
    /// New EVM creators classified per chain and pass.
    pub max_new_creators: i64,
    /// Signers of launches through shared intermediaries resolved per chain and pass.
    pub max_signer_lookups: i64,
    /// Candidate devs whose history through intermediaries is fetched per chain and pass.
    pub max_dev_histories: i64,
    /// Launches older than this many days are not loaded for the derivation.
    pub since_days: i64,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            chains: ["solana", "bsc", "base", "robinhood"]
                .map(String::from)
                .to_vec(),
            ingest_every_minutes: 30,
            derive_every_minutes: 60,
            start_hours_back: 24,
            ath_requests_per_chain: 20,
            solana_max_pages: 200,
            max_new_creators: 2_000,
            max_signer_lookups: 5_000,
            max_dev_histories: 500,
            since_days: 365,
        }
    }
}

/// Exports and their delivery (`[delivery]`, all optional).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DeliveryConfig {
    /// Terminal formats: `gmgn`, `axiom`, `basedbot` (see `export`).
    pub formats: Vec<String>,
    /// Chains exported (one file per category, chain and format).
    pub chains: Vec<String>,
    /// Best-ranked wallets kept per file (GMGN tracks at most 2,000).
    pub max_wallets_per_file: usize,
    /// Directory the current files are written to (empty = none).
    pub out_dir: String,
    /// Send changed files to Telegram (`SCOUT_TELEGRAM_BOT_TOKEN`,
    /// `SCOUT_TELEGRAM_CHAT_ID`).
    pub telegram: bool,
}

impl Default for DeliveryConfig {
    fn default() -> Self {
        Self {
            formats: ["gmgn", "axiom", "basedbot"].map(String::from).to_vec(),
            chains: ["solana", "bsc", "base", "robinhood"]
                .map(String::from)
                .to_vec(),
            max_wallets_per_file: 500,
            out_dir: "exports".to_string(),
            telegram: true,
        }
    }
}

/// The whole dev tracker config (`config/dev-tracker.example.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevTrackerConfig {
    pub pending_window_days: i64,
    #[serde(default)]
    pub launchpads_without_migration: Vec<String>,
    pub top_runners: TopRunnersConfig,
    pub top_migr: TopMigrConfig,
    pub win_streak: WinStreakConfig,
    #[serde(default)]
    pub schedule: ScheduleConfig,
    #[serde(default)]
    pub delivery: DeliveryConfig,
}

/// Invalid configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config is not valid TOML for the dev tracker: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("config value out of range: {0}")]
    Range(String),
}

impl DevTrackerConfig {
    /// Parse and validate a TOML document.
    ///
    /// # Errors
    /// Parse failure, unknown keys, or out-of-range values.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let c: Self = toml::from_str(text)?;
        let pct = |v: u32, what: &str| {
            if v > 100 {
                Err(ConfigError::Range(format!("{what} = {v} (0..=100)")))
            } else {
                Ok(())
            }
        };
        pct(
            c.top_runners.min_migration_rate_pct,
            "top_runners.min_migration_rate_pct",
        )?;
        pct(
            c.top_migr.min_migration_rate_pct,
            "top_migr.min_migration_rate_pct",
        )?;
        if c.pending_window_days < 0
            || c.top_runners.max_inactive_days < 0
            || c.top_migr.max_inactive_days < 0
            || c.win_streak.max_inactive_days < 0
        {
            return Err(ConfigError::Range("day counts must be >= 0".into()));
        }
        if c.top_runners.max_plausible_ath_usd < c.top_runners.big_runner_ath_usd {
            return Err(ConfigError::Range(
                "top_runners.max_plausible_ath_usd must be >= big_runner_ath_usd".into(),
            ));
        }
        if c.schedule.ingest_every_minutes == 0 || c.schedule.derive_every_minutes == 0 {
            return Err(ConfigError::Range(
                "schedule.*_every_minutes must be >= 1".into(),
            ));
        }
        for f in &c.delivery.formats {
            if export::Format::parse(f).is_none() {
                return Err(ConfigError::Range(format!(
                    "delivery.formats: unknown format {f:?} (gmgn, axiom, basedbot)"
                )));
            }
        }
        if c.top_runners.big_runner_ath_usd < c.top_runners.runner_ath_usd {
            return Err(ConfigError::Range(
                "top_runners.big_runner_ath_usd must be >= runner_ath_usd".into(),
            ));
        }
        Ok(c)
    }
}

/// A derived list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    TopRunners,
    TopMigr,
    WinStreak,
}

impl Category {
    pub const ALL: [Self; 3] = [Self::TopRunners, Self::TopMigr, Self::WinStreak];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::TopRunners => "top-runners",
            Self::TopMigr => "top-migr",
            Self::WinStreak => "win-streak",
        }
    }
}

/// What the derivation knows about one creator (raw counts kept).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevStats {
    pub chain: String,
    pub creator: String,
    pub launches: u32,
    /// Launches on launchpads with a migration (curve).
    pub curve_launches: u32,
    pub migrated: u32,
    /// Curve launches that migrated or are older than the pending window.
    pub resolved: u32,
    pub pending: u32,
    /// `migrated / resolved` in basis points (`None` with no resolved launch).
    pub migration_rate_bp: Option<u32>,
    /// Consecutive migrated curve launches from the newest resolved one.
    pub current_streak: u32,
    pub runners: u32,
    pub big_runners: u32,
    pub last_launch_at: i64,
}

/// A creator with the categories it qualifies for (possibly none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevVerdict {
    pub stats: DevStats,
    pub categories: Vec<Category>,
}

fn rate_bp(num: u32, den: u32) -> Option<u32> {
    u32::try_from(
        u64::from(num)
            .saturating_mul(10_000)
            .checked_div(u64::from(den))?,
    )
    .ok()
}

/// Per-creator statistics over the given launch rows (any order).
#[must_use]
pub fn dev_stats(rows: &[DevLaunchRow], now: i64, cfg: &DevTrackerConfig) -> Vec<DevStats> {
    let mut by_dev: BTreeMap<(&str, &str), Vec<&DevLaunchRow>> = BTreeMap::new();
    for r in rows {
        by_dev
            .entry((r.chain.as_str(), r.creator.as_str()))
            .or_default()
            .push(r);
    }
    let pending_cutoff = now.saturating_sub(cfg.pending_window_days.saturating_mul(DAY));
    let runner = cfg.top_runners.runner_ath_usd.saturating_mul(100);
    let big = cfg.top_runners.big_runner_ath_usd.saturating_mul(100);
    let plausible = cfg.top_runners.max_plausible_ath_usd.saturating_mul(100);
    // unknown holder count = not a runner (never assumed)
    let held = |r: &DevLaunchRow| {
        let holders_ok = r
            .ath_holders
            .is_some_and(|h| h >= cfg.top_runners.min_holders);
        let liquidity_ok = !cfg
            .top_runners
            .liquidity_checked_launchpads
            .iter()
            .any(|lp| lp == &r.launchpad)
            || match (r.ath_max_liquidity_cents, r.ath_fdv_cents) {
                (Some(liq), Some(ath)) => {
                    i128::from(liq).saturating_mul(10_000)
                        >= i128::from(ath)
                            .saturating_mul(i128::from(cfg.top_runners.min_liquidity_bp_of_ath))
                }
                _ => false,
            };
        holders_ok && liquidity_ok
    };
    let no_curve = |lp: &str| cfg.launchpads_without_migration.iter().any(|x| x == lp);
    let mut out = Vec::with_capacity(by_dev.len());
    for ((chain, creator), mut ls) in by_dev {
        ls.sort_by_key(|r| (r.created_at, r.token.as_str()));
        let count = |f: &dyn Fn(&DevLaunchRow) -> bool| {
            u32::try_from(ls.iter().filter(|r| f(r)).count()).unwrap_or(u32::MAX)
        };
        let curve = |r: &DevLaunchRow| !no_curve(&r.launchpad);
        let is_pending =
            |r: &DevLaunchRow| r.migrated_at.is_none() && r.created_at > pending_cutoff;
        let curve_launches = count(&|r| curve(r));
        let migrated = count(&|r| curve(r) && r.migrated_at.is_some());
        let pending = count(&|r| curve(r) && is_pending(r));
        let resolved = curve_launches.saturating_sub(pending);
        // newest resolved curve launch backwards, pending ones skipped
        let current_streak = u32::try_from(
            ls.iter()
                .rev()
                .filter(|r| curve(r) && !is_pending(r))
                .take_while(|r| r.migrated_at.is_some())
                .count(),
        )
        .unwrap_or(u32::MAX);
        out.push(DevStats {
            chain: chain.to_string(),
            creator: creator.to_string(),
            launches: u32::try_from(ls.len()).unwrap_or(u32::MAX),
            curve_launches,
            migrated,
            resolved,
            pending,
            migration_rate_bp: rate_bp(migrated, resolved),
            current_streak,
            runners: count(&|r| {
                held(r)
                    && r.ath_fdv_cents
                        .is_some_and(|a| a >= runner && a <= plausible)
            }),
            big_runners: count(&|r| {
                held(r) && r.ath_fdv_cents.is_some_and(|a| a >= big && a <= plausible)
            }),
            last_launch_at: ls.last().map_or(0, |r| r.created_at),
        });
    }
    out
}

/// Derive the categories of every creator (docs/DEV-TRACKER.md §2).
#[must_use]
pub fn derive_categories(
    rows: &[DevLaunchRow],
    now: i64,
    cfg: &DevTrackerConfig,
) -> Vec<DevVerdict> {
    dev_stats(rows, now, cfg)
        .into_iter()
        .map(|s| verdict_of(s, now, cfg))
        .collect()
}

/// Categories of one creator's statistics.
#[must_use]
pub fn verdict_of(s: DevStats, now: i64, cfg: &DevTrackerConfig) -> DevVerdict {
    let active =
        |s: &DevStats, days: i64| s.last_launch_at >= now.saturating_sub(days.saturating_mul(DAY));
    {
        {
            let mut categories = Vec::new();
            let tr = &cfg.top_runners;
            let others = s
                .runners
                .saturating_sub(s.big_runners.min(tr.min_big_runners));
            // migration rate is checked only where the dev has curve launches
            let tr_rate_ok = s.curve_launches == 0
                || s.migration_rate_bp
                    .is_some_and(|bp| bp >= tr.min_migration_rate_pct.saturating_mul(100));
            if s.big_runners >= tr.min_big_runners
                && others >= tr.min_other_runners
                && tr_rate_ok
                && active(&s, tr.max_inactive_days)
            {
                categories.push(Category::TopRunners);
            }
            let tm = &cfg.top_migr;
            if s.resolved >= tm.min_launches
                && s.migration_rate_bp
                    .is_some_and(|bp| bp >= tm.min_migration_rate_pct.saturating_mul(100))
                && active(&s, tm.max_inactive_days)
            {
                categories.push(Category::TopMigr);
            }
            let ws = &cfg.win_streak;
            if categories.is_empty()
                && s.current_streak >= ws.min_streak
                && active(&s, ws.max_inactive_days)
            {
                categories.push(Category::WinStreak);
            }
            DevVerdict {
                stats: s,
                categories,
            }
        }
    }
}

/// Totals of a streamed derivation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeriveTotals {
    pub launch_rows: u64,
    pub creators: u64,
    pub members: u64,
}

/// Derive from the database one creator at a time (constant memory in the
/// number of launches): only category members are kept unless `keep_all`,
/// plus the `watch`ed `(chain, creator)` pairs (members of the last delivered
/// lists, so a dropped wallet's reason can be reported).
///
/// # Errors
/// Database failure.
pub async fn derive_from_db(
    db: &scout_devdb::DevDb,
    since: i64,
    now: i64,
    cfg: &DevTrackerConfig,
    keep_all: bool,
    watch: &std::collections::BTreeSet<(String, String)>,
) -> Result<(Vec<DevVerdict>, DeriveTotals), scout_devdb::DevDbError> {
    let mut out = Vec::new();
    let mut totals = DeriveTotals::default();
    db.for_each_dev(None, since, |rows| {
        totals.launch_rows += u64::try_from(rows.len()).unwrap_or(u64::MAX);
        for s in dev_stats(rows, now, cfg) {
            totals.creators += 1;
            let v = verdict_of(s, now, cfg);
            if !v.categories.is_empty() {
                totals.members += 1;
            }
            let watched = || watch.contains(&(v.stats.chain.clone(), v.stats.creator.clone()));
            if keep_all || !v.categories.is_empty() || watched() {
                out.push(v);
            }
        }
    })
    .await?;
    Ok((out, totals))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn cfg() -> DevTrackerConfig {
        DevTrackerConfig::from_toml(include_str!("../../../config/dev-tracker.example.toml"))
            .unwrap()
    }

    fn l(
        creator: &str,
        lp: &str,
        days_ago: i64,
        migrated: bool,
        ath_usd: Option<i64>,
    ) -> DevLaunchRow {
        let at = NOW - days_ago * DAY;
        DevLaunchRow {
            chain: "solana".into(),
            creator: creator.into(),
            token: format!("{creator}-{days_ago}-{lp}"),
            launchpad: lp.into(),
            created_at: at,
            migrated_at: migrated.then_some(at + 60),
            ath_fdv_cents: ath_usd.map(|u| u * 100),
            ath_holders: ath_usd.map(|_| 500),
            ath_max_liquidity_cents: ath_usd.map(|u| u * 10),
        }
    }

    fn cats(rows: &[DevLaunchRow], creator: &str) -> Vec<Category> {
        derive_categories(rows, NOW, &cfg())
            .into_iter()
            .find(|v| v.stats.creator == creator)
            .unwrap()
            .categories
    }

    #[test]
    fn example_config_parses_and_validates() {
        let c = cfg();
        assert_eq!(c.pending_window_days, 3);
        assert_eq!(c.top_migr.min_launches, 3);
        let bad = include_str!("../../../config/dev-tracker.example.toml").replace(
            "min_migration_rate_pct = 80",
            "min_migration_rate_pct = 180",
        );
        assert!(matches!(
            DevTrackerConfig::from_toml(&bad),
            Err(ConfigError::Range(_))
        ));
        let unknown = format!(
            "{}\nsurprise = 1\n",
            include_str!("../../../config/dev-tracker.example.toml")
        );
        assert!(
            DevTrackerConfig::from_toml(&unknown).is_err(),
            "unknown keys are refused"
        );
    }

    #[test]
    fn top_migr_needs_three_resolved_launches_at_eighty_percent() {
        // 4 of 5 resolved migrated = 80 %
        let mut rows = vec![
            l("a", "pump", 50, true, None),
            l("a", "pump", 40, true, None),
            l("a", "pump", 30, false, None),
            l("a", "pump", 20, true, None),
            l("a", "pump", 10, true, None),
        ];
        assert_eq!(cats(&rows, "a"), vec![Category::TopMigr]);
        // 1/1 is 100 % but below min_launches
        rows.push(l("b", "pump", 10, true, None));
        assert!(cats(&rows, "b").is_empty());
        // inactive for over a year
        let old: Vec<_> = (400..403).map(|d| l("c", "pump", d, true, None)).collect();
        assert!(cats(&old, "c").is_empty());
    }

    #[test]
    fn pending_launches_neither_fail_nor_break_a_streak() {
        // three migrated, then a 1-day-old unmigrated launch (pending)
        let rows = vec![
            l("s", "pump", 30, false, None),
            l("s", "pump", 30, false, None),
            l("s", "pump", 20, true, None),
            l("s", "pump", 15, true, None),
            l("s", "pump", 10, true, None),
            l("s", "pump", 1, false, None),
        ];
        let st = &dev_stats(&rows, NOW, &cfg())[0];
        assert_eq!(
            (st.pending, st.resolved, st.migrated, st.current_streak),
            (1, 5, 3, 3)
        );
        // 3/5 = 60 % < 80 %: not top-migr, so the streak list applies
        assert_eq!(cats(&rows, "s"), vec![Category::WinStreak]);
        // once the window expires the unmigrated launch breaks the streak
        let later: Vec<_> = rows
            .iter()
            .cloned()
            .map(|mut r| {
                r.created_at -= 5 * DAY;
                r.migrated_at = r.migrated_at.map(|m| m - 5 * DAY);
                r
            })
            .collect();
        assert_eq!(dev_stats(&later, NOW, &cfg())[0].current_streak, 0);
    }

    #[test]
    fn top_runners_rule_and_overlap_with_top_migr() {
        // 1 big runner + 2 runners, all migrated: top-runners AND top-migr
        let rows = vec![
            l("r", "pump", 30, true, Some(1_200_000)),
            l("r", "pump", 20, true, Some(600_000)),
            l("r", "pump", 10, true, Some(510_000)),
        ];
        assert_eq!(
            cats(&rows, "r"),
            vec![Category::TopRunners, Category::TopMigr]
        );
        // three runners but none >= $1M
        let rows = vec![
            l("q", "pump", 30, true, Some(900_000)),
            l("q", "pump", 20, true, Some(600_000)),
            l("q", "pump", 10, true, Some(510_000)),
        ];
        assert!(!cats(&rows, "q").contains(&Category::TopRunners));
        // migration rate below 3 % blocks top-runners on curve launchpads
        let mut rows = vec![
            l("m", "pump", 30, true, Some(2_000_000)),
            l("m", "pump", 29, true, Some(700_000)),
            l("m", "pump", 28, true, Some(700_000)),
        ];
        rows.extend((40..140).map(|d| l("m", "pump", d, false, None)));
        assert!(!cats(&rows, "m").contains(&Category::TopRunners));
    }

    #[test]
    fn thin_pool_ath_without_holders_is_not_a_runner() {
        let mut rows = vec![
            l("t", "zora", 1, false, Some(2_000_000)),
            l("t", "zora", 2, false, Some(900_000)),
            l("t", "zora", 3, false, Some(800_000)),
        ];
        assert_eq!(cats(&rows, "t"), [Category::TopRunners]);
        // one tiny trade in an empty pool: a $2M "ATH" with 3 holders
        rows[0].ath_holders = Some(3);
        assert!(cats(&rows, "t").is_empty());
        // unknown holders are never assumed
        rows[0].ath_holders = None;
        assert!(cats(&rows, "t").is_empty());
    }

    #[test]
    fn supply_artifact_ath_needs_liquidity_on_checked_launchpads() {
        let mut rows = vec![
            l("b", "bankr", 1, false, Some(400_000_000)),
            l("b", "bankr", 2, false, Some(900_000)),
            l("b", "bankr", 3, false, Some(800_000)),
        ];
        // liquidity seen = 10 % of ATH (test builder): real runners
        assert_eq!(cats(&rows, "b"), [Category::TopRunners]);
        // $400M "ATH" with $46 of liquidity (live Bankr shape)
        rows[0].ath_max_liquidity_cents = Some(4_600);
        assert!(cats(&rows, "b").is_empty());
        // the same numbers on a launchpad without the check still count
        for r in &mut rows {
            r.launchpad = "zora".into();
        }
        assert_eq!(cats(&rows, "b"), [Category::TopRunners]);
    }

    #[test]
    fn implausible_ath_never_makes_a_runner() {
        let rows = vec![
            l("x", "zora", 30, false, Some(99_990_000_000)),
            l("x", "zora", 20, false, Some(800_000)),
            l("x", "zora", 10, false, Some(700_000)),
        ];
        let v = derive_categories(&rows, NOW, &cfg());
        assert_eq!((v[0].stats.runners, v[0].stats.big_runners), (2, 0));
        assert!(v[0].categories.is_empty());
    }

    #[test]
    fn launchpads_without_migration_qualify_for_top_runners_only() {
        let rows = vec![
            l("z", "zora", 30, false, Some(3_000_000)),
            l("z", "zora", 20, false, Some(800_000)),
            l("z", "zora", 10, false, Some(700_000)),
        ];
        let v = derive_categories(&rows, NOW, &cfg());
        assert_eq!(v[0].stats.curve_launches, 0);
        assert_eq!(v[0].stats.migration_rate_bp, None);
        assert_eq!(v[0].categories, vec![Category::TopRunners]);
    }
}
