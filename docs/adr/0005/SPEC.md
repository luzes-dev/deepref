# Workload / break-even computation spec (ADR 0005)

These are historical standalone-alpha, 80%-scenario-power illustrations.
Implemented V1 instead uses a registered review-wide level and a zero-miss
minimum planner with additional interleaved blind control labels; this script
does not calculate its operational forecast or those control costs.
See ADR 0005 sections 7.2–7.4 and the V1 acceptance roadmap.

This committed supporting spec describes `workload.py` in this directory.
Run `python3 -I docs/adr/0005/workload.py` from the repository root; it writes
`results.md` and `results.json` alongside itself using the standard library.
The full integer scan is authoritative; coarse-grid refinement is diagnostic
because it can miss an earlier crossing when power is non-monotone.

All prevalence, AI, human and reference-quality parameters are illustrative
assumptions, not measured DeepRef performance. The recall test conditions on a
fixed, correctly labelled R_IU and frozen population X. It does not cover
human losses in I/U or guarantee true recall with an imperfect reference.
Frame, R_IU and n are frozen before probability sampling; targeted removals
must precede that freeze. Power uses a fixed rounded miss count, not a mixture
model for uncertainty in that count.

Workload values assume successful finalization and exclude fallback screening,
review administration, targeted review, full-text work and AI spend. Loss
values are unconditional hypothetical-finalization scenarios with perfect
sample rescue, not post-pass losses or the losses of the fail-and-fallback
policy. R_IU for power is optimistically set before human misses; actual plans
must use the count found by people. Table 4 models fixed-sensitivity independent
binomial detection with no false positives. Its 0.995 row is an optimistic
scenario, not evidence that full-text checks rescue double-negative audit
records. See ADR 0005 sections 7–10 for these limitations and owner decisions.

## Exact hypergeometric helpers (log-space with math.lgamma for numerical stability; N up to 100,000)

- logC(n,k) = lgamma(n+1)-lgamma(k+1)-lgamma(n-k+1)
- hyp_pmf(k; N, D, n) = C(D,k) C(N-D,n-k) / C(N,n) for max(0,n-(N-D)) <= k <= min(n,D), else 0
- hyp_cdf(k; N, D, n) = sum_{j<=k} hyp_pmf(j; N, D, n) (sum the pmf terms; clamp to [0,1])

## Closed-cohort recall test (the estimator under evaluation)

Inputs: N_x = size of the AI "automation-eligible exclude" stratum X (records removed from the human queue);
R_IU = number of relevant records found by humans in the human-reviewed strata (AI include + AI unsure);
n = simple random sample (without replacement) drawn from X and labelled by the reference standard;
k = relevant records found in the sample; target recall tau; one-sided alpha.
Recall if X contains D relevant records in total = (R_IU + k) / (R_IU + D) (sampled relevant records are rescued).
D0(k) = floor((R_IU + k)/tau - R_IU) + 1 = smallest D for which recall < tau.
p_value(k) = hyp_cdf(k; N_x, D0(k), n) if D0(k) <= N_x, else 0.0 (recall >= tau even if all of X were relevant).
Cohort passes iff p_value(k) <= alpha.
Use tau = 0.95, alpha = 0.05 unless a table says otherwise. Guard floating point in floor: compute (R_IU+k)/tau with
fractions.Fraction (tau = Fraction(95,100)) to avoid 104.99999 errors.

### Table 1: audit fraction needed when the audit finds zero relevant records

For R_IU in [10, 20, 50, 100, 200, 500, 1000, 2000] and N_x in [1000, 5000, 20000, 50000]:
n0 = minimal n such that the cohort passes with k = 0. Report n0 and n0/N_x as a percentage (1 decimal).
Also report the large-N approximation f ≈ 1 - alpha^(1/D0(0)) for each R_IU.
Repeat for tau = 0.90 and tau = 0.98 (only N_x = 20000) in a compact second table.

### Table 2: planned sample size with power

