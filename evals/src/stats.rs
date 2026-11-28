//! The two statistics the run report needs: a 95% range for a share, and a median.

/// z for a two-sided 95% range: the 97.5th percentile of the standard normal distribution.
pub const Z95: f64 = 1.959_963_984_540_054;

/// The 95% Wilson score interval for `hits` successes in `n` trials, as `(low, high)`.
///
/// Unlike a bootstrap or the plain ±1.96·√(p(1−p)/n) range, it does not shrink to zero width
/// when every trial succeeded or every trial failed: 10 of 10 gives about 72% to 100%, not
/// 100% to 100%. `n = 0` gives `(0.0, 0.0)`: nothing was measured.
pub fn wilson95(hits: u32, n: u32) -> (f64, f64) {
    if n == 0 {
        return (0.0, 0.0);
    }
    let trials = f64::from(n);
    let share = f64::from(hits.min(n)) / trials;
    let z2 = Z95 * Z95;
    let shrink = 1.0 + z2 / trials;
    let centre = (share + z2 / (2.0 * trials)) / shrink;
    let half_width =
        Z95 * (share * (1.0 - share) / trials + z2 / (4.0 * trials * trials)).sqrt() / shrink;
    // At 0 of n and n of n the formula gives exactly 0 and 1; rounding must not move them.
    let low = if hits == 0 {
        0.0
    } else {
        (centre - half_width).max(0.0)
    };
    let high = if hits >= n {
        1.0
    } else {
        (centre + half_width).min(1.0)
    };
    (low, high)
}

/// The middle value (the mean of the two middle values for an even count); `None` when empty.
pub fn median(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    match sorted.len() {
        0 => None,
        len if len % 2 == 1 => Some(sorted[mid]),
        _ => Some((sorted[mid - 1] + sorted[mid]) / 2.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Wilson bound is a share `p` for which the observed share sits exactly 1.96 standard
    /// errors away, the standard error taken at `p` itself. Zero when `p` is such a bound.
    fn distance_from_a_wilson_bound(bound: f64, hits: u32, n: u32) -> f64 {
        let observed = f64::from(hits) / f64::from(n);
        let standard_error = (bound * (1.0 - bound) / f64::from(n)).sqrt();
        ((observed - bound).abs() - Z95 * standard_error).abs()
    }

    #[test]
    fn bounds_solve_the_wilson_equation() {
        for (hits, n) in [(10, 10), (0, 10), (5, 10), (1, 3), (7, 25), (99, 100)] {
            let (low, high) = wilson95(hits, n);
            assert!(
                distance_from_a_wilson_bound(low, hits, n) < 1e-12,
                "low of {hits}/{n}"
            );
            assert!(
                distance_from_a_wilson_bound(high, hits, n) < 1e-12,
                "high of {hits}/{n}"
            );
            let observed = f64::from(hits) / f64::from(n);
            assert!(low <= observed && observed <= high, "{hits}/{n}");
        }
    }

    #[test]
    fn all_passed_and_all_failed_keep_a_real_width() {
        let (low, high) = wilson95(10, 10);
        assert!((low - 0.722).abs() < 5e-4, "{low}");
        assert_eq!(high, 1.0);
        let (low, high) = wilson95(0, 10);
        assert_eq!(low, 0.0);
        assert!((high - 0.278).abs() < 5e-4, "{high}");
        let (low, high) = wilson95(5, 10);
        assert!((low - 0.237).abs() < 5e-4, "{low}");
        assert!((high - 0.763).abs() < 5e-4, "{high}");
        assert!(
            (low + high - 1.0).abs() < 1e-12,
            "5 of 10 is symmetric around one half"
        );
    }

    #[test]
    fn nothing_measured_gives_zeros_and_more_trials_narrow_the_range() {
        assert_eq!(wilson95(0, 0), (0.0, 0.0));
        let width = |hits, n| {
            let (low, high) = wilson95(hits, n);
            high - low
        };
        assert!(width(50, 100) < width(5, 10));
        assert!(width(100, 100) < width(10, 10));
    }

    #[test]
    fn median_of_odd_even_and_empty() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }
}
