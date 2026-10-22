//! Statistics over a run's frames.

use serde::{Deserialize, Serialize};

/// Mean, 95th percentile and maximum of a series of per-frame values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub mean: f32,
    pub p95: f32,
    pub max: f32,
}

impl Summary {
    /// Summarize `values`; all zero for an empty series. The p95 is the
    /// nearest-rank percentile: the smallest value that at least 95% of the
    /// series if at or below, so it is always a value that occurred.
    pub fn of(values: &[f32]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f32::total_cmp);
        // Summed in f64: a 120 s run is ~7,200 frames, enough for f32 to drop
        // the low bits of each addend.
        let sum: f64 = sorted.iter().map(|&v| f64::from(v)).sum();
        // The rank in integers, because `0.95 * len` in floating point can land
        // a hair above a whole number and round up to the next frame.
        let rank = (sorted.len() * 95).div_ceil(100);
        Self {
            mean: (sum / sorted.len() as f64) as f32,
            p95: sorted[rank.max(1) - 1],
            max: sorted[sorted.len() - 1],
        }
    }
}

/// One figure across repeated runs: its median, its range, and that range as a
/// percentage of the median, which is the spread the baseline is recorded with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spread {
    pub median: f64,
    pub min: f64,
    pub max: f64,
    /// `(max - min) / median`,  as a percentage; 0 when the median is 0.
    pub percent: f64,
}

impl Spread {
    /// The spread of `values`; all zero when there are none. With an even
    /// count the median is the mean of the middle two.
    pub fn of(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        let median = if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        };
        let (min, max) = (sorted[0], sorted[n - 1]);
        let percent = if median != 0.0 { (max - min) / median * 100.0 } else { 0.0 };
        Self { median, min, max, percent }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1.0, 2.0, ..., `n` as f32.
    fn ramp(n: u32) -> Vec<f32> {
        (1..=n).map(|i| i as f32).collect()
    }

    #[test]
    fn an_empty_series_summarizes_to_zero() {
        assert_eq!(Summary::of(&[]), Summary::default());
    }

    #[test]
    fn one_value_is_its_own_mean_p95_and_max() {
        assert_eq!(Summary::of(&[4.5]), Summary { mean: 4.5, p95: 4.5, max: 4.5 });
    }

    #[test]
    fn the_p95_is_the_nearest_rank() {
        // 95% of 20 is 19 exactly, so the 19th value; of 100, the 95th; of 21,
        // 19.95 rounds up to the 20th.
        assert_eq!(Summary::of(&ramp(20)), Summary { mean: 10.5, p95: 19.0, max: 20.0 });
        assert_eq!(Summary::of(&ramp(100)).p95, 95.0);
        assert_eq!(Summary::of(&ramp(21)).p95, 20.0);
    }

    #[test]
    fn the_spread_of_an_odd_count_is_about_the_middle_run() {
        assert_eq!(
            Spread::of(&[4.5, 3.5, 4.0]),
            Spread { median: 4.0, min: 3.5, max: 4.5, percent: 25.0 },
        );
    }

    #[test]
    fn the_median_of_an_even_count_is_the_mean_of_the_middle_two() {
        assert_eq!(Spread::of(&[1.0, 4.0, 2.0, 3.0]).median, 2.5);
    }

    #[test]
    fn no_runs_spread_to_zero() {
        assert_eq!(Spread::of(&[]), Spread::default());
    }

    #[test]
    fn a_zero_median_has_no_spread() {
        assert_eq!(Spread::of(&[0.0, 0.0, 1.0]).percent, 0.0);
    }

    #[test]
    fn the_order_of_the_frames_does_not_matter() {
        let mut reversed = ramp(37);
        reversed.reverse();
        assert_eq!(Summary::of(&reversed), Summary::of(&ramp(37)));
    }
}
