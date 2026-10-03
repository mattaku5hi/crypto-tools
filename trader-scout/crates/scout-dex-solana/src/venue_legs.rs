//! Direct-venue swap events as verified leg evidence (ADR-013 section 2b,
//! P4.9): Orca Whirlpool `Traded`, Meteora DLMM `Swap`/`Swap2Evt`, Raydium
//! CLMM and CPMM `SwapEvent`.
//!
//! Emission mechanism per venue (derived from the committed live fixtures):
//!
//! | venue | mechanism | gate |
//! |---|---|---|
//! | Meteora DLMM | Anchor `emit_cpi!` inner instruction to the DLMM program | program id + event-CPI tag + event authority + exact length |
//! | Orca Whirlpool | Anchor `emit!`: `Program data:` log line | log attributed to the program by the invoke/success stack + exact length |
//! | Raydium CLMM | `emit!` log line | same (213 live bytes, see `raydium_clmm_event`) |
//! | Raydium CPMM | `emit!` log line | same |
//!
//! A leg says WHICH mints were swapped (`input_mint`/`output_mint`): the
//! events name pools and directions, the mints come from the parent swap
//! instruction's accounts (`swap_v2`, DLMM, CPMM) or, only for the v1
//! Whirlpool/CLMM `swap` (no mint accounts), from the pool's own vault
//! balance deltas (exactly one rising and one falling mint of the accounts
//! owned by the pool, a single swap instruction on that pool in the
//! transaction, and one event amount equal to its side's delta). Anything
//! that does not resolve is an `Unresolved` issue and never a leg
//! (invariant 18).
//!
//! A leg proves a swap of `input_mint -> output_mint`; it carries no owner
//! and no consideration (ADR-013 section 2: ownership and price come only
//! from the wallet's own deltas).

use std::collections::BTreeMap;

use scout_core::{RawSolanaInstruction, RawSolanaTransaction, SolanaPubkey};

use crate::bonding_curve_buy::{EVENT_CPI_DISCRIMINATOR, VariantVerification, hex8};
use crate::dlmm_event::{
    DLMM_PROGRAM_ID_BYTES, DlmmEventOutcome, DlmmSwap2Event, DlmmSwapEvent, DlmmSwapIx,
    classify_dlmm_event, parse_dlmm_swap_instruction,
};
use crate::raydium_clmm_event::{
    RAYDIUM_CLMM_PROGRAM_ID_BYTES, RaydiumClmmSwapEvent, RaydiumClmmSwapIx,
    classify_raydium_clmm_event, parse_raydium_clmm_swap_instruction,
};
use crate::raydium_cpmm_event::{
    RAYDIUM_CPMM_PROGRAM_ID_BYTES, RaydiumCpmmSwapEvent, RaydiumCpmmSwapIx,
    classify_raydium_cpmm_event, parse_raydium_cpmm_swap_instruction,
};
use crate::venue_log::{LogAttribution, attribute_program_data};
use crate::venue_wire::{SwapInstructionParse, VenueEventOutcome};
use crate::whirlpool_event::{
    WHIRLPOOL_PROGRAM_ID_BYTES, WhirlpoolSwapIx, WhirlpoolTraded, classify_whirlpool_event,
    parse_whirlpool_swap_instruction,
};

/// The four direct venues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VenueKind {
    Whirlpool,
    Dlmm,
    RaydiumClmm,
    RaydiumCpmm,
}

impl VenueKind {
    pub const ALL: [Self; 4] = [
        Self::Whirlpool,
        Self::Dlmm,
        Self::RaydiumClmm,
        Self::RaydiumCpmm,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Whirlpool => "whirlpool",
            Self::Dlmm => "dlmm",
            Self::RaydiumClmm => "raydium_clmm",
            Self::RaydiumCpmm => "raydium_cpmm",
        }
    }

    #[must_use]
    pub const fn program_id(self) -> SolanaPubkey {
        match self {
            Self::Whirlpool => WHIRLPOOL_PROGRAM_ID_BYTES,
            Self::Dlmm => DLMM_PROGRAM_ID_BYTES,
            Self::RaydiumClmm => RAYDIUM_CLMM_PROGRAM_ID_BYTES,
            Self::RaydiumCpmm => RAYDIUM_CPMM_PROGRAM_ID_BYTES,
        }
    }

    /// The venue a program id belongs to.
    #[must_use]
    pub fn from_program(program: &SolanaPubkey) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.program_id() == *program)
    }

    /// `true` for the venues whose events are `emit!` log lines.
    const fn uses_logs(self) -> bool {
        !matches!(self, Self::Dlmm)
    }
}

/// Where the mints of a leg came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintSource {
    /// Mint accounts of the swap instruction.
    InstructionAccounts,
    /// The event itself names the mints (CPMM; the instruction agrees).
    EventFields,
    /// The pool's own vault balance deltas (v1 Whirlpool/CLMM `swap`).
    PoolVaultDeltas,
}

/// One swap proven by a venue event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VenueLeg {
    pub venue: VenueKind,
    /// IDL event name (`Traded`, `Swap`, `Swap2Evt`, `SwapEvent`).
    pub event: &'static str,
    /// `instruction_index` of the swap instruction.
    pub instruction_index: u32,
    pub pool: SolanaPubkey,
    pub input_mint: SolanaPubkey,
    pub input_amount: u64,
    pub output_mint: SolanaPubkey,
    pub output_amount: u64,
    pub mint_source: MintSource,
    /// `FixtureVerified` only when the swap instruction variant AND the event
    /// layout both have live samples that passed
    /// `scout-engine/tests/venue_swap_legs.rs`.
    pub verification: VariantVerification,
}

/// What went wrong with a candidate event or instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueIssueKind {
    /// An event or swap instruction with a broken structure. COVERAGE GAP.
    Malformed,
    /// An event discriminator outside the pinned IDL. COVERAGE GAP.
    UnknownEvent,
    /// Decoded, but an `IdlOnly` variant/layout: counted, never evidence.
    IdlOnly,
    /// No leg could be built: logs missing/misaligned/truncated, event of an
    /// unsupported or non-swap instruction, pool/direction mismatch,
    /// ambiguous or unresolvable mints.
    Unresolved,
}

/// One bounded coverage-gap observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueIssue {
    pub venue: VenueKind,
    pub kind: VenueIssueKind,
    /// `instruction_index` of the instruction the issue is about.
    pub instruction_index: u32,
    /// Event / instruction name or discriminator hex.
    pub name: String,
    pub reason: String,
}

