//! Trial order for one scenario variant.
//!
//! A warm-up per side is discarded. Measured rounds then alternate AB, BA,
//! AB, BA, so each pair's two trials are neighbours and drift cancels.
//! After the tier's base pairs, a side whose headline spread exceeds 10%
//! adds one pair, up to twice the base count.

use anyhow::{Result, bail};

/// How many measured pairs a run starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// 2 pairs.
    Quick,
    /// 5 pairs.
    Standard,
    /// 7 pairs.
    Full,
}

impl Tier {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "quick" => Ok(Self::Quick),
            "standard" => Ok(Self::Standard),
            "full" => Ok(Self::Full),
            other => bail!("unknown tier {other}; expected quick, standard, or full"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Standard => "standard",
            Self::Full => "full",
        }
    }

    /// Pairs before the spread check can add more.
    pub fn base_pairs(self) -> u32 {
        match self {
            Self::Quick => 2,
            Self::Standard => 5,
            Self::Full => 7,
        }
    }

    /// The spread rule stops at twice the base count.
    pub fn max_pairs(self) -> u32 {
        self.base_pairs().saturating_mul(2)
    }
}

/// One side of a comparison. A is the baseline, B the candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Side {
    A,
    B,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

/// One slot in the run, including discarded warm-ups (`pair_index == None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedTrial {
    pub order_index: u32,
    pub side: Side,
    /// `None` is the warm-up. Measured pairs number from 0.
    pub pair_index: Option<u32>,
    pub seed: u64,
}

/// `(max − min) / |median|` above this adds a pair.
pub const SPREAD_LIMIT: f64 = 0.10;

/// Even pairs are A then B. Odd pairs are B then A.
pub fn abba(pair_index: u32) -> [Side; 2] {
    if pair_index % 2 == 0 {
        [Side::A, Side::B]
    } else {
        [Side::B, Side::A]
    }
}

/// Warm-up A, warm-up B, then `pairs` measured ABBA rounds.
pub fn plan(pairs: u32, base_seed: u64) -> Vec<PlannedTrial> {
    let mut out = Vec::with_capacity(2 + pairs as usize * 2);
    push_warmup(&mut out, base_seed);
    for pair in 0..pairs {
        push_pair(&mut out, pair, base_seed);
    }
    out
}

pub fn push_warmup(plan: &mut Vec<PlannedTrial>, base_seed: u64) {
    for side in [Side::A, Side::B] {
        let order_index = plan.len() as u32;
        plan.push(PlannedTrial {
            order_index,
            side,
            pair_index: None,
            seed: base_seed + u64::from(order_index),
        });
    }
}

pub fn push_pair(plan: &mut Vec<PlannedTrial>, pair_index: u32, base_seed: u64) {
    for side in abba(pair_index) {
        let order_index = plan.len() as u32;
        plan.push(PlannedTrial {
            order_index,
            side,
            pair_index: Some(pair_index),
            seed: base_seed + u64::from(order_index),
        });
    }
}

/// Median. Even counts average the two central values.
pub fn median(values: &[f64]) -> Option<f64> {
    let mut finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() {
        return None;
    }
    finite.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = finite.len();
    if n % 2 == 1 {
        Some(finite[n / 2])
    } else {
        Some((finite[n / 2 - 1] + finite[n / 2]) / 2.0)
    }
}

/// `(max − min) / |median|`. Zero median with any spread is infinite.
/// Equal values are zero. Fewer than two samples is `None` (no extra pair).
pub fn spread_ratio(values: &[f64]) -> Option<f64> {
    let finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.len() < 2 {
        return None;
    }
    let min = finite.iter().copied().fold(f64::INFINITY, f64::min);
    let max = finite.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let med = median(&finite)?;
    if med == 0.0 {
        return if max == min {
            Some(0.0)
        } else {
            Some(f64::INFINITY)
        };
    }
    Some((max - min) / med.abs())
}

/// True when [`spread_ratio`] is strictly above [`SPREAD_LIMIT`].
pub fn exceeds_spread(values: &[f64]) -> bool {
    spread_ratio(values).is_some_and(|ratio| ratio > SPREAD_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_and_abba_order() {
        assert_eq!(Tier::Quick.base_pairs(), 2);
        assert_eq!(Tier::Standard.base_pairs(), 5);
        assert_eq!(Tier::Full.base_pairs(), 7);
        assert_eq!(Tier::Quick.max_pairs(), 4);
        assert_eq!(abba(0), [Side::A, Side::B]);
        assert_eq!(abba(1), [Side::B, Side::A]);
        assert_eq!(abba(2), [Side::A, Side::B]);
        let planned = plan(2, 10);
        let shape: Vec<_> = planned
            .iter()
            .map(|t| (t.side, t.pair_index, t.order_index, t.seed))
            .collect();
        assert_eq!(
            shape,
            vec![
                (Side::A, None, 0, 10),
                (Side::B, None, 1, 11),
                (Side::A, Some(0), 2, 12),
                (Side::B, Some(0), 3, 13),
                (Side::B, Some(1), 4, 14),
                (Side::A, Some(1), 5, 15),
            ]
        );
        // Each measured pair occupies two neighbouring slots.
        let measured: Vec<_> = planned.iter().filter(|t| t.pair_index.is_some()).collect();
        for chunk in measured.chunks(2) {
            assert_eq!(chunk[1].order_index, chunk[0].order_index + 1);
            assert_eq!(chunk[0].pair_index, chunk[1].pair_index);
        }
    }

    #[test]
    fn spread_boundary_is_exclusive() {
        // (105 − 95) / 100 = 0.10 exactly. The rule adds a pair only above 10%.
        assert!(!exceeds_spread(&[95.0, 105.0]));
        assert!(exceeds_spread(&[90.0, 110.0]));
        assert!(!exceeds_spread(&[0.0, 0.0]));
        assert!(exceeds_spread(&[0.0, 1.0]));
        assert!(!exceeds_spread(&[4.0]));
        assert_eq!(median(&[100.0, 110.0, 110.0, 120.0]), Some(110.0));
    }
}
