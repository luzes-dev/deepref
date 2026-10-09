#!/usr/bin/env python3
"""ADR 0005 workload / break-even computation (implements SPEC.md in this directory).

Standard library only.  Run from this directory:   python3 -I workload.py
Writes results.md and results.json next to this file and prints a short summary.
"""

import json
import math
import os
import random
import sys
import time
from fractions import Fraction

HERE = os.path.dirname(os.path.abspath(__file__))
RESULTS_MD = os.path.join(HERE, "results.md")
RESULTS_JSON = os.path.join(HERE, "results.json")

F = Fraction

# ----------------------------------------------------------------- parameters
TAU = F(95, 100)              # target recall for Tables 1-4 (Table 1b: 0.90 and 0.98)
ALPHA = 0.05                  # one-sided alpha (float)
ALPHA_EXACT = F(5, 100)       # same alpha as an exact rational (used to decide ties)
POWER_TARGET = 0.80
PMF_CUTOFF = 1e-15            # pmf terms below this are skipped inside power sums
TIE_TOL = 1e-9                # |p - alpha| below this is decided with exact integers
S_H = F(9, 10)                # human single-screener sensitivity
C_HH = F(1, 10)               # human-human disagreement rate
C_REF = F(1, 10)              # reference-standard reviewer disagreement on audit records
REC_RATIO = F(8, 10)          # "recommended": W_af <= 0.8 * W_base ...
REC_ABS = 500                 #   ... and W_base - W_af >= 500

N_GRID = [2000, 5000, 20000, 50000]
PI_GRID = [F(5, 1000), F(2, 100), F(10, 100)]
PROFILES = [
    {"name": "strong", "r_ai": F(99, 100), "q_x": F(85, 100), "c_ha": F(12, 100)},
    {"name": "weak", "r_ai": F(965, 1000), "q_x": F(60, 100), "c_ha": F(20, 100)},
]
R_GRID = [10, 20, 50, 100, 200, 500, 1000, 2000]
NX_GRID_T1 = [1000, 5000, 20000, 50000]
TAUS_T1B = [("0.90", F(90, 100)), ("0.98", F(98, 100))]
R3_GRID = [F(95, 100), F(96, 100), F(97, 100), F(98, 100), F(99, 100), F(995, 1000)]
S_REF_GRID = [                 # SPEC order
    ("single reviewer", F(90, 100)),
    ("dual, independent", F(99, 100)),
    ("dual, correlated", F(97, 100)),
    ("dual + full-text check", F(995, 1000)),
]
S_PERFECT = ("perfect reference (baseline)", F(1))
R_BAD = F(93, 100)             # AI actually bad: true recall before audit 0.93 < 0.95
T4_N, T4_PI = 20000, F(2, 100)

# ------------------------------------------- log-factorial table (math.lgamma)
MAX_TABLE_N = 100_000
LF = [math.lgamma(i + 1) for i in range(MAX_TABLE_N + 1)]   # LF[i] = lgamma(i+1) = log(i!)


# ------------------------------------------------ hypergeometric helpers (SPEC)
def logC(n, k):
    """log C(n,k) = lgamma(n+1) - lgamma(k+1) - lgamma(n-k+1), read from LF."""
    return LF[n] - LF[k] - LF[n - k]


def hyp_support(N, D, n):
    return max(0, n - (N - D)), min(n, D)


def hyp_pmf(k, N, D, n):
    lo, hi = hyp_support(N, D, n)
    if k < lo or k > hi:
        return 0.0
    return math.exp(logC(D, k) + logC(N - D, n - k) - logC(N, n))


def hyp_cdf(k, N, D, n):
    """P(K <= k): direct summation of the pmf terms, clamped to [0, 1]."""
    lo, hi = hyp_support(N, D, n)
    s = 0.0
    for j in range(lo, min(k, hi) + 1):
        s += hyp_pmf(j, N, D, n)
    return min(1.0, max(0.0, s))


# ------------------------------------------------ closed-cohort recall test (SPEC)
_D0_CACHE = {}


def d0_of(R, k, tau=TAU):
    """D0(k) = floor((R+k)/tau - R) + 1 (smallest D with recall < tau); exact via Fraction."""
    key = (R, k, tau)
    v = _D0_CACHE.get(key)
    if v is None:
        v = math.floor(F(R + k) / tau - R) + 1
        _D0_CACHE[key] = v
    return v


def p_value(k, Nx, R, n, tau=TAU):
    """SPEC p_value(k): hyp_cdf(k; Nx, D0(k), n), or 0.0 when D0(k) > Nx (float version)."""
    D0 = d0_of(R, k, tau)
    if D0 > Nx:
        return 0.0
    return hyp_cdf(k, Nx, D0, n)


def pass_exact(k, Nx, R, n, tau=TAU):
    """Exact decision [p_value(k) <= alpha] with integer arithmetic (used for near-ties)."""
    D0 = d0_of(R, k, tau)
    if D0 > Nx:
        return True
    lo, hi = hyp_support(Nx, D0, n)
    num = 0
    for j in range(lo, min(k, hi) + 1):
        num += math.comb(D0, j) * math.comb(Nx - D0, n - j)
    return F(num, math.comb(Nx, n)) <= ALPHA_EXACT