/// Result of scanning one transaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VenueScan {
    pub legs: Vec<VenueLeg>,
    pub issues: Vec<VenueIssue>,
}

#[derive(Clone, Copy)]
enum SwapIx {
    Whirlpool(WhirlpoolSwapIx),
    Dlmm(DlmmSwapIx),
    Clmm(RaydiumClmmSwapIx),
    Cpmm(RaydiumCpmmSwapIx),
}

impl SwapIx {
    const fn pool(&self) -> SolanaPubkey {
        match self {
            Self::Whirlpool(i) => i.whirlpool,
            Self::Dlmm(i) => i.lb_pair,
            Self::Clmm(i) => i.pool_state,
            Self::Cpmm(i) => i.pool_state,
        }
    }

    const fn verification(&self) -> VariantVerification {
        match self {
            Self::Whirlpool(i) => i.variant.verification(),
            Self::Dlmm(i) => i.variant.verification(),
            Self::Clmm(i) => i.variant.verification(),
            Self::Cpmm(i) => i.variant.verification(),
        }
    }
}

#[derive(Clone)]
enum Slot {
    /// Not a venue instruction, a non-swap instruction or a DLMM event-CPI.
    Other,
    Swap(SwapIx),
    Unsupported(&'static str),
    Malformed,
}

#[derive(Clone, Copy)]
enum Ev {
    Traded(WhirlpoolTraded),
    Clmm(RaydiumClmmSwapEvent),
    Cpmm(RaydiumCpmmSwapEvent),
    DlmmSwap(DlmmSwapEvent),
    DlmmSwap2(DlmmSwap2Event),
}

impl Ev {
    const fn name(&self) -> &'static str {
        match self {
            Self::Traded(_) => "Traded",
            Self::Clmm(_) | Self::Cpmm(_) => "SwapEvent",
            Self::DlmmSwap(_) => "Swap",
            Self::DlmmSwap2(_) => "Swap2Evt",
        }
    }
}

/// A decoded event with the position of its swap instruction.
struct Cand {
    /// Position (in `tx.instructions`) of the swap instruction the event
    /// belongs to; `None` when no parent could be established.
    swap_pos: Option<usize>,
    /// Position of the instruction that emitted the event.
    event_pos: usize,
    venue: VenueKind,
    ev: Ev,
}

fn parse_slot(ix: &RawSolanaInstruction, issues: &mut Vec<VenueIssue>) -> Slot {
    let Some(venue) = VenueKind::from_program(&ix.program_id) else {
        return Slot::Other;
    };
    if venue == VenueKind::Dlmm && ix.data.get(..8) == Some(EVENT_CPI_DISCRIMINATOR.as_slice()) {
        return Slot::Other;
    }
    let mut malformed = |reason: String| {
        issues.push(VenueIssue {
            venue,
            kind: VenueIssueKind::Malformed,
            instruction_index: ix.instruction_index,
            name: ix
                .data
                .get(..8)
                .and_then(|d| <[u8; 8]>::try_from(d).ok())
                .map_or_else(|| "unknown".to_owned(), |d| hex8(&d)),
            reason,
        });
        Slot::Malformed
    };
    macro_rules! parse {
        ($parse:expr, $wrap:path) => {
            match $parse(ix) {
                SwapInstructionParse::Swap(s) => Slot::Swap($wrap(s)),
                SwapInstructionParse::NotSwap => Slot::Other,
                SwapInstructionParse::Unsupported { name } => Slot::Unsupported(name),
                SwapInstructionParse::Malformed { reason } => malformed(reason),
            }
        };
    }
    match venue {
        VenueKind::Whirlpool => parse!(parse_whirlpool_swap_instruction, SwapIx::Whirlpool),
        VenueKind::Dlmm => parse!(parse_dlmm_swap_instruction, SwapIx::Dlmm),
        VenueKind::RaydiumClmm => parse!(parse_raydium_clmm_swap_instruction, SwapIx::Clmm),
        VenueKind::RaydiumCpmm => parse!(parse_raydium_cpmm_swap_instruction, SwapIx::Cpmm),
    }
}

fn both_verified(a: VariantVerification, b: VariantVerification) -> VariantVerification {
    if a == VariantVerification::FixtureVerified && b == VariantVerification::FixtureVerified {
        VariantVerification::FixtureVerified
    } else {
        VariantVerification::IdlOnly
    }
}

/// Net owner-keyed delta per mint of the token accounts owned by `owner`.
fn owner_deltas(tx: &RawSolanaTransaction, owner: &SolanaPubkey) -> BTreeMap<SolanaPubkey, i128> {
    let mut out: BTreeMap<SolanaPubkey, i128> = BTreeMap::new();
    for c in &tx.token_balance_changes {
        if c.owner.as_ref() != Some(owner) {
            continue;
        }
        let delta = i128::from(c.post_amount) - i128::from(c.pre_amount.unwrap_or(0));
        *out.entry(c.mint).or_insert(0) += delta;
    }
    out
}

/// v1 Whirlpool/CLMM `swap`: the instruction names no mint. The pool's own
/// vaults (owned by the pool account) identify them: exactly one mint gains
/// and exactly one loses; the event amount of at least one side must equal
/// that delta exactly; and only one swap instruction may touch the pool.
fn mints_from_pool_deltas(
    tx: &RawSolanaTransaction,
    pool: &SolanaPubkey,
    swaps_on_pool: usize,
    input_amount: u64,
    output_amount: u64,
) -> Result<(SolanaPubkey, SolanaPubkey), String> {
    if swaps_on_pool != 1 {
        return Err(format!(
            "{swaps_on_pool} swap instructions touch the pool: vault deltas are ambiguous"
        ));
    }
    let deltas = owner_deltas(tx, pool);
    let mut rising = deltas.iter().filter(|(_, d)| **d > 0);
    let mut falling = deltas.iter().filter(|(_, d)| **d < 0);
    let (Some((in_mint, in_delta)), None) = (rising.next(), rising.next()) else {
        return Err("pool vault deltas do not show exactly one rising mint".to_owned());
    };
    let (Some((out_mint, out_delta)), None) = (falling.next(), falling.next()) else {
        return Err("pool vault deltas do not show exactly one falling mint".to_owned());
    };
    let in_ok = *in_delta == i128::from(input_amount);
    let out_ok = out_delta.checked_neg() == Some(i128::from(output_amount));
    if !(in_ok || out_ok) {
        return Err("neither event amount equals its pool vault delta".to_owned());
    }
    Ok((*in_mint, *out_mint))
}

