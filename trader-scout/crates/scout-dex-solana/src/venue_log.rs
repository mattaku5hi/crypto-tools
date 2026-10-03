//! Attribution of Anchor `emit!` events (`Program data: <base64>` log lines)
//! to the program that emitted them (ADR-013 section 2b, P4.9).
//!
//! Orca Whirlpool, Raydium CLMM and Raydium CPMM emit their swap events with
//! `emit!`, i.e. as a runtime log line, NOT as an event-CPI instruction.
//! A `Program data:` line carries no program id; the only sound attribution
//! is the runtime's own invoke/success stack:
//!
//! - every `Program <id> invoke [<depth>]` line is the invocation of the next
//!   instruction of the flattened list (top-level instruction, its inner
//!   instructions in execution order, next top-level...). The id must equal
//!   that instruction's program and the depth must equal stack size + 1;
//! - `Program <id> success` / `Program <id> failed...` pops the stack (the id
//!   must equal the top);
//! - a `Program data:` line belongs to the top of the stack.
//!
//! Anything that breaks these invariants makes the whole transaction
//! `Misaligned`: no line is attributed (a line is never given to a program
//! on a guess). `Program log:` lines can never be mistaken for a control
//! line because their id token would be `log:` (not base58).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use scout_core::{RawSolanaTransaction, SolanaPubkey};

/// Runtime marker after which no log line exists.
const LOG_TRUNCATED: &str = "Log truncated";
/// Longest `Program data:` base64 text decoded (bytes of text).
const MAX_PROGRAM_DATA_TEXT: usize = 16 * 1024;
/// Precompile programs: they appear in the instruction list but are never
/// invoked through the runtime and so write no `invoke` log line.
const PRECOMPILE_IDS: [&str; 3] = [
    "Ed25519SigVerify111111111111111111111111111",
    "KeccakSecp256k11111111111111111111111111111",
    "Secp256r1SigVerify1111111111111111111111111",
];

/// One `Program data:` line attributed to an instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramDataEvent {
    /// Position of the emitting instruction in `tx.instructions`.
    pub position: usize,
    /// Decoded base64 payload (`event discriminator (8) ++ Borsh event`).
    pub payload: Vec<u8>,
}

/// Result of attributing the log of one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogAttribution {
    /// The provider did not observe logs (`log_messages == None`).
    NotObserved,
    /// The invoke/success stream does not match the instruction list; no
    /// line is attributed.
    Misaligned { reason: String },
    Attributed {
        events: Vec<ProgramDataEvent>,
        /// The runtime truncated the log: instructions after the cut have no
        /// observable events.
        truncated: bool,
    },
}

fn is_precompile(program: &SolanaPubkey) -> bool {
    PRECOMPILE_IDS
        .iter()
        .any(|id| bs58::decode(id).into_vec().is_ok_and(|b| b == *program))
}

/// `Program <id> <tail>` -> `(id bytes, tail)`; `None` for every other line
/// (including `Program log:`/`Program data:`/`Program return:`).
fn control_line(line: &str) -> Option<(SolanaPubkey, &str)> {
    let rest = line.strip_prefix("Program ")?;
    let (id, tail) = rest.split_once(' ')?;
    if id.len() < 32 || id.len() > 44 {
        return None;
    }
    let bytes: SolanaPubkey = bs58::decode(id).into_vec().ok()?.try_into().ok()?;
    Some((bytes, tail))
}

