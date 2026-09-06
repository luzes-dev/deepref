use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use deepref_application::deduplication::{
    DedupeCandidate, score_candidate, select_fuzzy_candidate,
};
use deepref_domain::{ReportId, normalize_bibliography_title};
use uuid::Uuid;

fn generate_deterministic_candidates(count: usize) -> Vec<DedupeCandidate> {
    let base_titles = [
        "Systematic review of pharmacological treatments for major depressive disorder",
        "Clinical efficacy and safety of novel oral anticoagulants in atrial fibrillation",
        "Machine learning approaches to biomarker discovery in oncology clinical trials",
        "A randomized double-blind placebo-controlled trial of metformin in prediabetes",
        "Long-term outcomes of endovascular thrombectomy versus standard medical care",
        "Diagnostic accuracy of rapid point-of-care testing for respiratory pathogens",
        "Genomic epidemiology and transmission dynamics of emerging antimicrobial resistance",
        "Comparative effectiveness of biologic therapies in moderate-to-severe rheumatoid arthritis",
    ];

    let authors = [
        "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis",
    ];

    (0..count)
        .map(|index| {
            let title_idx = index % base_titles.len();
            let author_idx = index % authors.len();
            let year = 2000 + ((index % 25) as i32);
            let title_variant = format!("{} (cohort study #{})", base_titles[title_idx], index);

            DedupeCandidate {
                report_id: ReportId::new(Uuid::from_u128(index as u128 + 1)),
                title: Some(title_variant),
                first_author: Some(authors[author_idx].to_owned()),
                publication_year: Some(year),
                exact_identifier_match: false,
                conflicting_identifier: false,
            }
        })
        .collect()
}

fn bench_title_normalization(c: &mut Criterion) {
    let mut group = c.benchmark_group("dedup_title_normalization");

    let short_title = "Randomized trial of metformin in type 2 diabetes";
    let noisy_title = "Effect of SGLT2 Inhibitors vs GLP-1 Receptor Agonists on Cardiovascular and Renal Outcomes: A Systematic Review, Network Meta-Analysis &amp; Multi-Center Trial (Phase III/IV) — 2024 Update!";

    group.bench_function("short_title", |b| {
        b.iter(|| normalize_bibliography_title(black_box(short_title)))
    });

    group.bench_function("noisy_title_with_unicode_and_punctuation", |b| {
        b.iter(|| normalize_bibliography_title(black_box(noisy_title)))
    });

    group.finish();
}

fn bench_single_candidate_scoring(c: &mut Criterion) {
    let mut group = c.benchmark_group("dedup_single_candidate_scoring");

    let source_title =
        "Systematic review of pharmacological treatments for major depressive disorder";
    let source_author = "Smith";
    let source_year = 2022;

    let matching_candidate = DedupeCandidate {
        report_id: ReportId::new(Uuid::from_u128(101)),
        title: Some(
            "A Systematic Review of Pharmacological Treatments for Major Depressive Disorders"
                .to_owned(),
        ),
        first_author: Some("Smith".to_owned()),
        publication_year: Some(2022),
        exact_identifier_match: false,
        conflicting_identifier: false,
    };

    let dissimilar_candidate = DedupeCandidate {
        report_id: ReportId::new(Uuid::from_u128(102)),
        title: Some("Pediatric dental hygiene guidelines in rural clinics".to_owned()),
        first_author: Some("Yamamoto".to_owned()),
        publication_year: Some(2015),
        exact_identifier_match: false,
        conflicting_identifier: false,
    };

    group.bench_function("matching_candidate", |b| {
        b.iter(|| {
            score_candidate(
                black_box(Some(source_title)),
                black_box(Some(source_author)),
                black_box(Some(source_year)),
                black_box(&matching_candidate),
            )
        })
    });

    group.bench_function("dissimilar_candidate", |b| {
        b.iter(|| {
            score_candidate(
                black_box(Some(source_title)),
                black_box(Some(source_author)),
                black_box(Some(source_year)),
                black_box(&dissimilar_candidate),
            )
        })
    });

    group.finish();
}

fn bench_shortlist_selection(c: &mut Criterion) {
    let mut group = c.benchmark_group("dedup_shortlist_selection");

    let query_title =
        "Systematic review of pharmacological treatments for major depressive disorder";
    let query_author = "Smith";
    let query_year = 2022;

    for size in [50, 200, 1000] {
        let candidates = generate_deterministic_candidates(size);

        group.bench_with_input(
            BenchmarkId::new("candidates", size),
            &candidates,
            |b, cands| {
                b.iter(|| {
                    select_fuzzy_candidate(
                        black_box(Some(query_title)),
                        black_box(Some(query_author)),
                        black_box(Some(query_year)),
                        black_box(cands.clone()),
                    )
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_title_normalization,
    bench_single_candidate_scoring,
    bench_shortlist_selection
);
criterion_main!(benches);