struct Ctx<'a> {
    tx: &'a RawSolanaTransaction,
    slots: &'a [Slot],
}

impl Ctx<'_> {
    fn swaps_on_pool(&self, venue: VenueKind, pool: &SolanaPubkey) -> usize {
        self.slots
            .iter()
            .zip(&self.tx.instructions)
            .filter(|(s, ix)| {
                ix.program_id == venue.program_id()
                    && matches!(s, Slot::Swap(i) if i.pool() == *pool)
            })
            .count()
    }

    #[allow(clippy::too_many_lines)]
    fn build_leg(&self, c: &Cand) -> Result<VenueLeg, String> {
        let pos = c
            .swap_pos
            .ok_or_else(|| "no parent swap instruction for the event".to_owned())?;
        let ix = self
            .tx
            .instructions
            .get(pos)
            .ok_or_else(|| "parent instruction out of range".to_owned())?;
        let swap = match self.slots.get(pos) {
            Some(Slot::Swap(s)) => *s,
            Some(Slot::Unsupported(n)) => {
                return Err(format!("event of unsupported instruction {n}"));
            }
            Some(Slot::Malformed) => return Err("event of a malformed swap instruction".to_owned()),
            _ => return Err("event of a non-swap instruction".to_owned()),
        };
        let leg = |event: &'static str,
                   pool: SolanaPubkey,
                   input_mint: SolanaPubkey,
                   input_amount: u64,
                   output_mint: SolanaPubkey,
                   output_amount: u64,
                   mint_source: MintSource,
                   event_verification: VariantVerification| VenueLeg {
            venue: c.venue,
            event,
            instruction_index: ix.instruction_index,
            pool,
            input_mint,
            input_amount,
            output_mint,
            output_amount,
            mint_source,
            verification: both_verified(swap.verification(), event_verification),
        };
        match (&swap, &c.ev) {
            (SwapIx::Whirlpool(i), Ev::Traded(t)) => {
                if t.whirlpool != i.whirlpool {
                    return Err("event pool differs from the instruction's whirlpool".to_owned());
                }
                if t.a_to_b != i.a_to_b {
                    return Err("event a_to_b differs from the instruction argument".to_owned());
                }
                let (input_mint, output_mint, source) = if let Some((a, b)) = i.mints_ab {
                    if t.a_to_b {
                        (a, b, MintSource::InstructionAccounts)
                    } else {
                        (b, a, MintSource::InstructionAccounts)
                    }
                } else {
                    let (a, b) = mints_from_pool_deltas(
                        self.tx,
                        &t.whirlpool,
                        self.swaps_on_pool(c.venue, &t.whirlpool),
                        t.input_amount,
                        t.output_amount,
                    )?;
                    (a, b, MintSource::PoolVaultDeltas)
                };
                Ok(leg(
                    "Traded",
                    t.whirlpool,
                    input_mint,
                    t.input_amount,
                    output_mint,
                    t.output_amount,
                    source,
                    crate::whirlpool_event::whirlpool_traded_verification(),
                ))
            }
            (SwapIx::Clmm(i), Ev::Clmm(e)) => {
                if e.pool_state != i.pool_state {
                    return Err("event pool differs from the instruction's pool_state".to_owned());
                }
                let (in_acct, out_acct) = if e.zero_for_one {
                    (e.token_account_0, e.token_account_1)
                } else {
                    (e.token_account_1, e.token_account_0)
                };
                if in_acct != i.input_token_account || out_acct != i.output_token_account {
                    return Err(
                        "event token accounts do not match the instruction's input/output accounts in the event direction"
                            .to_owned(),
                    );
                }
                let (input_amount, output_amount) = e.in_out_amounts();
                let (input_mint, output_mint, source) = if let Some((a, b)) = i.mints_in_out {
                    (a, b, MintSource::InstructionAccounts)
                } else {
                    let (a, b) = mints_from_pool_deltas(
                        self.tx,
                        &e.pool_state,
                        self.swaps_on_pool(c.venue, &e.pool_state),
                        input_amount,
                        output_amount,
                    )?;
                    (a, b, MintSource::PoolVaultDeltas)
                };
                Ok(leg(
                    "SwapEvent",
                    e.pool_state,
                    input_mint,
                    input_amount,
                    output_mint,
                    output_amount,
                    source,
                    e.layout.verification(),
                ))
            }
            (SwapIx::Cpmm(i), Ev::Cpmm(e)) => {
                if e.pool_id != i.pool_state {
                    return Err("event pool differs from the instruction's pool_state".to_owned());
                }
                if e.input_mint != i.input_mint || e.output_mint != i.output_mint {
                    return Err(
                        "event mints differ from the instruction's mint accounts".to_owned()
                    );
                }
                if e.base_input != i.variant.is_base_input() {
                    return Err("event base_input contradicts the instruction variant".to_owned());
                }
                Ok(leg(
                    "SwapEvent",
                    e.pool_id,
                    e.input_mint,
                    e.input_amount,
                    e.output_mint,
                    e.output_amount,
                    MintSource::EventFields,
                    VariantVerification::FixtureVerified,
                ))
            }
            (SwapIx::Dlmm(i), Ev::DlmmSwap(e)) => {
                if e.lb_pair != i.lb_pair {
                    return Err("event lb_pair differs from the instruction's".to_owned());
                }
                let (input_mint, output_mint) = if e.swap_for_y {
                    (i.token_x_mint, i.token_y_mint)
                } else {
                    (i.token_y_mint, i.token_x_mint)
                };
                Ok(leg(
                    "Swap",
                    e.lb_pair,
                    input_mint,
                    e.amount_in,
                    output_mint,
                    e.amount_out,
                    MintSource::InstructionAccounts,
                    VariantVerification::FixtureVerified,
                ))
            }
            (SwapIx::Dlmm(i), Ev::DlmmSwap2(e)) => {
                if e.lb_pair != i.lb_pair {
                    return Err("event lb_pair differs from the instruction's".to_owned());
                }
                let input_amount = e
                    .amount_in
                    .checked_sub(e.amount_left)
                    .ok_or_else(|| "amount_left exceeds amount_in".to_owned())?;
                let (input_mint, output_mint) = if e.swap_for_y {
                    (i.token_x_mint, i.token_y_mint)
                } else {
                    (i.token_y_mint, i.token_x_mint)
                };
                Ok(leg(
                    "Swap2Evt",
                    e.lb_pair,
                    input_mint,
                    input_amount,
                    output_mint,
                    e.amount_out,
                    MintSource::InstructionAccounts,
                    VariantVerification::FixtureVerified,
                ))
            }
            _ => Err("event does not belong to the instruction's venue".to_owned()),
        }
    }
}

