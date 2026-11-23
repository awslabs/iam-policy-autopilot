use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use iam_policy_autopilot_policy_generation::{ExtractionEngine, Language, SourceFile};

const FILE_COUNT: usize = 256;

const JS_FILES: &[&str] = &["demo_javascript.js"];
const TS_FILES: &[&str] = &[
    "demo_typescript.ts",
    "test_command_with_params.ts",
    "test_js_libraries.ts",
    "test_waiters.ts",
];

fn resource(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../iam-policy-autopilot-cli/tests/resources")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn corpus(language: Language, names: &[&str], extension: &str) -> Vec<SourceFile> {
    let contents: Vec<String> = names.iter().map(|name| resource(name)).collect();
    (0..FILE_COUNT)
        .map(|i| {
            SourceFile::with_language(
                PathBuf::from(format!("bench/file_{i}.{extension}")),
                contents[i % contents.len()].clone(),
                language,
            )
        })
        .collect()
}

fn bench_language(
    c: &mut Criterion,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    language: Language,
    files: Vec<SourceFile>,
) {
    let engine = ExtractionEngine::new();

    let warm = runtime
        .block_on(engine.extract_sdk_method_calls(language, files.clone()))
        .expect("warm-up extraction");
    assert!(!warm.methods.is_empty(), "{name}: no methods");

    c.bench_function(name, |b| {
        b.iter_batched(
            || files.clone(),
            |files| {
                let extracted = runtime
                    .block_on(engine.extract_sdk_method_calls(language, files))
                    .expect("extraction");
                std::hint::black_box(extracted)
            },
            BatchSize::LargeInput,
        );
    });
}

fn js_ts_extraction(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    bench_language(
        c,
        &runtime,
        "javascript_256_files",
        Language::JavaScript,
        corpus(Language::JavaScript, JS_FILES, "js"),
    );
    bench_language(
        c,
        &runtime,
        "typescript_256_files",
        Language::TypeScript,
        corpus(Language::TypeScript, TS_FILES, "ts"),
    );
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = js_ts_extraction
}
criterion_main!(benches);