fn depth_of(tail: &str) -> Option<usize> {
    tail.strip_prefix("invoke [")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// Attributes every `Program data:` line of `tx` to its emitting instruction.
#[must_use]
pub fn attribute_program_data(tx: &RawSolanaTransaction) -> LogAttribution {
    let Some(lines) = &tx.log_messages else {
        return LogAttribution::NotObserved;
    };
    let ixs = &tx.instructions;
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut events = Vec::new();
    let mut truncated = false;
    let mis = |reason: String| LogAttribution::Misaligned { reason };
    for line in lines {
        if line == LOG_TRUNCATED {
            truncated = true;
            break;
        }
        if let Some(text) = line.strip_prefix("Program data: ") {
            let Some(top) = stack.last().copied() else {
                return mis("`Program data:` line outside any invocation".to_owned());
            };
            if text.len() > MAX_PROGRAM_DATA_TEXT {
                return mis(format!(
                    "`Program data:` line of {} bytes exceeds {MAX_PROGRAM_DATA_TEXT}",
                    text.len()
                ));
            }
            // A line that is not base64 cannot be an Anchor event; it is
            // simply not an event line (programs may log other data).
            if let Ok(payload) = STANDARD.decode(text) {
                events.push(ProgramDataEvent {
                    position: top,
                    payload,
                });
            }
            continue;
        }
        let Some((id, tail)) = control_line(line) else {
            continue;
        };
        if let Some(depth) = depth_of(tail) {
            // Skip precompile instructions: they write no invoke line.
            while ixs
                .get(next)
                .is_some_and(|ix| is_precompile(&ix.program_id))
            {
                next += 1;
            }
            let Some(ix) = ixs.get(next) else {
                return mis("more invoke lines than instructions".to_owned());
            };
            if ix.program_id != id {
                return mis(format!(
                    "invoke line {next} names a different program than instruction {next}"
                ));
            }
            if depth != stack.len() + 1 {
                return mis(format!(
                    "invoke depth {depth} with a stack of {}",
                    stack.len()
                ));
            }
            stack.push(next);
            next += 1;
        } else if tail == "success" || tail.starts_with("failed") {
            let Some(top) = stack.pop() else {
                return mis("success/failed line with an empty stack".to_owned());
            };
            if ixs.get(top).map(|ix| ix.program_id) != Some(id) {
                return mis("success/failed line names a program other than the top".to_owned());
            }
        }
    }
    LogAttribution::Attributed { events, truncated }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scout_core::{RawSolanaInstruction, SolanaExecutionStatus};

    fn pk(s: &str) -> SolanaPubkey {
        bs58::decode(s).into_vec().unwrap().try_into().unwrap()
    }

    const A: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";
    const B: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
    const C: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";

    fn tx(programs: &[&str], logs: Option<Vec<String>>) -> RawSolanaTransaction {
        RawSolanaTransaction {
            block_time: None,
            signature: [0; 64],
            execution: SolanaExecutionStatus::Succeeded,
            slot: 1,
            transaction_index: 0,
            instructions: programs
                .iter()
                .enumerate()
                .map(|(i, p)| RawSolanaInstruction {
                    program_id: pk(p),
                    accounts: vec![],
                    data: vec![],
                    instruction_index: u32::try_from(i).unwrap(),
                })
                .collect(),
            token_balance_changes: vec![],
            fee_lamports: 0,
            fee_payer: [1; 32],
            signers: vec![[1; 32]],
            native_balance_changes: vec![],
            log_messages: logs,
        }
    }

    fn l(s: &str) -> String {
        s.to_owned()
    }

    fn data(bytes: &[u8]) -> String {
        format!("Program data: {}", STANDARD.encode(bytes))
    }

    #[test]
    fn not_observed_without_logs() {
        assert_eq!(
            attribute_program_data(&tx(&[A], None)),
            LogAttribution::NotObserved
        );
    }

    #[test]
    fn nested_stack_attributes_to_the_emitting_program_only() {
        // ix0 = A (top level) -> ix1 = B (depth 2) emits, back in A emits,
        // then ix2 = C (top level) emits.
        let logs = vec![
            format!("Program {A} invoke [1]"),
            format!("Program {B} invoke [2]"),
            l("Program log: Program data: spoof"),
            data(&[1, 1]),
            format!("Program {B} consumed 1 of 2 compute units"),
            format!("Program {B} success"),
            data(&[2, 2]),
            format!("Program {A} success"),
            format!("Program {C} invoke [1]"),
            data(&[3, 3]),
            format!("Program {C} success"),
        ];
        let LogAttribution::Attributed { events, truncated } =
            attribute_program_data(&tx(&[A, B, C], Some(logs)))
        else {
            panic!("misaligned");
        };
        assert!(!truncated);
        let got: Vec<(usize, Vec<u8>)> = events
            .into_iter()
            .map(|e| (e.position, e.payload))
            .collect();
        assert_eq!(got, vec![(1, vec![1, 1]), (0, vec![2, 2]), (2, vec![3, 3])]);
    }

    #[test]
    fn program_mismatch_depth_and_overflow_misalign_everything() {
        let wrong_program = vec![format!("Program {B} invoke [1]"), data(&[1])];
        assert!(matches!(
            attribute_program_data(&tx(&[A], Some(wrong_program))),
            LogAttribution::Misaligned { .. }
        ));
        let wrong_depth = vec![format!("Program {A} invoke [2]"), data(&[1])];
        assert!(matches!(
            attribute_program_data(&tx(&[A], Some(wrong_depth))),
            LogAttribution::Misaligned { .. }
        ));
        let too_many = vec![
            format!("Program {A} invoke [1]"),
            format!("Program {A} success"),
            format!("Program {A} invoke [1]"),
        ];
        assert!(matches!(
            attribute_program_data(&tx(&[A], Some(too_many))),
            LogAttribution::Misaligned { .. }
        ));
        let wrong_pop = vec![
            format!("Program {A} invoke [1]"),
            format!("Program {B} success"),
        ];
        assert!(matches!(
            attribute_program_data(&tx(&[A], Some(wrong_pop))),
            LogAttribution::Misaligned { .. }
        ));
        let orphan = vec![data(&[1])];
        assert!(matches!(
            attribute_program_data(&tx(&[A], Some(orphan))),
            LogAttribution::Misaligned { .. }
        ));
    }

    #[test]
    fn truncation_keeps_the_prefix_and_reports_it() {
        let logs = vec![
            format!("Program {A} invoke [1]"),
            data(&[9]),
            l("Log truncated"),
        ];
        let LogAttribution::Attributed { events, truncated } =
            attribute_program_data(&tx(&[A, B], Some(logs)))
        else {
            panic!("misaligned");
        };
        assert!(truncated);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn failed_line_pops_and_non_base64_data_is_not_an_event() {
        let logs = vec![
            format!("Program {A} invoke [1]"),
            l("Program data: !!not base64!!"),
            format!("Program {A} failed: custom program error: 0x1"),
        ];
        let LogAttribution::Attributed { events, .. } =
            attribute_program_data(&tx(&[A], Some(logs)))
        else {
            panic!("misaligned");
        };
        assert!(events.is_empty());
    }
}
