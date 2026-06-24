use criterion::{black_box, criterion_group, criterion_main, Criterion};
use needle::indexing::hnsw::HnswIndex;

fn hnsw_insertion_bench(c: &mut Criterion) {
    c.bench_function("hnsw_insert_1000_nodes", |b| {
        b.iter(|| {
            let mut index = HnswIndex::new(384);
            for i in 0..1000 {
                let embedding = vec![0.1; 384];
                let _ = index.add_node(black_box(i), black_box(embedding));
            }
        })
    });
}

fn hnsw_search_bench(c: &mut Criterion) {
    c.bench_function("hnsw_search_knn_k10", |b| {
        let mut index = HnswIndex::new(384);
        for i in 0..1000 {
            let embedding = vec![0.1; 384];
            let _ = index.add_node(i, embedding);
        }

        let query = vec![0.1; 384];
        b.iter(|| {
            let _ = index.search_knn(black_box(&query), 10);
        })
    });
}

criterion_group!(benches, hnsw_insertion_bench, hnsw_search_bench);
criterion_main!(benches);
