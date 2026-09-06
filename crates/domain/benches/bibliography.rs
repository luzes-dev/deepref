use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use deepref_domain::{normalize_bibliography_title, normalize_doi};

fn bench_doi_normalization(c: &mut Criterion) {
    let mut group = c.benchmark_group("bibliography_doi_normalization");

    let clean_doi = "10.1016/j.cell.2023.01.001";
    let url_prefixed_doi = "https://doi.org/10.1056/NEJMoa2201445";
    let dx_doi_url = "http://dx.doi.org/10.1136/bmj.n1234";
    let complex_doi = "  doi:10.1002/(SICI)1097-0142(19980101)82:1<1::AID-CNCR1>3.0.CO;2-E.  ";

    group.bench_function("clean_standard_doi", |b| {
        b.iter(|| normalize_doi(black_box(clean_doi)))
    });

    group.bench_function("https_url_prefixed_doi", |b| {
        b.iter(|| normalize_doi(black_box(url_prefixed_doi)))
    });

    group.bench_function("http_dx_prefixed_doi", |b| {
        b.iter(|| normalize_doi(black_box(dx_doi_url)))
    });

    group.bench_function("complex_unnormalized_doi", |b| {
        b.iter(|| normalize_doi(black_box(complex_doi)))
    });

    group.finish();
}

fn bench_title_normalization(c: &mut Criterion) {
    let mut group = c.benchmark_group("bibliography_title_normalization");

    let plain_title = "A Comprehensive Guide to Modern Evidence Synthesis";
    let complex_title = "Interventions for Preventing Obesity in Children: A Systematic Review, Meta-Analysis, &amp; GRADE Assessment of 153 Randomized Controlled Trials (1990–2023) [Protocol Update]";

    group.bench_function("plain_title", |b| {
        b.iter(|| normalize_bibliography_title(black_box(plain_title)))
    });

    group.bench_function("complex_title_with_unicode_html_punctuation", |b| {
        b.iter(|| normalize_bibliography_title(black_box(complex_title)))
    });

    group.finish();
}

criterion_group!(benches, bench_doi_normalization, bench_title_normalization);
criterion_main!(benches);