power(n) = sum_k hyp_pmf(k; N_x, D_true, n) * [p_value(k) <= alpha], where D_true = round(expected relevant in X).
n_star = minimal n in [0, N_x] with power(n) >= 0.80 (binary search is NOT valid because power is not monotone in n
in general; do a linear scan in steps, e.g. step = max(1, N_x//2000) then refine downward with step 1 around the first hit;
document the method). If none, report "not achievable".

## Scenarios

Scenario grid (title/abstract stage, one closed cohort = the whole stage):

- N (records) in [2000, 5000, 20000, 50000]
- prevalence pi (fraction of records that should pass title/abstract per the reference standard) in [0.005, 0.02, 0.10]
- AI profile:
  - strong: r_ai = 0.99 (share of relevant records the AI does NOT route to X), q_x = 0.85 (share of irrelevant records routed to X)
  - weak: r_ai = 0.965, q_x = 0.60
    Fixed parameters: human single-screener sensitivity s_h = 0.90 (used only for the expected-loss column);
    human–human dual screening disagreement rate c_hh = 0.10 (each disagreement costs 1 adjudication judgment);
    AI–human disagreement rate in advisory second-reviewer mode c_ha = 0.12 (strong) / 0.20 (weak);
    reference-standard reviewer disagreement on audit records c_ref = 0.10.
    Derived (use exact expectations, round counts to nearest integer where a count is needed):
    Rel = pi N; Irr = (1-pi) N; D = Rel (1 - r_ai); N_x = D + q_x Irr; N_h = N - N_x; R_IU = Rel r_ai.

Workloads in "human record judgments" (1 adjudication = 1 judgment):

- W_single = N
- W_dual = 2N + c_hh N
- W_adv = N (advisory AI second reviewer, human not required to act on disagreement)
- W_adv_r = N + c_ha N (advisory AI: every AI–human disagreement adjudicated; does not validate human replacement)
- AI-first variants (human reviewers screen all of N_h singly, plus the audit):
  - W_af_1 = N_h + n_star (single blinded reference reviewer)
  - W_af_2 = N_h + 2 n_star (dual blinded reviewers, OR rule: relevant if either includes; no adjudication needed for the estimator)
  - W_af_2a = N_h + 2 n_star + c_ref n_star (dual + adjudication)
  - W_af_2d = 2 N_h + c_hh N_h + 2 n_star (dual screening of the human strata too + dual OR audit) — the comparator for projects whose baseline is dual screening
    Expected relevant records lost at this stage (protection column):
- single: Rel (1 - s_h); dual: Rel (1 - s_h)^2 * 3 (factor 3 = correlated errors: second reviewer misses 30% of what the first missed instead of 10%)
- AI-first (W_af_2): R_IU (1 - s_h) [human misses in human strata] + (D - E[k]) where E[k] = n_star * D / N_x [automation misses not rescued]
- AI-first dual-baseline (W_af_2d): R_IU (1 - s_h)^2 * 3 + (D - E[k])
  Report per scenario: N, pi, AI profile, Rel, N_x, N_h, R_IU, D, n_star (or n/a), audit share n_star/N_x, power at n_star,
  W_single, W_dual, W_adv_r, W_af_2, W_af_2d, savings of W_af_2 vs W_single and of W_af_2d vs W_dual (percent, negative = more work),
  expected losses, and a recommendation:
  "AI-first recommended" if achievable and W_af_2 <= 0.8 * W_single and (W_single - W_af_2) >= 500
  "AI-first allowed, little benefit" if achievable but not the above
  "AI-first not achievable" if no n reaches power 0.80
  (Also give the same classification against the dual baseline using W_af_2d vs W_dual.)

### Table 3: realism check of the strong/weak profiles

For N=20000, pi=0.02 show how n_star and W_af_2 change for r_ai in [0.95, 0.96, 0.97, 0.98, 0.99, 0.995] (q_x 0.85).

### Table 4: reference-standard bias

For a cohort where the reference standard detects a truly relevant audit record with probability s_ref, the observed k
is Binomial-thinned. For N=20000, pi=0.02, strong AI, n = n_star from Table 2, compute for s_ref in
[0.90 (single), 0.99 (dual independent, 1-(0.1)^2), 0.97 (dual correlated: 1-0.1*0.3), 0.995 (dual + downstream full-text check)]:
expected observed k, the probability that the cohort passes (exact: thin D_true -> sum over true k then binomial thinning),
and the expected upward bias of the recall point estimate (R_IU+k_obs)/(R_IU + (k_obs*N_x/n)) vs the true recall.
Also report the probability of passing when the AI is actually bad (r_ai = 0.93, true recall below 0.95) for each s_ref
— this is the "false pass" risk the reference standard must control.

## Output quality

- Put the parameter values above the tables. Use thousands separators. Percent with 1 decimal.
- Add a "self-check" section: verify by brute force (direct summation with math.comb for small N) that hyp_cdf agrees with
  the log-space version for N_x=1000 to 1e-9, and that for R_IU=100, tau=0.95, k=0, D0=6 and the large-N audit share ≈ 39.3%.
- Self-checks must return a nonzero exit status on failure. Commit the tables and machine-readable results; runtime duration is printed only, keeping outputs reproducible.
