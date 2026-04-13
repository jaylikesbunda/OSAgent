use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use memory_stats::memory_stats;
use osagent::storage::{Message as StorageMessage, SqliteStorage};
use osagent::workflow::db::WorkflowDb;
use osagent::workflow::types::{NodeLog, Workflow, WorkflowRun, WorkflowVersion};
use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use uuid::Uuid;

fn get_memory_bytes() -> Option<usize> {
    memory_stats().map(|m| m.physical_mem)
}

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

    let small_file = temp_dir.path().join("small.txt");
    let medium_file = temp_dir.path().join("medium.txt");
    let large_file = temp_dir.path().join("large.txt");

    fs::write(&small_file, "x".repeat(1024)).unwrap();
    fs::write(&medium_file, "x".repeat(1024 * 1024)).unwrap();
    fs::write(&large_file, "x".repeat(10 * 1024 * 1024)).unwrap();

    let mut group = c.benchmark_group("file_read");

    group.throughput(Throughput::Bytes(1024));
    group.bench_function("1kb", |b| {
        let small_file = small_file.clone();
        b.iter_custom(|_| {
            let before = get_memory_bytes().unwrap_or(0);
            let start = Instant::now();
            rt.block_on(async { tokio::fs::read_to_string(&small_file).await.unwrap() });
            let elapsed = start.elapsed();
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            elapsed
        })
    });

    group.throughput(Throughput::Bytes(1024 * 1024));
    group.bench_function("1mb", |b| {
        let medium_file = medium_file.clone();
        b.iter_custom(|_| {
            let before = get_memory_bytes().unwrap_or(0);
            let start = Instant::now();
            rt.block_on(async { tokio::fs::read_to_string(&medium_file).await.unwrap() });
            let elapsed = start.elapsed();
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            elapsed
        })
    });

    group.throughput(Throughput::Bytes(10 * 1024 * 1024));
    group.bench_function("10mb", |b| {
        let large_file = large_file.clone();
        b.iter_custom(|_| {
            let before = get_memory_bytes().unwrap_or(0);
            let start = Instant::now();
            rt.block_on(async { tokio::fs::read_to_string(&large_file).await.unwrap() });
            let elapsed = start.elapsed();
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            elapsed
        })
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
        b.iter(|| {
            let before = get_memory_bytes().unwrap_or(0);
            let result = serde_json::from_str::<serde_json::Value>(black_box(&raw));
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            result
        })
    });

    group.bench_function("parse_medium", |b| {
        let raw = serde_json::to_string(&medium_json).unwrap();
        b.iter(|| {
            let before = get_memory_bytes().unwrap_or(0);
            let result = serde_json::from_str::<serde_json::Value>(black_box(&raw));
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            result
        })
    });

    group.bench_function("serialize_medium", |b| {
        b.iter(|| {
            let before = get_memory_bytes().unwrap_or(0);
            let result = serde_json::to_string(black_box(&medium_json));
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            result
        })
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
            let before = get_memory_bytes().unwrap_or(0);
            let mut map = HashMap::new();
            for i in 0..1000 {
                map.insert(format!("key_{}", i), i);
            }
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            map
        })
    });

    let prepopulated: HashMap<String, usize> =
        (0..10_000).map(|i| (format!("key_{}", i), i)).collect();

    group.bench_function("lookup_1000", |b| {
        b.iter(|| {
            let before = get_memory_bytes().unwrap_or(0);
            let mut sum = 0;
            for i in 0..1000 {
                sum += prepopulated.get(&format!("key_{}", i)).unwrap();
            }
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            sum
        })
    });

    group.finish();
}

fn bench_string_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("string");

    let strings: Vec<String> = (0..1000).map(|i| format!("string_number_{}", i)).collect();

    group.bench_function("concat_1000", |b| {
        b.iter(|| {
            let before = get_memory_bytes().unwrap_or(0);
            let result = strings.join("\n");
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            result
        })
    });

    group.bench_function("clone_1000", |b| {
        b.iter(|| {
            let before = get_memory_bytes().unwrap_or(0);
            let result = strings.to_vec();
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            result
        })
    });

    group.finish();
}

