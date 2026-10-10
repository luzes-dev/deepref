//! Fixed-cohort inference against a declared human reference standard.
//! This measures conditional reference retention, never total scientific recall.
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditDesign {
    pub population: u32,
    pub reference_relevant: u32,
    pub sample_size: u32,
    pub target_percent: u32,
    /// Review-wide alpha spending: 50_000_000 / 2^cohort_ordinal.
    pub alpha_billionths: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditResult {
    pub observed_relevant: u32,
    pub first_unsafe_total: u32,
    pub p_value: f64,
    pub passed: bool,
    pub reference_retention: Option<f64>,
}

#[derive(Debug, Error)]
#[error("invalid fixed-cohort audit design")]
pub struct InvalidAuditDesign;

impl AuditDesign {
    pub fn validate(self) -> Result<Self, InvalidAuditDesign> {
        if self.population > 1_000_000
            || self.reference_relevant > 1_000_000
            || self.sample_size > self.population
            || !matches!(self.target_percent, 95 | 98)
            || self.alpha_billionths == 0
            || self.alpha_billionths > 50_000_000
        {
            return Err(InvalidAuditDesign);
        }
        Ok(self)
    }

    /// Invert the hypergeometric CDF at the smallest unsafe number of positives.
    /// The integer boundary avoids rounding the recall threshold. Authorization
    /// uses an exact integer comparison; the numerical CDF is diagnostic only.
    pub fn evaluate(self, observed: u32) -> Result<AuditResult, InvalidAuditDesign> {
        self.validate()?;
        if observed > self.sample_size {
            return Err(InvalidAuditDesign);
        }
        let first_unsafe = ((u64::from(self.reference_relevant + observed) * 100)
            / u64::from(self.target_percent)
            - u64::from(self.reference_relevant)
            + 1) as u32;
        let p_value = if first_unsafe > self.population {
            0.0
        } else {
            hypergeometric_cdf(self.population, first_unsafe, self.sample_size, observed)
        };
        let passed = self.sample_size == self.population
            || first_unsafe > self.population
            || exact_cdf_passes(
                self.population,
                first_unsafe,
                self.sample_size,
                observed,
                self.alpha_billionths,
            );
        let denominator = self.reference_relevant + observed;
        Ok(AuditResult {
            observed_relevant: observed,
            first_unsafe_total: first_unsafe,
            p_value,
            passed,
            reference_retention: (denominator > 0 && self.sample_size > 0).then(|| {
                f64::from(denominator)
                    / (f64::from(self.reference_relevant)
                        + f64::from(observed) * f64::from(self.population)
                            / f64::from(self.sample_size))
            }),
        })
    }

    /// Transparent zero-miss planning baseline. Finding misses can fail this
    /// fixed sample; the sample can never grow after any audit labels arrive.
    pub fn zero_miss_sample(mut self) -> Result<u32, InvalidAuditDesign> {
        self.sample_size = 0;
        self.validate()?;
        let (mut low, mut high) = (0, self.population);
        while low < high {
            let mid = low + (high - low) / 2;
            self.sample_size = mid;
            if self.evaluate(0)?.passed {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        Ok(low)
    }
}

fn exact_choose(n: u32, k: u32) -> num_bigint::BigUint {
    if k > n {
        return num_bigint::BigUint::from(0u32);
    }
    let k = k.min(n - k);
    (1..=k).fold(num_bigint::BigUint::from(1u32), |v, i| v * (n - k + i) / i)
}

/// Transpose successes and draws to keep the integer terms small for rare misses.
/// C(n,k) C(N-n,D-k) / C(N,D) is the same hypergeometric law.
fn exact_cdf_passes(n: u32, d: u32, draws: u32, observed: u32, alpha: u32) -> bool {
    let low = d.saturating_sub(n - draws);
    let high = observed.min(draws).min(d);
    if low > high {
        return true;
    }
    let denominator = exact_choose(n, d);
    let mut term = exact_choose(draws, low) * exact_choose(n - draws, d - low);
    let mut sum = term.clone();
    for k in low..high {
        term = term * u64::from(draws - k) * u64::from(d - k)
            / (u64::from(k + 1) * u64::from(n - draws - (d - k) + 1));
        sum += &term;
    }
    sum * 1_000_000_000u32 <= denominator * alpha
}

fn log_choose(n: u32, k: u32) -> f64 {
    if k > n {
        return f64::NEG_INFINITY;
    }
    let k = k.min(n - k);
    (1..=k)
        .map(|i| (f64::from(n - k + i) / f64::from(i)).ln())
        .sum()
}

fn hypergeometric_cdf(n: u32, d: u32, draws: u32, observed: u32) -> f64 {
    let low = draws.saturating_sub(n - d);
    let high = observed.min(draws).min(d);
    if low > high {
        return 0.0;
    }
    let mut log_term = log_choose(d, low) + log_choose(n - d, draws - low) - log_choose(n, draws);
    let mut log_sum = log_term;
    for k in low..high {
        log_term += (f64::from(d - k) / f64::from(k + 1)).ln()
            + (f64::from(draws - k) / f64::from(n - d - (draws - k) + 1)).ln();
        let max = log_sum.max(log_term);
        log_sum = max + ((log_sum - max).exp() + (log_term - max).exp()).ln();
    }
    log_sum.exp().min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn design(n: u32, r: u32) -> AuditDesign {
        AuditDesign {
            population: n,
            reference_relevant: r,
            sample_size: 0,
            target_percent: 95,
            alpha_billionths: 50_000_000,
        }
    }
    #[test]
    fn census_and_empty_denominator() {
        let mut d = design(100, 0);
        d.sample_size = 100;
        let result = d.evaluate(0).unwrap();
        assert!(result.passed);
        assert_eq!(result.reference_retention, None);
        assert!(d.evaluate(101).is_err());
    }
    #[test]
    fn empty_null_with_no_labels_has_no_point_estimate() {
        let d = design(40, 10_000);
        let result = d.evaluate(0).unwrap();
        assert!(result.passed);
        assert_eq!(d.zero_miss_sample().unwrap(), 0);
        assert_eq!(result.reference_retention, None);
    }
    #[test]
    fn known_zero_miss_workload() {
        let mut d = design(1000, 100);
        let n = d.zero_miss_sample().unwrap();
        assert!((390..=395).contains(&n));
        d.sample_size = n;
        assert!(d.evaluate(0).unwrap().passed);
        d.sample_size -= 1;
        assert!(!d.evaluate(0).unwrap().passed);
    }
    #[test]
    fn exact_authority_matches_independent_integer_oracle_and_cutoff_ties() {
        fn choose(n: u32, k: u32) -> u128 {
            if k > n {
                return 0;
            }
            (1..=k.min(n - k)).fold(1, |v, i| {
                v * u128::from(n - k.min(n - k) + i) / u128::from(i)
            })
        }
        for n in 1..=20 {
            for d in 0..=n {
                for draws in 0..=n {
                    for k in 0..=draws {
                        let numerator: u128 = (0..=k.min(d))
                            .filter(|x| draws - x <= n - d)
                            .map(|x| choose(d, x) * choose(n - d, draws - x))
                            .sum();
                        let denominator = choose(n, draws);
                        for alpha in [1, 25_000_000, 50_000_000] {
                            assert_eq!(
                                exact_cdf_passes(n, d, draws, k, alpha),
                                numerator * 1_000_000_000 <= denominator * u128::from(alpha)
                            );
                        }
                    }
                }
            }
        }
        // One missed reference-positive record among forty; a 39-record sample.
        assert!(exact_cdf_passes(40, 1, 39, 0, 25_000_000));
        assert!(!exact_cdf_passes(40, 1, 39, 0, 24_999_999));
    }

    #[test]
    fn planner_and_authority_cover_targets_and_project_allocations() {
        for target in [95, 98] {
            for ordinal in [1, 2, 10, 25] {
                for (population, reference) in
                    [(0, 0), (1, 0), (40, 20), (1000, 100), (100, 10_000)]
                {
                    let mut d = design(population, reference);
                    d.target_percent = target;
                    d.alpha_billionths = 50_000_000 >> ordinal;
                    let n = d.zero_miss_sample().unwrap();
                    d.sample_size = n;
                    assert!(d.evaluate(0).unwrap().passed);
                    if n > 0 {
                        d.sample_size = n - 1;
                        assert!(!d.evaluate(0).unwrap().passed);
                    }
                }
            }
        }
        let mut d = design(10, 0);
        d.alpha_billionths = 0;
        assert!(d.validate().is_err());
        d = design(10, 0);
        d.target_percent = 97;
        assert!(d.validate().is_err());
    }

    #[test]
    fn finite_population_cdf_matches_enumeration() {
        fn choose(n: u32, k: u32) -> f64 {
            if k > n {
                return 0.0;
            }
            (1..=k).fold(1.0, |v, i| v * f64::from(n - k + i) / f64::from(i))
        }
        for n in 1..=24 {
            for d in 0..=n {
                for draws in 0..=n {
                    for k in 0..=draws {
                        let exact: f64 = (0..=k.min(d))
                            .filter(|x| draws - x <= n - d)
                            .map(|x| choose(d, x) * choose(n - d, draws - x) / choose(n, draws))
                            .sum();
                        assert!((hypergeometric_cdf(n, d, draws, k) - exact).abs() < 1e-10);
                    }
                }
            }
        }
    }
}