def passes(k, Nx, R, n, tau=TAU):
    """Fast [p_value(k) <= alpha]: log-space terms with early exit.  Results within
    TIE_TOL of alpha are re-decided exactly, so the indicator is exact."""
    D0 = d0_of(R, k, tau)
    if D0 > Nx:
        return True
    lo = max(0, n - (Nx - D0))
    hi = min(k, n, D0)
    if hi < lo:
        return True
    lognorm = LF[Nx] - LF[n] - LF[Nx - n]
    s = 0.0
    for j in range(lo, hi + 1):
        s += math.exp(LF[D0] - LF[j] - LF[D0 - j]
                      + LF[Nx - D0] - LF[n - j] - LF[Nx - D0 - n + j]
                      - lognorm)
        if s > ALPHA + TIE_TOL:
            return False
    if s < ALPHA - TIE_TOL:
        return True
    return pass_exact(k, Nx, R, n, tau)


def power_at(n, Nx, Dt, R, tau=TAU):
    """power(n) = sum_k hyp_pmf(k; Nx, Dt, n) * [p_value(k) <= alpha]  (fast path)."""
    lo, hi = hyp_support(Nx, Dt, n)
    lognorm = LF[Nx] - LF[n] - LF[Nx - n]
    total = 0.0
    for k in range(lo, hi + 1):
        w = math.exp(LF[Dt] - LF[k] - LF[Dt - k]
                     + LF[Nx - Dt] - LF[n - k] - LF[Nx - Dt - n + k]
                     - lognorm)
        if w < PMF_CUTOFF:
            continue
        if passes(k, Nx, R, n, tau):
            total += w
    return total


def power_reference(n, Nx, Dt, R, tau=TAU):
    """Straightforward reference: full pmf and full hyp_cdf sums, no cutoff, no early exit."""
    total = 0.0
    for k in range(0, min(n, Dt) + 1):
        pk = hyp_pmf(k, Nx, Dt, n)
        if pk <= 0.0:
            continue
        p = p_value(k, Nx, R, n, tau)
        ok = (p <= ALPHA) if abs(p - ALPHA) > TIE_TOL else pass_exact(k, Nx, R, n, tau)
        if ok:
            total += pk
    return total


def power_exact(n, Nx, Dt, R, tau=TAU):
    """Fully exact rational power (small N only; used as a brute-force check)."""
    lo, hi = hyp_support(Nx, Dt, n)
    denom = math.comb(Nx, n)
    total = F(0)
    for k in range(lo, hi + 1):
        if pass_exact(k, Nx, R, n, tau):
            total += F(math.comb(Dt, k) * math.comb(Nx - Dt, n - k), denom)
    return float(total)


# ------------------------------------------------------- sample-size search
def find_nstar(Nx, Dt, R, tau=TAU):
    """Exact minimal n in [0, Nx] with power(n) >= 0.80: full step-1 scan from n = 0.
    (Power is not monotone in n, so no binary search.)"""
    for n in range(Nx + 1):
        pw = power_at(n, Nx, Dt, R, tau)
        if pw >= POWER_TARGET:
            return n, pw
    return None, None


