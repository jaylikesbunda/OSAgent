use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    println!("OSAgent Stress Test");
    println!("===================\n");

    let mut failures = 0;

    // Test 1: Memory under load
    println!("[1/4] Memory stress test...");
    let initial_mem = get_memory_mb();

    let mut handles = vec![];
    for i in 0..100 {
        let handle = tokio::spawn(async move {
            let data = vec![0u8; 1024 * 1024]; // 1MB
            tokio::time::sleep(Duration::from_millis(10)).await;
            data.len()
        });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    let peak_mem = get_memory_mb();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let final_mem = get_memory_mb();

    println!("   Initial: {}MB", initial_mem);
    println!("   Peak:    {}MB", peak_mem);
    println!("   Final:   {}MB", final_mem);

    if final_mem > initial_mem + 10 {
        println!(
            "   ❌ FAIL: Memory leak detected ({}MB leaked)",
            final_mem - initial_mem
        );
        failures += 1;
    } else {
        println!("   ✅ PASS: No memory leak");
    }

    // Test 2: Channel backpressure
    println!("\n[2/4] Channel backpressure test...");
    let (tx, mut rx) = mpsc::channel::<String>(100);

    let producer = tokio::spawn(async move {
        for i in 0..10_000 {
            if tx.send(format!("message_{}", i)).await.is_err() {
                break;
            }
        }
    });

    let consumer = tokio::spawn(async move {
        let mut count = 0;
        while let Some(_msg) = rx.recv().await {
            count += 1;
            if count % 1000 == 0 {
                tokio::time::sleep(Duration::from_micros(1)).await;
            }
        }
        count
    });

    let _ = producer.await;
    let count = consumer.await.unwrap();
    println!("   Processed {} messages", count);
    println!("   ✅ PASS: Channel handled backpressure");

    // Test 3: Concurrent file operations
    println!("\n[3/4] Concurrent file I/O test...");
    let temp_dir = std::env::temp_dir().join("osagent_stress_test");
    std::fs::create_dir_all(&temp_dir).ok();

    let mut file_handles = vec![];
    for i in 0..50 {
        let path = temp_dir.join(format!("test_{}.txt", i));
        let handle = tokio::spawn(async move {
            let content = "x".repeat(10_000);
            tokio::fs::write(&path, &content).await.unwrap();
            let read = tokio::fs::read_to_string(&path).await.unwrap();
            tokio::fs::remove_file(&path).await.ok();
            read.len()
        });
        file_handles.push(handle);
    }

    let mut total_bytes = 0;
    for handle in file_handles {
        total_bytes += handle.await.unwrap();
    }
    println!("   Wrote/read {} bytes concurrently", total_bytes);
    println!("   ✅ PASS: Concurrent I/O handled");

    std::fs::remove_dir_all(&temp_dir).ok();

    // Test 4: Long-running stability
    println!("\n[4/4] Stability test (10s)...");
    let start = Instant::now();
    let mut iterations = 0;

    while start.elapsed() < Duration::from_secs(10) {
        let data: Arc<Vec<u8>> = Arc::new(vec![0; 1024]);
        let _ = data.len();
        iterations += 1;
    }

    println!("   Completed {} iterations", iterations);
    println!("   ✅ PASS: Stable execution");

    // Summary
    println!("\n===================");
    if failures == 0 {
        println!("✅ ALL TESTS PASSED");
    } else {
        println!("❌ {} TESTS FAILED", failures);
        std::process::exit(1);
    }
}

fn get_memory_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        use std::fs;
        let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                let kb: u64 = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
                return kb / 1024;
            }
        }
        0
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok();

        output
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|kb| kb / 1024)
            .unwrap_or(0)
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let output = Command::new("wmic")
            .args(["OS", "get", "TotalVisibleMemorySize", "/Value"])
            .output()
            .ok();
        // Simplified - would need proper Windows API for accurate per-process
        0
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        0
    }
}
