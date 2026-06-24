use criterion::{black_box, criterion_group, criterion_main, Criterion};
use needle::indexing::bm25::BM25Index;

fn bm25_scoring_bench(c: &mut Criterion) {
    c.bench_function("bm25_score_query", |b| {
        let index = BM25Index::new();
        let query_terms = black_box(vec![
            "retry".to_string(),
            "backoff".to_string(),
            "http".to_string(),
        ]);
        b.iter(|| {
            let _ = index.score(&query_terms, 42, 500);
        })
    });
}

criterion_group!(benches, bm25_scoring_bench);
criterion_main!(benches);