def find_nstar_coarse(Nx, Dt, R, tau=TAU):
    """SPEC method: coarse grid with step max(1, Nx//2000), then step-1 refinement
    between the last failing grid point and the first passing one.  Cross-check only."""
    step = max(1, Nx // 2000)
    grid = list(range(0, Nx + 1, step))
    if grid[-1] != Nx:
        grid.append(Nx)
    prev = None
    for g in grid:
        if power_at(g, Nx, Dt, R, tau) >= POWER_TARGET:
            start = 0 if prev is None else prev + 1
            for m in range(start, g + 1):
                if power_at(m, Nx, Dt, R, tau) >= POWER_TARGET:
                    return m
            return g
        prev = g
    return None


def dips_after(nstar, Nx, Dt, R, npts=400):
    """Diagnostic for non-monotonicity: grid points above n* where power falls back below 0.80."""
    step = max(1, Nx // npts)
    pts = list(range(nstar + step, Nx + 1, step))
    dips = [m for m in pts if power_at(m, Nx, Dt, R) < POWER_TARGET]
    return len(pts), dips


def n0_scan(Nx, R, tau=TAU):
    """Table 1: minimal n with the cohort passing when the audit finds k = 0."""
    for n in range(Nx + 1):
        if passes(0, Nx, R, n, tau):
            return n
    return None


# ---------------------------------------------------------- scenario derivation
def rhu(x):
    """Nearest integer, ties rounded up (x >= 0).  Exact for Fraction input."""
    if isinstance(x, F):
        return math.floor(x + F(1, 2))
    return math.floor(x + 0.5)


def derive(N, pi, r_ai, q_x):
    rel = pi * N
    irr = (1 - pi) * N
    D = rel * (1 - r_ai)
    nx_exact = D + q_x * irr
    R_exact = rel * r_ai
    Nx = rhu(nx_exact)
    return {
        "N": N, "Rel": rel, "Irr": irr, "D_exact": D, "Nx_exact": nx_exact,
        "Nx": Nx, "Nh": N - Nx, "R_exact": R_exact, "R": rhu(R_exact), "Dt": rhu(D),
    }


def classify(achievable, w_af, w_base):
    if not achievable:
        return "AI-first not achievable"
    if w_af <= REC_RATIO * w_base and (w_base - w_af) >= REC_ABS:
        return "AI-first recommended"
    return "AI-first allowed, little benefit"


def run_scenario(N, pi, prof):
    d = derive(N, pi, prof["r_ai"], prof["q_x"])
    Nx, Nh, Dt, R = d["Nx"], d["Nh"], d["Dt"], d["R"]
    nstar, pw = find_nstar(Nx, Dt, R)
    out = {
        "N": N, "pi": float(pi), "profile": prof["name"],
        "r_ai": float(prof["r_ai"]), "q_x": float(prof["q_x"]),
        "Rel": float(d["Rel"]), "N_x": Nx, "N_h": Nh, "R_IU": R,
        "D_exact": float(d["D_exact"]), "D_true": Dt,
        "n_star": nstar, "achievable": nstar is not None,
    }
    if nstar is None:
        out.update({"rec_single": classify(False, None, None), "rec_dual": classify(False, None, None)})
        return out
    n_coarse = find_nstar_coarse(Nx, Dt, R)
    n_pts, dips = dips_after(nstar, Nx, Dt, R)
    c_ha = prof["c_ha"]
    W = {
        "single": F(N),
        "dual": (2 + C_HH) * N,
        "adv": F(N),
        "adv_r": N + c_ha * N,
        "af_1": Nh + nstar,
        "af_2": Nh + 2 * nstar,
        "af_2a": Nh + 2 * nstar + C_REF * nstar,
        "af_2d": 2 * Nh + C_HH * Nh + 2 * nstar,
    }
    Ek = F(nstar) * d["D_exact"] / Nx
    loss = {
        "single": d["Rel"] * (1 - S_H),
        "dual": d["Rel"] * (1 - S_H) ** 2 * 3,
        "af_2": d["R_exact"] * (1 - S_H) + (d["D_exact"] - Ek),
        "af_2d": d["R_exact"] * (1 - S_H) ** 2 * 3 + (d["D_exact"] - Ek),
    }
    out.update({
        "audit_share": nstar / Nx,
        "power_at_nstar": pw,
        "power_at_Nx": power_at(Nx, Nx, Dt, R),
        "n_star_coarse_refine": n_coarse,
        "post_nstar_grid_points": n_pts,
        "post_nstar_dips": len(dips),
        "post_nstar_first_dip": dips[0] if dips else None,
        "W": {k: float(v) for k, v in W.items()},
        "savings_af2_vs_single": float(1 - W["af_2"] / W["single"]),
        "savings_af2d_vs_dual": float(1 - W["af_2d"] / W["dual"]),
        "loss": {k: float(v) for k, v in loss.items()},
        "E_k_sample": float(Ek),
        "rec_single": classify(True, W["af_2"], W["single"]),
        "rec_dual": classify(True, W["af_2d"], W["dual"]),
        "_W": W, "_loss": loss,   # exact values, stripped before JSON output
    })
    return out


def run_table1():
    t1 = []
    for R in R_GRID:
        D0 = d0_of(R, 0)
        cells = []
        for Nx in NX_GRID_T1:
            n0 = n0_scan(Nx, R)
            cells.append({"N_x": Nx, "n0": n0, "share": n0 / Nx})
        t1.append({"R_IU": R, "D0": D0, "f_approx": 1 - ALPHA ** (1.0 / D0), "cells": cells})
    t1b = []
    for R in R_GRID:
        row = {"R_IU": R}
        for label, tau in TAUS_T1B:
            D0 = d0_of(R, 0, tau)
            n0 = n0_scan(20000, R, tau)
            row[label] = {"D0": D0, "f_approx": 1 - ALPHA ** (1.0 / D0), "n0": n0, "share": n0 / 20000}
        t1b.append(row)
    return t1, t1b


def run_table3():
    rows = []
    W_single = F(20000)
    for r in R3_GRID:
        d = derive(20000, F(2, 100), r, F(85, 100))
        nstar, pw = find_nstar(d["Nx"], d["Dt"], d["R"])
        W_af2 = d["Nh"] + 2 * nstar
        rows.append({
            "r_ai": float(r), "D_exact": float(d["D_exact"]), "D_true": d["Dt"],
            "N_x": d["Nx"], "R_IU": d["R"], "n_star": nstar, "audit_share": nstar / d["Nx"],
            "power_at_nstar": pw, "W_af_2": W_af2,
            "savings_af2_vs_single": float(1 - F(W_af2) / W_single),
            "rec_single": classify(True, F(W_af2), W_single),
        })
    return rows


def obs_distribution(Dt, Nx, n, s):
    """Distribution of the observed relevant count k_obs: true K ~ Hypergeom(Nx, Dt, n),
    then each truly relevant sampled record is detected with probability s (binomial thinning)."""
    dist = [0.0] * (Dt + 1)
    sf = float(s)
    for j in range(Dt + 1):
        pj = hyp_pmf(j, Nx, Dt, n)
        if pj == 0.0:
            continue
        for m in range(j + 1):
            dist[m] += pj * math.comb(j, m) * sf ** m * (1.0 - sf) ** (j - m)
    return dist


def t4_case(d, n, s):
    Nx, Dt, R = d["Nx"], d["Dt"], d["R"]
    dist = obs_distribution(Dt, Nx, n, s)
    pv = [passes(m, Nx, R, n, TAU) for m in range(Dt + 1)]
    E_k = sum(m * dist[m] for m in range(Dt + 1))
    # Conditional automation recall: R is fixed; human-stratum misses are excluded.
    # Only observed relevant sampled records are rescued.
    E_true = sum(dist[m] * (R + m) / (R + Dt) for m in range(Dt + 1))
    # SPEC point estimate, which scales the observed count by Nx/n
    E_est = sum(dist[m] * (R + m) / (R + m * Nx / n) for m in range(Dt + 1))
    P_pass = sum(dist[m] for m in range(Dt + 1) if pv[m])
    return {
        "mass": sum(dist), "E_k": E_k, "E_k_expected": float(s) * n * Dt / Nx,
        "E_true_recall": E_true, "E_est_recall": E_est,
        "bias_pp": 100.0 * (E_est - E_true), "P_pass": P_pass,
    }


def run_table4(n, d_strong, d_bad):
    rows = []
    for label, s in [S_PERFECT] + S_REF_GRID:
        st = t4_case(d_strong, n, s)
        sb = t4_case(d_bad, n, s)
        rows.append({"label": label, "s_ref": float(s), **st, "P_pass_bad": sb["P_pass"]})
    return rows


def run_table4b(n):
    """Extra diagnostic (not in SPEC): false pass at the recall boundary.
    Hypothetical AI with R_IU = 372 and D_true = 20 (true recall before audit 0.949, just below 0.95),
    N_x = 20 + 0.85 * 19,600 = 16,680, same n as Table 4."""
    R_b, D_b = 372, 20
    Nx_b = rhu(F(D_b) + F(85, 100) * F(19600))
    d = {"Nx": Nx_b, "Dt": D_b, "R": R_b}
    rows = []
    for label, s in [S_PERFECT] + S_REF_GRID:
        rows.append({"label": label, "s_ref": float(s), "P_pass_boundary": t4_case(d, n, s)["P_pass"]})
    return {"R_IU": R_b, "D_true": D_b, "N_x": Nx_b, "recall_before_audit": R_b / (R_b + D_b),
            "n": n, "rows": rows}


# ------------------------------------------------------------- self-checks
def check_hyp_bruteforce(rng):
    N = 1000
    max_pmf_err = max_cdf_err = max_norm_err = 0.0
    combos = 0
    for D in [0, 1, 7, 100, 333, 999, 1000]:
        for n in [0, 1, 13, 250, 999, 1000]:
            lo, hi = hyp_support(N, D, n)
            denom = math.comb(N, n)
            cum = 0
            cdf_ex = []
            for j in range(lo, hi + 1):
                num = math.comb(D, j) * math.comb(N - D, n - j)
                cum += num
                pe = float(F(num, denom))
                max_pmf_err = max(max_pmf_err, abs(hyp_pmf(j, N, D, n) - pe))
                cdf_ex.append(F(cum, denom))
            max_norm_err = max(max_norm_err, abs(sum(hyp_pmf(j, N, D, n) for j in range(lo, hi + 1)) - 1.0))
            ks = {0, N, lo, hi} | {rng.randint(0, N) for _ in range(12)}
            for k in ks:
                ex = 0.0 if k < lo else float(cdf_ex[min(k, hi) - lo])
                max_cdf_err = max(max_cdf_err, abs(hyp_cdf(k, N, D, n) - ex))
            combos += 1
    return max_pmf_err, max_cdf_err, max_norm_err, combos


def run_self_checks(rng, t1, t3, t4, scen):
    checks = []

    def add(name, ok, detail):
        checks.append({"check": name, "passed": bool(ok), "detail": detail})

    pe, ce, ne, nc = check_hyp_bruteforce(rng)
    add("log-space hyp_pmf/hyp_cdf vs exact math.comb brute force (N_x = 1,000)",
        pe <= 1e-9 and ce <= 1e-9 and ne <= 1e-9,
        f"{nc} (D, n) pairs; max abs pmf error {pe:.1e}; max abs cdf error {ce:.1e} (tol 1e-9); "
        f"max abs(sum of pmf - 1) {ne:.1e}")

    d0 = d0_of(100, 0, TAU)
    add("D0(0) = 6 for R_IU = 100, tau = 0.95", d0 == 6, f"D0 = {d0}")

    f_closed = 1 - ALPHA ** (1 / 6)
    s50 = n0_scan(50000, 100) / 50000
    s20 = n0_scan(20000, 100) / 20000
    add("large-N audit share for R_IU = 100 is 39.3%",
        round(100 * f_closed, 1) == 39.3 and round(100 * s50, 1) == 39.3 and round(100 * s20, 1) == 39.3,
        f"closed form 1-0.05^(1/6) = {100*f_closed:.2f}%; exact n0/N_x: N_x=50,000 -> {100*s50:.2f}%, "
        f"N_x=20,000 -> {100*s20:.2f}%")

    ok_tie = passes(0, 1000, 10, 950, TAU) and not passes(0, 1000, 10, 949, TAU)
    add("exact tie handling: p = 50/1000 = 0.05 passes, p = 51/1000 fails (R_IU = 10, N_x = 1,000)",
        ok_tie, f"n0 = 950 -> share 95.0% (ties are resolved exactly, p <= alpha)")

    mism = 0
    tested = 0
    for _ in range(3000):
        Nx = rng.choice([1000, 5000, 20000, 50000])
        R = rng.choice(R_GRID)
        n = rng.randint(0, Nx // 2)
        k = rng.randint(0, min(80, n))
        p = p_value(k, Nx, R, n, TAU)
        if abs(p - ALPHA) <= TIE_TOL:
            continue
        tested += 1
        if (p <= ALPHA) != passes(k, Nx, R, n, TAU):
            mism += 1
    add("fast pass indicator (early exit, log terms) vs SPEC p_value <= alpha",
        mism == 0, f"{tested} random non-tie cases, {mism} mismatches")

    cases = [(1692, 0, 10), (4166, 1, 99), (3830, 5, 495), (16664, 4, 396), (16688, 28, 372),
             (27175, 175, 4825), (38300, 50, 4950), (41660, 10, 990), (1194, 0, 10), (2943, 4, 96)]
    worst = 0.0
    for Nx, Dt, R in cases:
        for _ in range(3):
            n = rng.randint(1, Nx)
            worst = max(worst, abs(power_at(n, Nx, Dt, R) - power_reference(n, Nx, Dt, R)))
    add("fast power(n) vs full-sum reference power(n) (no cutoff, no early exit)",
        worst <= 1e-9, f"{len(cases) * 3} (N_x, D, n) cases; max abs difference {worst:.1e}")

    worst = 0.0
    for Dt, R in [(5, 100), (40, 20), (12, 60)]:
        for n in [30, 150, 400, 700]:
            worst = max(worst, abs(power_at(n, 1000, Dt, R) - power_exact(n, 1000, Dt, R)))
    add("power(n) vs fully exact rational power (N_x = 1,000)",
        worst <= 1e-9, f"12 cases; max abs difference {worst:.1e}")

    ok_closed = all(n0_scan(Nx, 10) == math.ceil(F(19, 20) * Nx) for Nx in NX_GRID_T1)
    add("closed form for D0 = 1 (R_IU = 10): n0 = ceil(0.95 N_x)", ok_closed,
        "n0 = " + ", ".join(f"{n0_scan(Nx, 10):,}" for Nx in NX_GRID_T1))

    ok_p = all(c["achievable"] and c["power_at_Nx"] >= 1 - 1e-9 and c["power_at_nstar"] >= POWER_TARGET
               for c in scen)
    add("power(N_x) = 1 (census always passes) and power(n*) >= 0.80 in every scenario", ok_p,
        f"{len(scen)} scenarios checked")

    s2 = [c for c in scen if c["N"] == 20000 and c["pi"] == 0.02 and c["profile"] == "strong"][0]
    t3_row = [r for r in t3 if abs(r["r_ai"] - 0.99) < 1e-12][0]
    add("Table 3 row r_ai = 0.99 reproduces Table 2 strong, N = 20,000, pi = 2%",
        t3_row["n_star"] == s2["n_star"] and t3_row["N_x"] == s2["N_x"],
        f"n* {t3_row['n_star']} vs {s2['n_star']}")

    ok_t4 = all(abs(r["bias_pp"]) >= 0 and abs(r["E_k"] - r["E_k_expected"]) <= 1e-9
                and abs(r["mass"] - 1.0) <= 1e-9 for r in t4)
    add("Table 4: observed-k distribution sums to 1 and E[k_obs] = s_ref * n * D_true / N_x", ok_t4,
        f"max abs(E[k_obs] - s n D / N_x) = {max(abs(r['E_k'] - r['E_k_expected']) for r in t4):.1e}")

    ok_coarse = all(c["n_star_coarse_refine"] >= c["n_star"] for c in scen if c["achievable"])
    same = sum(1 for c in scen if c["n_star_coarse_refine"] == c["n_star"])
    add("spec coarse-grid + refine result is never below the exact minimal n*", ok_coarse,
        f"coarse method equals exact n* in {same} of {len(scen)} scenarios")
    # Exhaustive exact finite-population false-certification check. The null
    # recall varies with rescued k; count only outcomes both passing and below tau.
    worst_error = F(0)
    cases = 0
    for Nx in [5, 12, 25]:
        for R in [0, 1, 5, 15]:
            for Dt in range(Nx + 1):
                if R + Dt == 0:
                    continue
                for n in range(Nx + 1):
                    lo, hi = hyp_support(Nx, Dt, n)
                    error = F(0)
                    for k in range(lo, hi + 1):
                        below = F(R + k, R + Dt) < TAU
                        if below and pass_exact(k, Nx, R, n):
                            error += F(math.comb(Dt, k) * math.comb(Nx - Dt, n - k),
                                       math.comb(Nx, n))
                    worst_error = max(worst_error, error)
                    cases += 1
    add("exact-label conditional-recall false certification <= alpha (exhaustive small frames)",
        worst_error <= ALPHA_EXACT,
        f"{cases} fixed (N_x, R, D, n) configurations; maximum {float(worst_error):.4f}")
    # Correct arithmetic is insufficient to restore coverage under noisy labels.
    # R=10, one true relevant in X, n=950/1000: observing zero passes, but
    # missing a sampled relevant adds 0.95*(1-s) to the nominal 0.05 event.
    imperfect_error = t4_case({"Nx": 1000, "Dt": 1, "R": 10}, 950, F(9, 10))["P_pass"]
    add("imperfect-label counterexample exceeds alpha (s_ref=0.90)",
        abs(imperfect_error - 0.145) <= 1e-9 and imperfect_error > ALPHA,
        f"R=10, N_x=1000, D=1, n=950: false certification {imperfect_error:.4f}")
    return checks


# ------------------------------------------------------------------ rendering
def fmt_int(x):
    return f"{int(math.floor(float(x) + 0.5)):,}"


def fmt_pct(x):
    return f"{100 * float(x):.1f}%"


def fmt_dec(x, d=1):
    return f"{float(x):,.{d}f}"


def md_table(headers, rows):
    lines = ["| " + " | ".join(headers) + " |",
             "| " + " | ".join("---" for _ in headers) + " |"]
    for row in rows:
        lines.append("| " + " | ".join(str(c) for c in row) + " |")
    return "\n".join(lines)


def build_markdown(t1, t1b, scen, t3, t4, t4b, checks):
    out = []
    out.append("# Workload / break-even results (ADR 0005)\n")
    out.append("Generated by `workload.py` from SPEC.md (run as `python3 -I workload.py`). "
               "Standard library only. Counts are integers; percentages have 1 decimal; "
               "workloads are in human record judgments.\n")
    out.append("## Interpretation limits\n\n"
               "All performance parameters are scenario assumptions. The test conditions on fixed, correct "
               "reference labels and R_IU; it does not bound total true stage recall. Audit frames and sizes "
               "must be frozen before drawing. R_IU for sample planning is optimistic (before human misses); "
               "the separate loss forecast applies human errors later. Workloads assume successful finalization "
               "and exclude fallback screening, targeted review, administration, full-text work and AI costs. "
               "Losses are unconditional hypothetical-finalization scenarios with perfect audit rescue, not "
               "post-pass losses or expected losses of the fallback policy. Rounded D_true is a fixed count, "
               "not a stochastic prevalence model. Reference sensitivity assumptions, including the optimistic "
               "0.995 row, are not measured guarantees. Table 4 assumes independent per-record detection "
               "and no false positives; its probabilities do not restore nominal coverage with imperfect labels.\n")

    out.append("## Parameters\n")
    out.append(md_table(["Parameter", "Value"], [
        ["Target recall tau", "0.95 (Tables 1-4); 0.90 and 0.98 in Table 1b"],
        ["One-sided alpha", "0.05"],
        ["Power target", "0.80"],
        ["Human single-screener sensitivity s_h", "0.90 (expected-loss column only)"],
        ["Human-human disagreement c_hh", "0.10 (one adjudication per disagreement)"],
        ["AI-human disagreement c_ha", "0.12 (strong), 0.20 (weak)"],
        ["Reference-standard disagreement c_ref", "0.10"],
        ["AI profile strong", "r_ai = 0.99, q_x = 0.85"],
        ["AI profile weak", "r_ai = 0.965, q_x = 0.60"],
        ["N (records)", "2,000; 5,000; 20,000; 50,000"],
        ["Prevalence pi", "0.5%; 2.0%; 10.0%"],
        ["Derived counts", "Rel = pi N; Irr = (1-pi) N; D = Rel (1-r_ai); N_x = D + q_x Irr; "
                           "N_h = N - N_x; R_IU = Rel r_ai (exact expectations, Fraction arithmetic)"],
        ["Counts used in formulas", "N_x, N_h, R_IU, D_true = round(D) to nearest integer (ties up)"],
        ["Recommendation rule", "AI-first recommended if achievable and W_af <= 0.8 x W_base and "
                                "W_base - W_af >= 500; AI-first allowed, little benefit if achievable "
                                "otherwise; AI-first not achievable if no n reaches power 0.80"],
    ]))
    out.append("")
    out.append("Method notes: n* is the exact minimal n in [0, N_x] from a step-1 scan starting at n = 0 "
               "(power is not monotone, so no binary search). The SPEC coarse grid + refinement is also run "
               "and is reported in Table 2 diagnostics. Ties (p exactly equal to alpha, possible when D0 = 1 "
               "and N_x is a multiple of 20) count as a pass and are decided with integer arithmetic. "
               "Expected losses use exact expectations. Table 4's realised recall for an observed count "
               "k_obs is (R_IU + k_obs)/(R_IU + D_true), because a relevant record the reference standard "
               "misses is not rescued.\n")

    out.append("## Table 1: audit fraction needed when the audit finds zero relevant (tau = 0.95, alpha = 0.05)\n")
    rows = []
    for r in t1:
        rows.append([fmt_int(r["R_IU"]), r["D0"], fmt_pct(r["f_approx"])] +
                    [f"{fmt_int(c['n0'])} ({fmt_pct(c['share'])})" for c in r["cells"]])
    out.append(md_table(["R_IU", "D0(0)", "Large-N f = 1-alpha^(1/D0)", "N_x = 1,000", "N_x = 5,000",
                         "N_x = 20,000", "N_x = 50,000"], rows))
    out.append("")

    out.append("## Table 1b: same at tau = 0.90 and tau = 0.98 (N_x = 20,000)\n")
    rows = []
    for r in t1b:
        a, b = r["0.90"], r["0.98"]
        rows.append([fmt_int(r["R_IU"]),
                     a["D0"], fmt_pct(a["f_approx"]), f"{fmt_int(a['n0'])} ({fmt_pct(a['share'])})",
                     b["D0"], fmt_pct(b["f_approx"]), f"{fmt_int(b['n0'])} ({fmt_pct(b['share'])})"])
    out.append(md_table(["R_IU", "tau=0.90: D0(0)", "f approx", "tau=0.90: n0 (share)",
                         "tau=0.98: D0(0)", "f approx", "tau=0.98: n0 (share)"], rows))
    out.append("")

    out.append("## Table 2a: planned sample size with power (SPEC scenario grid)\n")
    rows = []
    for c in scen:
        rows.append([fmt_int(c["N"]), fmt_pct(c["pi"]), c["profile"], fmt_int(c["Rel"]),
                     fmt_int(c["N_x"]), fmt_int(c["N_h"]), fmt_int(c["R_IU"]), fmt_dec(c["D_exact"], 2),
                     c["D_true"], fmt_int(c["n_star"]), fmt_pct(c["audit_share"]),
                     fmt_pct(c["power_at_nstar"])])
    out.append(md_table(["N", "pi", "AI", "Rel", "N_x", "N_h", "R_IU", "D (expected)", "D_true",
                         "n* (power >= 0.80)", "n*/N_x", "power(n*)"], rows))
    out.append("")
    diag_rows = []
    for c in scen:
        diag_rows.append([fmt_int(c["N"]), fmt_pct(c["pi"]), c["profile"], fmt_int(c["n_star"]),
                          fmt_int(c["n_star_coarse_refine"]) if c["n_star_coarse_refine"] is not None else "n/a",
                          "yes" if c["n_star_coarse_refine"] == c["n_star"] else "NO",
                          f"{c['post_nstar_dips']} of {c['post_nstar_grid_points']}"])
    out.append("Diagnostics for n* (power is non-monotone): spec coarse grid + refinement (step max(1, N_x // 2000)), "
               "and the number of grid points above n* (step max(1, N_x // 400)) at which power falls back below 0.80.\n")
    out.append(md_table(["N", "pi", "AI", "n* (exact scan)", "n* (spec coarse + refine)", "agree",
                         "grid points above n* with power < 0.80"], diag_rows))
    out.append("")

    out.append("## Table 2b: workloads (human record judgments) and savings\n")
    rows = []
    for c in scen:
        W = c["W"]
        rows.append([fmt_int(c["N"]), fmt_pct(c["pi"]), c["profile"], fmt_int(W["single"]),
                     fmt_int(W["dual"]), fmt_int(W["adv_r"]), fmt_int(W["af_2"]), fmt_int(W["af_2d"]),
                     fmt_pct(c["savings_af2_vs_single"]), fmt_pct(c["savings_af2d_vs_dual"])])
    out.append(md_table(["N", "pi", "AI", "W_single", "W_dual", "W_adv_r", "W_af_2", "W_af_2d",
                         "Savings W_af_2 vs W_single", "Savings W_af_2d vs W_dual"], rows))
    out.append("")
    out.append("Workloads are shown rounded to the nearest integer; savings use unrounded values. "
               "Negative savings mean more work than the baseline.\n")

    out.append("## Table 2c: expected relevant records lost at the stage, and recommendation\n")
    rows = []
    for c in scen:
        L = c["loss"]
        rows.append([fmt_int(c["N"]), fmt_pct(c["pi"]), c["profile"], fmt_dec(L["single"]),
                     fmt_dec(L["dual"]), fmt_dec(L["af_2"]), fmt_dec(L["af_2d"]),
                     c["rec_single"], c["rec_dual"]])
    out.append(md_table(["N", "pi", "AI", "Lost: single", "Lost: dual", "Lost: AI-first (W_af_2)",
                         "Lost: AI-first, dual baseline (W_af_2d)", "Recommendation vs single (W_af_2 vs W_single)",
                         "Recommendation vs dual (W_af_2d vs W_dual)"], rows))
    out.append("")
    n_more = sum(1 for c in scen if c["loss"]["af_2"] > c["loss"]["single"])
    worst_s = max(scen, key=lambda c: c["loss"]["af_2"] / c["loss"]["single"])
    worst_d = max(scen, key=lambda c: c["loss"]["af_2d"] / c["loss"]["dual"])
    n_d0 = sum(1 for c in scen if c["D_true"] == 0)
    n_dips = sum(1 for c in scen if c["post_nstar_dips"] > 0)

    def label(c):
        return f"N = {c['N']:,}, pi = {100 * c['pi']:.1f}%, {c['profile']} AI"

    out.append("Notes derived from the results above:\n")
    out.append(f"- Protection is not part of the recommendation rule. Expected relevant records lost with AI-first "
               f"(W_af_2) exceed single screening in {n_more} of {len(scen)} scenarios; the largest excess is "
               f"{100 * (worst_s['loss']['af_2'] / worst_s['loss']['single'] - 1):+.1f}% ({label(worst_s)}). "
               f"Against dual screening (W_af_2d vs dual) the largest excess is "
               f"{100 * (worst_d['loss']['af_2d'] / worst_d['loss']['dual'] - 1):+.1f}% ({label(worst_d)}).")
    out.append(f"- D_true = 0 (expected relevant records in X below 0.5) in {n_d0} scenarios. There power is 0 or 1 "
               f"by construction, and n* is simply the zero-found threshold (the k = 0 test).")
    out.append(f"- Power is non-monotone: in {n_dips} of {len(scen)} scenarios some grid point above n* has power below "
               f"0.80, so n* is the first crossing of the threshold, not a safe planning margin.\n")

    out.append("## Table 3: realism check, N = 20,000, pi = 2%, q_x = 0.85\n")
    rows = []
    for r in t3:
        rows.append([fmt_dec(r["r_ai"], 3), fmt_dec(r["D_exact"], 2), r["D_true"], fmt_int(r["N_x"]),
                     fmt_int(r["R_IU"]), fmt_int(r["n_star"]), fmt_pct(r["audit_share"]),
                     fmt_pct(r["power_at_nstar"]), fmt_int(r["W_af_2"]),
                     fmt_pct(r["savings_af2_vs_single"]), r["rec_single"]])
    out.append(md_table(["r_ai", "D (expected)", "D_true", "N_x", "R_IU", "n*", "n*/N_x", "power(n*)",
                         "W_af_2", "Savings vs W_single", "Recommendation"], rows))
    out.append("")

    s2 = [c for c in scen if c["N"] == T4_N and c["pi"] == float(T4_PI) and c["profile"] == "strong"][0]
    out.append(f"## Table 4: reference-standard bias (N = 20,000, pi = 2%, n = n* = {s2['n_star']:,} "
               f"from Table 2, strong AI: N_x = {s2['N_x']:,}, R_IU = {s2['R_IU']:,}, D_true = {s2['D_true']})\n")
    out.append("Realised recall = (R_IU + k_obs)/(R_IU + D_true). Point estimate = (R_IU + k_obs)/(R_IU + k_obs N_x/n). "
               "The false-pass column uses a bad AI with r_ai = 0.93 (true recall before audit 0.930; "
               "N_x = 16,688, D_true = 28, R_IU = 372) and the same n.\n")
    rows = []
    for r in t4:
        rows.append([r["label"], f"{r['s_ref']:.3f}", fmt_dec(r["E_k"], 2), fmt_pct(r["E_true_recall"]),
                     fmt_pct(r["E_est_recall"]), f"{r['bias_pp']:+.2f} pp", fmt_pct(r["P_pass"]),
                     fmt_pct(r["P_pass_bad"])])
    out.append(md_table(["Reference standard", "s_ref", "E[k_obs] (strong)", "E[realised recall] (strong)",
                         "E[point estimate] (strong)", "Upward bias, percentage points (estimate - realised)",
                         "P(pass), strong AI", "P(pass), bad AI (false pass)"], rows))
    out.append("")
    out.append("The bias is shown in percentage points with 2 decimals because the values are small.")
    out.append("")

    out.append("## Table 4b (extra diagnostic, not in SPEC): false pass at the recall boundary\n")
    out.append(f"Hypothetical AI with R_IU = {t4b['R_IU']} and D_true = {t4b['D_true']}: true recall before audit "
               f"{100 * t4b['recall_before_audit']:.1f}% (just below 0.95), N_x = {t4b['N_x']:,}, n = {t4b['n']:,}. "
               f"Pass probability at the boundary D = D0(0) = {t4b['D_true']}, the smallest D whose recall is below 0.95:\n")
    rows = [[r["label"], f"{r['s_ref']:.3f}", fmt_pct(r["P_pass_boundary"])] for r in t4b["rows"]]
    out.append(md_table(["Reference standard", "s_ref", "P(pass) at boundary (D = 20)"], rows))
    out.append("")

    out.append("## Self-check\n")
    rows = [[c["check"], "PASS" if c["passed"] else "FAIL", c["detail"]] for c in checks]
    out.append(md_table(["Check", "Result", "Detail"], rows))
    npass = sum(1 for c in checks if c["passed"])
    out.append(f"\n{npass} of {len(checks)} self-checks passed.\n")
    return "\n".join(out)


def json_clean(scen):
    cleaned = []
    for c in scen:
        cleaned.append({k: v for k, v in c.items() if not k.startswith("_")})
    return cleaned


def main():
    t_start = time.time()
    rng = random.Random(20260831)

    t1, t1b = run_table1()
    scen = []
    for N in N_GRID:
        for pi in PI_GRID:
            for prof in PROFILES:
                scen.append(run_scenario(N, pi, prof))
    t3 = run_table3()
    s2 = [c for c in scen if c["N"] == T4_N and c["pi"] == float(T4_PI) and c["profile"] == "strong"][0]
    d_strong = derive(T4_N, T4_PI, F(99, 100), F(85, 100))
    d_bad = derive(T4_N, T4_PI, R_BAD, F(85, 100))
    t4 = run_table4(s2["n_star"], d_strong, d_bad)
    t4b = run_table4b(s2["n_star"])

    checks = run_self_checks(rng, t1, t3, t4, scen)
    runtime = time.time() - t_start

    md = build_markdown(t1, t1b, scen, t3, t4, t4b, checks)
    with open(RESULTS_MD, "w", encoding="utf-8") as fh:
        fh.write(md)

    payload = {
        "parameters": {"tau": 0.95, "alpha": ALPHA, "power_target": POWER_TARGET,
                       "s_h": float(S_H), "c_hh": float(C_HH), "c_ref": float(C_REF)},
        "table1": t1, "table1b": t1b, "table2": json_clean(scen), "table3": t3, "table4": t4,
        "table4b_extra_boundary": t4b,
        "self_checks": checks,
    }
    with open(RESULTS_JSON, "w", encoding="utf-8") as fh:
        json.dump(payload, fh, indent=2)

    npass = sum(1 for c in checks if c["passed"])
    print(f"Self-checks: {npass}/{len(checks)} passed")
    for c in checks:
        print(f"  [{'PASS' if c['passed'] else 'FAIL'}] {c['check']}: {c['detail']}")
    print(f"Scenarios: {len(scen)}; achievable: {sum(1 for c in scen if c['achievable'])}")
    short = {"AI-first recommended": "recommended",
             "AI-first allowed, little benefit": "little benefit",
             "AI-first not achievable": "not achievable"}
    print("Recommendation vs single (N/pi/AI): " + "; ".join(
        f"{c['N']}/{c['pi'] * 100:g}%/{c['profile']}: {short[c['rec_single']]}" for c in scen))
    print("Recommendation vs dual   (N/pi/AI): " + "; ".join(
        f"{c['N']}/{c['pi'] * 100:g}%/{c['profile']}: {short[c['rec_dual']]}" for c in scen))
    print(f"Wrote {RESULTS_MD} and {RESULTS_JSON} in {runtime:.1f} s")
    return 0 if npass == len(checks) else 1


if __name__ == "__main__":
    sys.exit(main())
