use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn bench_startup_time(c: &mut Criterion) {
    let binary = "./target/release/osagent";

    c.bench_function("startup_cold", |b| {
        b.iter(|| {
            let start = Instant::now();
            let _output = Command::new(binary)
                .arg("--version")
                .output()
                .expect("Failed to start osagent");
            start.elapsed()
        })
    });

    c.bench_function("startup_help", |b| {
        b.iter(|| {
            Command::new(binary)
                .arg("--help")
                .output()
                .expect("Failed to start osagent")
        })
    });
}

fn bench_file_operations(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let temp_dir = TempDir::new().unwrap();

    // Create test files
    let small_file = temp_dir.path().join("small.txt");
    let medium_file = temp_dir.path().join("medium.txt");
    let large_file = temp_dir.path().join("large.txt");

    fs::write(&small_file, "x".repeat(1024)).unwrap(); // 1KB
    fs::write(&medium_file, "x".repeat(1024 * 1024)).unwrap(); // 1MB
    fs::write(&large_file, "x".repeat(10 * 1024 * 1024)).unwrap(); // 10MB

    let mut group = c.benchmark_group("file_read");

    group.throughput(Throughput::Bytes(1024));
    group.bench_function("1kb", |b| {
        b.to_async(&rt)
            .iter(|| async { tokio::fs::read_to_string(&small_file).await.unwrap() })
    });

    group.throughput(Throughput::Bytes(1024 * 1024));
    group.bench_function("1mb", |b| {
        b.to_async(&rt)
            .iter(|| async { tokio::fs::read_to_string(&medium_file).await.unwrap() })
    });

    group.throughput(Throughput::Bytes(10 * 1024 * 1024));
    group.bench_function("10mb", |b| {
        b.to_async(&rt)
            .iter(|| async { tokio::fs::read_to_string(&large_file).await.unwrap() })
    });

    group.finish();
}

fn bench_json_parsing(c: &mut Criterion) {
    let small_json = serde_json::json!({"message": "hello"});
    let medium_json = serde_json::json!({
        "messages": (0..100).map(|i| {
            serde_json::json!({"role": "user", "content": format!("Message {}", i)})
        }).collect::<Vec<_>>()
    });

    let mut group = c.benchmark_group("json");

    group.bench_function("parse_small", |b| {
        let raw = serde_json::to_string(&small_json).unwrap();
        b.iter(|| serde_json::from_str::<serde_json::Value>(black_box(&raw)))
    });

    group.bench_function("parse_medium", |b| {
        let raw = serde_json::to_string(&medium_json).unwrap();
        b.iter(|| serde_json::from_str::<serde_json::Value>(black_box(&raw)))
    });

    group.bench_function("serialize_medium", |b| {
        b.iter(|| serde_json::to_string(black_box(&medium_json)))
    });

    group.finish();
}

fn bench_regex_search(c: &mut Criterion) {
    use regex::Regex;

    let haystack = "x".repeat(100_000) + "target" + &"y".repeat(100_000);
    let pattern = Regex::new(r"target").unwrap();

    c.bench_function("regex_find", |b| {
        b.iter(|| pattern.find(black_box(&haystack)))
    });

    let multi_pattern = Regex::new(r"(fn|struct|impl|pub|async)\s+").unwrap();
    let code_haystack = include_str!("../src/main.rs").repeat(10);

    c.bench_function("regex_find_code", |b| {
        b.iter(|| multi_pattern.find_iter(black_box(&code_haystack)).count())
    });
}

fn bench_hashmap_operations(c: &mut Criterion) {
    use std::collections::HashMap;

    let mut group = c.benchmark_group("hashmap");

    group.bench_function("insert_1000", |b| {
        b.iter(|| {
            let mut map = HashMap::new();
            for i in 0..1000 {
                map.insert(format!("key_{}", i), i);
            }
            map
        })
    });

    let prepopulated: HashMap<String, usize> =
        (0..10_000).map(|i| (format!("key_{}", i), i)).collect();

    group.bench_function("lookup_1000", |b| {
        b.iter(|| {
            let mut sum = 0;
            for i in 0..1000 {
                sum += prepopulated.get(&format!("key_{}", i)).unwrap();
            }
            sum
        })
    });

    group.finish();
}

fn bench_string_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("string");

    let strings: Vec<String> = (0..1000).map(|i| format!("string_number_{}", i)).collect();

    group.bench_function("concat_1000", |b| b.iter(|| strings.join("\n")));

    group.bench_function("clone_1000", |b| b.iter(|| strings.to_vec()));

    group.finish();
}

fn bench_sqlite_operations(c: &mut Criterion) {
    use rusqlite::Connection;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("bench.db");

    let mut group = c.benchmark_group("sqlite");

    group.bench_function("insert_1000", |b| {
        b.iter(|| {
            let mut conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE IF NOT EXISTS test (id INTEGER PRIMARY KEY, data TEXT)",
                [],
            )
            .unwrap();
            let tx = conn.transaction().unwrap();
            {
                let mut stmt = tx
                    .prepare_cached("INSERT INTO test (data) VALUES (?)")
                    .unwrap();
                for i in 0..1000 {
                    stmt.execute([format!("data_{}", i)]).unwrap();
                }
            }
            tx.commit().unwrap();
            fs::remove_file(&db_path).ok();
        })
    });

    // Pre-populate for read test
    let mut conn = Connection::open(&db_path).unwrap();
    conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, data TEXT)", [])
        .unwrap();
    let tx = conn.transaction().unwrap();
    {
        let mut stmt = tx
            .prepare_cached("INSERT INTO test (data) VALUES (?)")
            .unwrap();
        for i in 0..10_000 {
            stmt.execute([format!("data_{}", i)]).unwrap();
        }
    }
    tx.commit().unwrap();

    group.bench_function("select_1000", |b| {
        b.iter(|| {
            let mut stmt = conn.prepare("SELECT data FROM test LIMIT 1000").unwrap();
            let rows: Vec<String> = stmt
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            rows.len()
        })
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(5))
        .sample_size(100);
    targets =
        bench_startup_time,
        bench_file_operations,
        bench_json_parsing,
        bench_regex_search,
        bench_hashmap_operations,
        bench_string_operations,
        bench_sqlite_operations
}

criterion_main!(benches);