fn issue(
    ix: &RawSolanaInstruction,
    venue: VenueKind,
    kind: VenueIssueKind,
    name: &str,
    reason: String,
) -> VenueIssue {
    VenueIssue {
        venue,
        kind,
        instruction_index: ix.instruction_index,
        name: name.to_owned(),
        reason,
    }
}

/// Scans one transaction for direct-venue swap events. Gates per venue are
/// described in the module docs; the result never contains a leg that was not
/// fully resolved. Only meaningful for successful transactions (the caller
/// decides).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn scan_venue_events(tx: &RawSolanaTransaction) -> VenueScan {
    let ixs = &tx.instructions;
    if !ixs
        .iter()
        .any(|ix| VenueKind::from_program(&ix.program_id).is_some())
    {
        return VenueScan::default();
    }
    let mut scan = VenueScan::default();
    let slots: Vec<Slot> = ixs
        .iter()
        .map(|ix| parse_slot(ix, &mut scan.issues))
        .collect();
    let mut cands: Vec<Cand> = Vec::new();

    // Log-emitting venues (Whirlpool, CLMM, CPMM).
    let any_log_swap = ixs.iter().zip(&slots).any(|(ix, s)| {
        matches!(s, Slot::Swap(_))
            && VenueKind::from_program(&ix.program_id).is_some_and(VenueKind::uses_logs)
    });
    let mut log_gap: Option<String> = None;
    if any_log_swap {
        match attribute_program_data(tx) {
            LogAttribution::NotObserved => {
                log_gap = Some("the provider did not return transaction logs".to_owned());
            }
            LogAttribution::Misaligned { reason } => {
                log_gap = Some(format!("log attribution failed: {reason}"));
            }
            LogAttribution::Attributed { events, truncated } => {
                if truncated {
                    log_gap = Some("log truncated before this instruction's event".to_owned());
                }
                for e in events {
                    let (Some(ix), Some(venue)) = (
                        ixs.get(e.position),
                        ixs.get(e.position)
                            .and_then(|ix| VenueKind::from_program(&ix.program_id)),
                    ) else {
                        continue;
                    };
                    if !venue.uses_logs() {
                        continue;
                    }
                    macro_rules! classify {
                        ($f:expr, $wrap:path) => {
                            match $f(&e.payload) {
                                VenueEventOutcome::Event(ev) => cands.push(Cand {
                                    swap_pos: Some(e.position),
                                    event_pos: e.position,
                                    venue,
                                    ev: $wrap(ev),
                                }),
                                VenueEventOutcome::KnownNonLeg { .. } => {}
                                VenueEventOutcome::UnknownEvent { discriminator } => {
                                    scan.issues.push(issue(
                                        ix,
                                        venue,
                                        VenueIssueKind::UnknownEvent,
                                        &hex8(&discriminator),
                                        "event discriminator is not in the pinned IDL".to_owned(),
                                    ));
                                }
                                VenueEventOutcome::Malformed { reason } => {
                                    scan.issues.push(issue(
                                        ix,
                                        venue,
                                        VenueIssueKind::Malformed,
                                        e.payload
                                            .get(..8)
                                            .and_then(|d| <[u8; 8]>::try_from(d).ok())
                                            .map_or("unknown".to_owned(), |d| hex8(&d))
                                            .as_str(),
                                        reason,
                                    ));
                                }
                            }
                        };
                    }
                    match venue {
                        VenueKind::Whirlpool => classify!(classify_whirlpool_event, Ev::Traded),
                        VenueKind::RaydiumClmm => {
                            classify!(classify_raydium_clmm_event, Ev::Clmm);
                        }
                        VenueKind::RaydiumCpmm => {
                            classify!(classify_raydium_cpmm_event, Ev::Cpmm);
                        }
                        VenueKind::Dlmm => {}
                    }
                }
            }
        }
    }

    // DLMM event-CPIs: the parent is the nearest preceding DLMM instruction
    // that is not itself an event-CPI (a program cannot CPI itself except for
    // its own event emission, so the swap instruction's subtree is contiguous).
    let mut parent: Option<usize> = None;
    for (pos, ix) in ixs.iter().enumerate() {
        match classify_dlmm_event(ix) {
            DlmmEventOutcome::NotMine => {}
            DlmmEventOutcome::NotEventCpi => parent = Some(pos),
            DlmmEventOutcome::Swap(e) => cands.push(Cand {
                swap_pos: parent,
                event_pos: pos,
                venue: VenueKind::Dlmm,
                ev: Ev::DlmmSwap(e),
            }),
            DlmmEventOutcome::Swap2(e) => cands.push(Cand {
                swap_pos: parent,
                event_pos: pos,
                venue: VenueKind::Dlmm,
                ev: Ev::DlmmSwap2(e),
            }),
            DlmmEventOutcome::KnownNonLeg { .. } => {}
            DlmmEventOutcome::UnknownEvent { discriminator } => scan.issues.push(issue(
                ix,
                VenueKind::Dlmm,
                VenueIssueKind::UnknownEvent,
                &hex8(&discriminator),
                "event discriminator is not in the pinned IDL".to_owned(),
            )),
            DlmmEventOutcome::Malformed { reason } => scan.issues.push(issue(
                ix,
                VenueKind::Dlmm,
                VenueIssueKind::Malformed,
                ix.data
                    .get(8..16)
                    .and_then(|d| <[u8; 8]>::try_from(d).ok())
                    .map_or("unknown".to_owned(), |d| hex8(&d))
                    .as_str(),
                reason,
            )),
        }
    }

    // More than one event of the same kind for one swap instruction is
    // ambiguous: none of them is trusted.
    let mut per_key: BTreeMap<(Option<usize>, &'static str), usize> = BTreeMap::new();
    for c in &cands {
        *per_key.entry((c.swap_pos, c.ev.name())).or_insert(0) += 1;
    }
    let ctx = Ctx { tx, slots: &slots };
    for c in &cands {
        let Some(emitter) = ixs.get(c.event_pos) else {
            continue;
        };
        let result = if per_key
            .get(&(c.swap_pos, c.ev.name()))
            .copied()
            .unwrap_or(0)
            > 1
        {
            Err("more than one event of this kind for the same swap instruction".to_owned())
        } else {
            ctx.build_leg(c)
        };
        match result {
            Ok(leg) => {
                if leg.verification != VariantVerification::FixtureVerified {
                    scan.issues.push(issue(
                        emitter,
                        c.venue,
                        VenueIssueKind::IdlOnly,
                        c.ev.name(),
                        "event or swap-instruction variant has no live sample: decoded and counted, not leg evidence"
                            .to_owned(),
                    ));
                }
                scan.legs.push(leg);
            }
            Err(reason) => scan.issues.push(issue(
                emitter,
                c.venue,
                VenueIssueKind::Unresolved,
                c.ev.name(),
                reason,
            )),
        }
    }

    // Supported swap instructions without any candidate event.
    for (pos, (ix, slot)) in ixs.iter().zip(&slots).enumerate() {
        let (Slot::Swap(swap), Some(venue)) = (slot, VenueKind::from_program(&ix.program_id))
        else {
            continue;
        };
        if cands.iter().any(|c| c.swap_pos == Some(pos)) {
            continue;
        }
        let reason = if venue.uses_logs() {
            log_gap.clone().unwrap_or_else(|| {
                "no swap event line was attributed to the instruction".to_owned()
            })
        } else {
            "no swap event-CPI follows the instruction".to_owned()
        };
        let name = match swap {
            SwapIx::Whirlpool(i) => i.variant.name(),
            SwapIx::Dlmm(i) => i.variant.name(),
            SwapIx::Clmm(i) => i.variant.name(),
            SwapIx::Cpmm(i) => i.variant.name(),
        };
        scan.issues
            .push(issue(ix, venue, VenueIssueKind::Unresolved, name, reason));
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dlmm_event::{
        DLMM_EVENT_AUTHORITY_BYTES, DLMM_SWAP_DISCRIMINATOR, DLMM_SWAP_EVENT_DISCRIMINATOR,
        DLMM_SWAP2_DISCRIMINATOR, DLMM_SWAP2_EVENT_DISCRIMINATOR,
    };
    use crate::raydium_clmm_event::{
        RAYDIUM_CLMM_SWAP_DISCRIMINATOR, RAYDIUM_CLMM_SWAP_EVENT_DISCRIMINATOR,
        RAYDIUM_CLMM_SWAP_V2_DISCRIMINATOR,
    };
    use crate::raydium_cpmm_event::{
        RAYDIUM_CPMM_SWAP_BASE_INPUT_DISCRIMINATOR, RAYDIUM_CPMM_SWAP_BASE_OUTPUT_DISCRIMINATOR,
        RAYDIUM_CPMM_SWAP_EVENT_DISCRIMINATOR,
    };
    use crate::whirlpool_event::{
        WHIRLPOOL_SWAP_DISCRIMINATOR, WHIRLPOOL_SWAP_V2_DISCRIMINATOR,
        WHIRLPOOL_TRADED_DISCRIMINATOR,
    };
    use base64::Engine as _;
    use scout_core::{SolanaExecutionStatus, SolanaTokenBalanceChange};

    fn b58(s: &str) -> SolanaPubkey {
        bs58::decode(s).into_vec().unwrap().try_into().unwrap()
    }

    fn id(n: u8) -> SolanaPubkey {
        [n; 32]
    }

    #[test]
    fn program_id_and_authority_constants_match_base58() {
        assert_eq!(
            VenueKind::Whirlpool.program_id(),
            b58(crate::WHIRLPOOL_PROGRAM_ID)
        );
        assert_eq!(VenueKind::Dlmm.program_id(), b58(crate::DLMM_PROGRAM_ID));
        assert_eq!(
            VenueKind::RaydiumClmm.program_id(),
            b58(crate::RAYDIUM_CLMM_PROGRAM_ID)
        );
        assert_eq!(
            VenueKind::RaydiumCpmm.program_id(),
            b58(crate::RAYDIUM_CPMM_PROGRAM_ID)
        );
        assert_eq!(
            DLMM_EVENT_AUTHORITY_BYTES,
            b58("D1ZN9Wj1fRSUQfCjhvnu1hqDMT7hzjzBBpi12nVniYD6")
        );
    }

    fn ix(
        program: SolanaPubkey,
        data: Vec<u8>,
        accounts: Vec<SolanaPubkey>,
    ) -> RawSolanaInstruction {
        RawSolanaInstruction {
            program_id: program,
            accounts,
            data,
            instruction_index: 0,
        }
    }

    fn tx_of(
        mut instructions: Vec<RawSolanaInstruction>,
        changes: Vec<SolanaTokenBalanceChange>,
        logs: Option<Vec<String>>,
    ) -> RawSolanaTransaction {
        for (i, x) in instructions.iter_mut().enumerate() {
            x.instruction_index = u32::try_from(i).unwrap();
        }
        RawSolanaTransaction {
            block_time: None,
            signature: [0; 64],
            execution: SolanaExecutionStatus::Succeeded,
            slot: 1,
            transaction_index: 0,
            instructions,
            token_balance_changes: changes,
            fee_lamports: 0,
            fee_payer: id(200),
            signers: vec![id(200)],
            native_balance_changes: vec![],
            log_messages: logs,
        }
    }

    fn accounts(n: usize, set: &[(usize, SolanaPubkey)]) -> Vec<SolanaPubkey> {
        let mut a: Vec<SolanaPubkey> = (0..n)
            .map(|i| [u8::try_from(i).unwrap() + 100; 32])
            .collect();
        for (i, k) in set {
            a[*i] = *k;
        }
        a
    }

    fn log_lines(program: SolanaPubkey, payload: &[u8]) -> Vec<String> {
        let p = bs58::encode(program).into_string();
        vec![
            format!("Program {p} invoke [1]"),
            format!(
                "Program data: {}",
                base64::engine::general_purpose::STANDARD.encode(payload)
            ),
            format!("Program {p} success"),
        ]
    }

    fn traded(pool: SolanaPubkey, a_to_b: bool, input: u64, output: u64) -> Vec<u8> {
        let mut p = WHIRLPOOL_TRADED_DISCRIMINATOR.to_vec();
        p.extend_from_slice(&pool);
        p.push(u8::from(a_to_b));
        p.extend_from_slice(&[0; 32]);
        for v in [input, output, 0, 0, 0, 0] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p
    }

    fn whirl_ix(v2: bool, pool: SolanaPubkey, a_to_b: bool) -> RawSolanaInstruction {
        let (disc, len, n, pool_idx) = if v2 {
            (WHIRLPOOL_SWAP_V2_DISCRIMINATOR, 43, 15, 4)
        } else {
            (WHIRLPOOL_SWAP_DISCRIMINATOR, 42, 11, 2)
        };
        let mut data = disc.to_vec();
        data.resize(len, 0);
        data[41] = u8::from(a_to_b);
        ix(
            VenueKind::Whirlpool.program_id(),
            data,
            accounts(n, &[(pool_idx, pool), (5, id(1)), (6, id(2))]),
        )
    }

    fn change(mint: u8, owner: SolanaPubkey, pre: u64, post: u64) -> SolanaTokenBalanceChange {
        SolanaTokenBalanceChange {
            mint: id(mint),
            owner: Some(owner),
            decimals: 6,
            pre_amount: Some(pre),
            post_amount: post,
            closed: false,
        }
    }

    #[test]
    fn whirlpool_v2_leg_from_logs_with_instruction_mints_and_direction() {
        let pool = id(50);
        let logs = log_lines(
            VenueKind::Whirlpool.program_id(),
            &traded(pool, false, 10, 20),
        );
        let tx = tx_of(vec![whirl_ix(true, pool, false)], vec![], Some(logs));
        let scan = scan_venue_events(&tx);
        assert_eq!(scan.issues, vec![]);
        let [leg] = scan.legs.as_slice() else {
            panic!("{:?}", scan.legs)
        };
        // a_to_b = false: b in, a out.
        assert_eq!((leg.input_mint, leg.output_mint), (id(2), id(1)));
        assert_eq!((leg.input_amount, leg.output_amount), (10, 20));
        assert_eq!(leg.mint_source, MintSource::InstructionAccounts);
        assert_eq!(leg.verification, VariantVerification::FixtureVerified);
        assert_eq!(
            (leg.venue, leg.event, leg.pool),
            (VenueKind::Whirlpool, "Traded", pool)
        );
    }

    #[test]
    fn log_venue_without_logs_or_with_misaligned_logs_is_unresolved_never_a_leg() {
        let pool = id(50);
        let none = tx_of(vec![whirl_ix(true, pool, true)], vec![], None);
        let s = scan_venue_events(&none);
        assert!(s.legs.is_empty());
        assert_eq!(s.issues.len(), 1);
        assert_eq!(s.issues[0].kind, VenueIssueKind::Unresolved);
        assert!(s.issues[0].reason.contains("did not return"));
        // Logs naming another program than the instruction.
        let logs = log_lines(
            VenueKind::RaydiumClmm.program_id(),
            &traded(pool, true, 1, 1),
        );
        let mis = tx_of(vec![whirl_ix(true, pool, true)], vec![], Some(logs));
        let s = scan_venue_events(&mis);
        assert!(s.legs.is_empty());
        assert!(
            s.issues
                .iter()
                .all(|i| i.kind == VenueIssueKind::Unresolved)
        );
        // Truncated before the event.
        let trunc = tx_of(
            vec![whirl_ix(true, pool, true)],
            vec![],
            Some(vec!["Log truncated".to_owned()]),
        );
        let s = scan_venue_events(&trunc);
        assert!(s.legs.is_empty());
        assert!(s.issues[0].reason.contains("truncated"));
    }

    #[test]
    fn event_pool_or_direction_mismatch_is_unresolved() {
        let pool = id(50);
        let wrong_pool = log_lines(
            VenueKind::Whirlpool.program_id(),
            &traded(id(51), true, 1, 1),
        );
        let s = scan_venue_events(&tx_of(
            vec![whirl_ix(true, pool, true)],
            vec![],
            Some(wrong_pool),
        ));
        assert!(s.legs.is_empty());
        assert_eq!(s.issues.len(), 1);
        let wrong_dir = log_lines(
            VenueKind::Whirlpool.program_id(),
            &traded(pool, false, 1, 1),
        );
        let s = scan_venue_events(&tx_of(
            vec![whirl_ix(true, pool, true)],
            vec![],
            Some(wrong_dir),
        ));
        assert!(s.legs.is_empty());
        assert_eq!(s.issues.len(), 1);
    }

    #[test]
    fn whirlpool_v1_resolves_mints_from_pool_deltas_only_when_unambiguous() {
        let pool = id(50);
        let logs = log_lines(
            VenueKind::Whirlpool.program_id(),
            &traded(pool, true, 100, 40),
        );
        let changes = vec![change(7, pool, 1000, 1100), change(8, pool, 500, 460)];
        let tx = tx_of(
            vec![whirl_ix(false, pool, true)],
            changes.clone(),
            Some(logs.clone()),
        );
        let s = scan_venue_events(&tx);
        let [leg] = s.legs.as_slice() else {
            panic!("{:?}", s.issues)
        };
        assert_eq!((leg.input_mint, leg.output_mint), (id(7), id(8)));
        assert_eq!(leg.mint_source, MintSource::PoolVaultDeltas);
        // Neither amount equals its delta: unresolved.
        let bad = vec![change(7, pool, 1000, 1101), change(8, pool, 500, 461)];
        let s = scan_venue_events(&tx_of(
            vec![whirl_ix(false, pool, true)],
            bad,
            Some(logs.clone()),
        ));
        assert!(s.legs.is_empty());
        // Two rising mints: ambiguous.
        let two = vec![
            change(7, pool, 0, 100),
            change(9, pool, 0, 5),
            change(8, pool, 500, 460),
        ];
        let s = scan_venue_events(&tx_of(vec![whirl_ix(false, pool, true)], two, Some(logs)));
        assert!(s.legs.is_empty());
        // Two swap instructions on the same pool: ambiguous, both unresolved.
        let mut both_logs = log_lines(
            VenueKind::Whirlpool.program_id(),
            &traded(pool, true, 100, 40),
        );
        both_logs.extend(log_lines(
            VenueKind::Whirlpool.program_id(),
            &traded(pool, true, 100, 40),
        ));
        let s = scan_venue_events(&tx_of(
            vec![whirl_ix(false, pool, true), whirl_ix(false, pool, true)],
            changes,
            Some(both_logs),
        ));
        assert!(s.legs.is_empty());
        assert_eq!(s.issues.len(), 2);
    }

    fn dlmm_event(disc: [u8; 8], swap_for_y: bool, ain: u64, aout: u64) -> RawSolanaInstruction {
        let mut data = EVENT_CPI_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&disc);
        data.extend_from_slice(&id(60)); // lb_pair
        data.extend_from_slice(&id(61)); // from
        data.extend_from_slice(&[0; 8]); // bins
        if disc == DLMM_SWAP_EVENT_DISCRIMINATOR {
            data.extend_from_slice(&ain.to_le_bytes());
            data.extend_from_slice(&aout.to_le_bytes());
            data.push(u8::from(swap_for_y));
            data.extend_from_slice(&[0; 8 + 8 + 16 + 8]);
        } else {
            data.push(u8::from(swap_for_y));
            data.extend_from_slice(&[0; 16]);
            data.extend_from_slice(&ain.to_le_bytes());
            data.extend_from_slice(&0u64.to_le_bytes()); // amount_left
            data.extend_from_slice(&aout.to_le_bytes());
            data.extend_from_slice(&[0; 4 * 8]);
            data.extend_from_slice(&[0, 0]);
        }
        ix(
            VenueKind::Dlmm.program_id(),
            data,
            vec![DLMM_EVENT_AUTHORITY_BYTES],
        )
    }

    fn dlmm_swap(v2: bool, pool: SolanaPubkey) -> RawSolanaInstruction {
        let (disc, len) = if v2 {
            (DLMM_SWAP2_DISCRIMINATOR, 28)
        } else {
            (DLMM_SWAP_DISCRIMINATOR, 24)
        };
        let mut data = disc.to_vec();
        data.resize(len, 0);
        ix(
            VenueKind::Dlmm.program_id(),
            data,
            accounts(16, &[(0, pool), (6, id(1)), (7, id(2))]),
        )
    }

    #[test]
    fn dlmm_events_attach_to_the_nearest_preceding_swap_instruction() {
        let pool = id(60);
        let other = ix(id(77), vec![1], vec![]);
        let tx = tx_of(
            vec![
                dlmm_swap(true, pool),
                dlmm_event(DLMM_SWAP_EVENT_DISCRIMINATOR, true, 10, 20),
                other,
                dlmm_event(DLMM_SWAP2_EVENT_DISCRIMINATOR, true, 10, 20),
            ],
            vec![],
            None,
        );
        let s = scan_venue_events(&tx);
        assert_eq!(s.issues, vec![]);
        assert_eq!(s.legs.len(), 2);
        for l in &s.legs {
            assert_eq!((l.input_mint, l.output_mint), (id(1), id(2)));
            assert_eq!(
                (l.input_amount, l.output_amount, l.instruction_index),
                (10, 20, 0)
            );
            assert_eq!(l.verification, VariantVerification::FixtureVerified);
        }
        // swap_for_y = false flips the direction.
        let tx = tx_of(
            vec![
                dlmm_swap(true, pool),
                dlmm_event(DLMM_SWAP2_EVENT_DISCRIMINATOR, false, 10, 20),
            ],
            vec![],
            None,
        );
        let l = scan_venue_events(&tx).legs[0];
        assert_eq!((l.input_mint, l.output_mint), (id(2), id(1)));
    }

    #[test]
    fn dlmm_orphan_wrong_pool_duplicate_and_missing_events_are_unresolved() {
        let pool = id(60);
        // Event with no preceding DLMM instruction.
        let s = scan_venue_events(&tx_of(
            vec![dlmm_event(DLMM_SWAP2_EVENT_DISCRIMINATOR, true, 1, 1)],
            vec![],
            None,
        ));
        assert!(s.legs.is_empty());
        assert_eq!(s.issues.len(), 1);
        // Event of another pool.
        let s = scan_venue_events(&tx_of(
            vec![
                dlmm_swap(true, id(99)),
                dlmm_event(DLMM_SWAP2_EVENT_DISCRIMINATOR, true, 1, 1),
            ],
            vec![],
            None,
        ));
        assert!(s.legs.is_empty());
        // Duplicate events of one kind.
        let s = scan_venue_events(&tx_of(
            vec![
                dlmm_swap(true, pool),
                dlmm_event(DLMM_SWAP2_EVENT_DISCRIMINATOR, true, 1, 1),
                dlmm_event(DLMM_SWAP2_EVENT_DISCRIMINATOR, true, 1, 1),
            ],
            vec![],
            None,
        ));
        assert!(s.legs.is_empty());
        assert_eq!(s.issues.len(), 2);
        // Swap instruction without any event.
        let s = scan_venue_events(&tx_of(vec![dlmm_swap(false, pool)], vec![], None));
        assert!(s.legs.is_empty());
        assert_eq!(s.issues[0].kind, VenueIssueKind::Unresolved);
        // Wrong authority: malformed, no leg, and the swap has no event.
        let mut bad = dlmm_event(DLMM_SWAP2_EVENT_DISCRIMINATOR, true, 1, 1);
        bad.accounts = vec![id(1)];
        let s = scan_venue_events(&tx_of(vec![dlmm_swap(true, pool), bad], vec![], None));
        assert!(s.legs.is_empty());
        assert!(s.issues.iter().any(|i| i.kind == VenueIssueKind::Malformed));
    }

    fn clmm_payload(
        pool: SolanaPubkey,
        acct0: SolanaPubkey,
        acct1: SolanaPubkey,
        zero_for_one: bool,
    ) -> Vec<u8> {
        let mut p = RAYDIUM_CLMM_SWAP_EVENT_DISCRIMINATOR.to_vec();
        p.extend_from_slice(&pool);
        p.extend_from_slice(&id(1));
        p.extend_from_slice(&acct0);
        p.extend_from_slice(&acct1);
        for v in [100u64, 0, 300, 0] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p.push(u8::from(zero_for_one));
        p.extend_from_slice(&[0; 36]);
        p.extend_from_slice(&[0; 16]);
        p
    }

    fn clmm_ix(v2: bool, pool: SolanaPubkey) -> RawSolanaInstruction {
        let (disc, n) = if v2 {
            (RAYDIUM_CLMM_SWAP_V2_DISCRIMINATOR, 16)
        } else {
            (RAYDIUM_CLMM_SWAP_DISCRIMINATOR, 13)
        };
        let mut data = disc.to_vec();
        data.resize(41, 0);
        // accounts 3/4 = input/output token accounts, 11/12 = vault mints.
        ix(
            VenueKind::RaydiumClmm.program_id(),
            data,
            accounts(
                n,
                &[
                    (2, pool),
                    (3, id(30)),
                    (4, id(31)),
                    (11, id(5)),
                    (12, id(6)),
                ],
            ),
        )
    }

    #[test]
    fn clmm_direction_is_checked_against_the_instruction_token_accounts() {
        let pool = id(50);
        let prog = VenueKind::RaydiumClmm.program_id();
        // zero_for_one: token_account_0 is the input account.
        let ok = log_lines(prog, &clmm_payload(pool, id(30), id(31), true));
        let s = scan_venue_events(&tx_of(vec![clmm_ix(true, pool)], vec![], Some(ok)));
        let [leg] = s.legs.as_slice() else {
            panic!("{:?}", s.issues)
        };
        assert_eq!((leg.input_mint, leg.output_mint), (id(5), id(6)));
        assert_eq!((leg.input_amount, leg.output_amount), (100, 300));
        assert_eq!(leg.verification, VariantVerification::FixtureVerified);
        // Reverse direction: accounts swapped in the event.
        let rev = log_lines(prog, &clmm_payload(pool, id(31), id(30), false));
        let s = scan_venue_events(&tx_of(vec![clmm_ix(true, pool)], vec![], Some(rev)));
        let [leg] = s.legs.as_slice() else {
            panic!("{:?}", s.issues)
        };
        assert_eq!((leg.input_amount, leg.output_amount), (300, 100));
        // Event accounts contradict the direction.
        let bad = log_lines(prog, &clmm_payload(pool, id(30), id(31), false));
        let s = scan_venue_events(&tx_of(vec![clmm_ix(true, pool)], vec![], Some(bad)));
        assert!(s.legs.is_empty());
        assert_eq!(s.issues.len(), 1);
        // IDL-length (197-byte) event: decoded, IdlOnly, counted.
        let mut idl_len = clmm_payload(pool, id(30), id(31), true);
        idl_len.truncate(idl_len.len() - 16);
        let s = scan_venue_events(&tx_of(
            vec![clmm_ix(true, pool)],
            vec![],
            Some(log_lines(prog, &idl_len)),
        ));
        assert_eq!(s.legs.len(), 1);
        assert_eq!(s.legs[0].verification, VariantVerification::IdlOnly);
        assert_eq!(s.issues.len(), 1);
        assert_eq!(s.issues[0].kind, VenueIssueKind::IdlOnly);
    }

    fn cpmm_payload(pool: SolanaPubkey, base_input: bool, in_mint: u8, out_mint: u8) -> Vec<u8> {
        let mut p = RAYDIUM_CPMM_SWAP_EVENT_DISCRIMINATOR.to_vec();
        p.extend_from_slice(&pool);
        for v in [0u64, 0, 500, 600, 0, 0] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p.push(u8::from(base_input));
        p.extend_from_slice(&id(in_mint));
        p.extend_from_slice(&id(out_mint));
        p.extend_from_slice(&[0; 16]);
        p.push(0);
        p
    }

    fn cpmm_ix(output_variant: bool, pool: SolanaPubkey) -> RawSolanaInstruction {
        let disc = if output_variant {
            RAYDIUM_CPMM_SWAP_BASE_OUTPUT_DISCRIMINATOR
        } else {
            RAYDIUM_CPMM_SWAP_BASE_INPUT_DISCRIMINATOR
        };
        let mut data = disc.to_vec();
        data.resize(24, 0);
        ix(
            VenueKind::RaydiumCpmm.program_id(),
            data,
            accounts(13, &[(3, pool), (10, id(5)), (11, id(6))]),
        )
    }

    #[test]
    fn cpmm_event_mints_must_agree_with_the_instruction_and_base_output_is_idl_only() {
        let pool = id(50);
        let prog = VenueKind::RaydiumCpmm.program_id();
        let s = scan_venue_events(&tx_of(
            vec![cpmm_ix(false, pool)],
            vec![],
            Some(log_lines(prog, &cpmm_payload(pool, true, 5, 6))),
        ));
        let [leg] = s.legs.as_slice() else {
            panic!("{:?}", s.issues)
        };
        assert_eq!((leg.input_mint, leg.output_mint), (id(5), id(6)));
        assert_eq!((leg.input_amount, leg.output_amount), (500, 600));
        assert_eq!(leg.mint_source, MintSource::EventFields);
        assert_eq!(leg.verification, VariantVerification::FixtureVerified);
        // Event mint differs from the instruction's mint account.
        let s = scan_venue_events(&tx_of(
            vec![cpmm_ix(false, pool)],
            vec![],
            Some(log_lines(prog, &cpmm_payload(pool, true, 5, 9))),
        ));
        assert!(s.legs.is_empty());
        // base_input flag contradicts the variant.
        let s = scan_venue_events(&tx_of(
            vec![cpmm_ix(false, pool)],
            vec![],
            Some(log_lines(prog, &cpmm_payload(pool, false, 5, 6))),
        ));
        assert!(s.legs.is_empty());
        // swap_base_output has no live sample: IdlOnly leg + issue.
        let s = scan_venue_events(&tx_of(
            vec![cpmm_ix(true, pool)],
            vec![],
            Some(log_lines(prog, &cpmm_payload(pool, false, 5, 6))),
        ));
        assert_eq!(s.legs.len(), 1);
        assert_eq!(s.legs[0].verification, VariantVerification::IdlOnly);
        assert_eq!(s.issues[0].kind, VenueIssueKind::IdlOnly);
    }

    #[test]
    fn same_discriminator_of_another_program_is_never_taken_for_this_venue() {
        // CLMM and CPMM `SwapEvent` share a discriminator; a CPMM line must
        // not be decoded as a CLMM event just because of its payload.
        let pool = id(50);
        let cpmm_line = log_lines(
            VenueKind::RaydiumClmm.program_id(),
            &cpmm_payload(pool, true, 5, 6),
        );
        let s = scan_venue_events(&tx_of(vec![clmm_ix(true, pool)], vec![], Some(cpmm_line)));
        assert!(s.legs.is_empty());
        assert!(s.issues.iter().any(|i| i.kind == VenueIssueKind::Malformed));
    }

    #[test]
    fn transaction_without_venue_programs_yields_nothing() {
        let tx = tx_of(vec![ix(id(77), vec![1, 2, 3], vec![])], vec![], None);
        assert_eq!(scan_venue_events(&tx), VenueScan::default());
    }
}