fn bench_sqlite_operations(c: &mut Criterion) {
    use rusqlite::Connection;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("insert_bench.db");
    let select_db_path = temp_dir.path().join("select_bench.db");

    let mut group = c.benchmark_group("sqlite");

    group.bench_function("insert_1000", |b| {
        b.iter(|| {
            let before = get_memory_bytes().unwrap_or(0);
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
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
        })
    });

    let mut conn = Connection::open(&select_db_path).unwrap();
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
            let before = get_memory_bytes().unwrap_or(0);
            let mut stmt = conn.prepare("SELECT data FROM test LIMIT 1000").unwrap();
            let rows: Vec<String> = stmt
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            let after = get_memory_bytes().unwrap_or(0);
            println!("  Memory delta: {} bytes", after.saturating_sub(before));
            rows.len()
        })
    });

    group.finish();
}

fn sample_storage_messages(count: usize) -> Vec<StorageMessage> {
    (0..count)
        .map(|i| StorageMessage::user(format!("message {}", i)))
        .collect()
}

fn sample_workflow() -> Workflow {
    Workflow {
        id: Uuid::new_v4().to_string(),
        name: "bench workflow".to_string(),
        description: Some("workflow benchmark".to_string()),
        default_workspace_id: None,
        current_version: 1,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn sample_workflow_version(workflow_id: &str) -> WorkflowVersion {
    WorkflowVersion {
        id: Uuid::new_v4().to_string(),
        workflow_id: workflow_id.to_string(),
        version: 1,
        graph_json: serde_json::json!({
            "nodes": [
                {
                    "id": "trigger-1",
                    "node_type": "trigger",
                    "position": {"x": 0.0, "y": 0.0},
                    "config": {}
                },
                {
                    "id": "output-1",
                    "node_type": "output",
                    "position": {"x": 200.0, "y": 0.0},
                    "config": {"format": "text", "template": "done"}
                }
            ],
            "edges": [
                {
                    "id": "edge-1",
                    "source_node_id": "trigger-1",
                    "source_port": "out",
                    "target_node_id": "output-1",
                    "target_port": "in"
                }
            ]
        })
        .to_string(),
        created_at: String::new(),
    }
}

fn sample_workflow_run(workflow_id: &str) -> WorkflowRun {
    WorkflowRun {
        id: Uuid::new_v4().to_string(),
        workflow_id: workflow_id.to_string(),
        workflow_version: 1,
        status: "running".to_string(),
        started_at: String::new(),
        completed_at: None,
        error_message: None,
    }
}

fn sample_node_log(run_id: &str, idx: usize) -> NodeLog {
    NodeLog {
        id: Uuid::new_v4().to_string(),
        run_id: run_id.to_string(),
        node_id: format!("node-{}", idx),
        node_type: if idx % 2 == 0 { "agent" } else { "transform" }.to_string(),
        status: "completed".to_string(),
        input_json: Some(serde_json::json!({"index": idx, "input": "bench"}).to_string()),
        output_json: Some(serde_json::json!({"index": idx, "output": "ok"}).to_string()),
        started_at: String::new(),
        completed_at: None,
    }
}

fn bench_app_storage_operations(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let mut group = c.benchmark_group("app_storage");

    group.bench_function("open_and_migrate", |b| {
        b.iter(|| {
            let db_path = temp_dir
                .path()
                .join(format!("storage_init_{}.db", Uuid::new_v4()));
            let storage = SqliteStorage::new(&db_path.to_string_lossy()).unwrap();
            drop(storage);
            fs::remove_file(db_path).ok();
        })
    });

    let create_path = temp_dir.path().join("storage_create.db");
    let create_storage = SqliteStorage::new(&create_path.to_string_lossy()).unwrap();
    group.bench_function("create_session", |b| {
        b.iter(|| {
            create_storage
                .create_session(
                    "gpt-4o-mini".to_string(),
                    "openai".to_string(),
                    Some("bench session".to_string()),
                )
                .unwrap()
        })
    });

    let read_path = temp_dir.path().join("storage_read.db");
    let read_storage = SqliteStorage::new(&read_path.to_string_lossy()).unwrap();
    let mut read_session = read_storage
        .create_session(
            "gpt-4o-mini".to_string(),
            "openai".to_string(),
            Some("bench read".to_string()),
        )
        .unwrap();
    read_session.messages = sample_storage_messages(100);
    read_session.context_state = Some(osagent::storage::SessionContextState {
        estimated_tokens: 8_192,
        context_window: 128_000,
        budget_tokens: 32_000,
        actual_usage: None,
        tool_usage: vec![],
        compaction_stats: Default::default(),
    });
    read_storage.update_session(&read_session).unwrap();
    let read_session_id = read_session.id.clone();
    group.bench_function("get_session_100_messages", |b| {
        b.iter(|| {
            read_storage
                .get_session(black_box(&read_session_id))
                .unwrap()
        })
    });

    let update_path = temp_dir.path().join("storage_update.db");
    let update_storage = SqliteStorage::new(&update_path.to_string_lossy()).unwrap();
    let update_session = update_storage
        .create_session(
            "gpt-4o-mini".to_string(),
            "openai".to_string(),
            Some("bench update".to_string()),
        )
        .unwrap();
    let mut update_payload = update_session.clone();
    update_payload.messages = sample_storage_messages(100);
    update_payload.metadata = serde_json::json!({"name": "bench update", "iteration": 1});
    update_payload.context_state = Some(osagent::storage::SessionContextState {
        estimated_tokens: 8_192,
        context_window: 128_000,
        budget_tokens: 32_000,
        actual_usage: None,
        tool_usage: vec![],
        compaction_stats: Default::default(),
    });
    group.bench_function("update_session_100_messages", |b| {
        b.iter(|| {
            update_storage
                .update_session(black_box(&update_payload))
                .unwrap()
        })
    });

    let list_path = temp_dir.path().join("storage_list.db");
    let list_storage = SqliteStorage::new(&list_path.to_string_lossy()).unwrap();
    for i in 0..1000 {
        list_storage
            .create_session(
                "gpt-4o-mini".to_string(),
                "openai".to_string(),
                Some(format!("session {}", i)),
            )
            .unwrap();
    }
    group.bench_function("list_sessions_1000", |b| {
        b.iter(|| list_storage.list_sessions().unwrap())
    });

    group.finish();
}

fn bench_workflow_db_operations(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let mut group = c.benchmark_group("workflow_db");

    group.bench_function("init_tables", |b| {
        b.iter(|| {
            let db_path = temp_dir
                .path()
                .join(format!("workflow_init_{}.db", Uuid::new_v4()));
            let workflow_db = WorkflowDb::new(db_path.clone());
            workflow_db.init_tables().unwrap();
            fs::remove_file(db_path).ok();
        })
    });

    let create_path = temp_dir.path().join("workflow_create.db");
    let create_db = WorkflowDb::new(create_path.clone());
    create_db.init_tables().unwrap();
    group.bench_function("create_workflow", |b| {
        b.iter(|| {
            create_db
                .create_workflow(black_box(&sample_workflow()))
                .unwrap()
        })
    });

    let version_path = temp_dir.path().join("workflow_version.db");
    let version_db = WorkflowDb::new(version_path.clone());
    version_db.init_tables().unwrap();
    let workflow = sample_workflow();
    version_db.create_workflow(&workflow).unwrap();
    let version = sample_workflow_version(&workflow.id);
    version_db.create_version(&version).unwrap();
    group.bench_function("get_version", |b| {
        b.iter(|| {
            version_db
                .get_version(black_box(&workflow.id), black_box(1))
                .unwrap()
        })
    });

    let list_path = temp_dir.path().join("workflow_list.db");
    let list_db = WorkflowDb::new(list_path.clone());
    list_db.init_tables().unwrap();
    for _ in 0..1000 {
        let workflow = sample_workflow();
        list_db.create_workflow(&workflow).unwrap();
    }
    group.bench_function("list_workflows_1000", |b| {
        b.iter(|| list_db.list_workflows().unwrap())
    });

    let logs_path = temp_dir.path().join("workflow_logs.db");
    let logs_db = WorkflowDb::new(logs_path.clone());
    logs_db.init_tables().unwrap();
    let logs_workflow = sample_workflow();
    logs_db.create_workflow(&logs_workflow).unwrap();
    logs_db
        .create_version(&sample_workflow_version(&logs_workflow.id))
        .unwrap();
    let run = sample_workflow_run(&logs_workflow.id);
    logs_db.create_run(&run).unwrap();
    for i in 0..1000 {
        logs_db
            .create_node_log(&sample_node_log(&run.id, i))
            .unwrap();
    }
    group.bench_function("get_node_logs_1000", |b| {
        b.iter(|| logs_db.get_node_logs(black_box(&run.id)).unwrap())
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
        bench_sqlite_operations,
        bench_app_storage_operations,
        bench_workflow_db_operations
}

criterion_main!(benches);
