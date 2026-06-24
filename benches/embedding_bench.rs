use criterion::{black_box, criterion_group, criterion_main, Criterion};
use needle::embedding::EmbeddingModel;

fn embedding_bench(c: &mut Criterion) {
    c.bench_function("embed_short_text", |b| {
        let model = EmbeddingModel::new(384).expect("Failed to create model");
        let text = black_box("retry backoff http requests");
        b.iter(|| {
            let _ = model.embed(text);
        })
    });
}

criterion_group!(benches, embedding_bench);
criterion_main!(benches);
