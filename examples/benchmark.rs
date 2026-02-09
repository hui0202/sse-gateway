//! Performance Benchmark for SSE Gateway
//!
//! Prerequisites:
//!   1. Start Redis: docker run -d -p 6379:6379 redis
//!   2. Start Gateway: cargo run
//!
//! Run benchmark:
//!   cargo run --example benchmark --release
//!
//! Options (via env vars):
//!   GATEWAY_URL=http://localhost:8080
//!   PUSH_URL=http://localhost:9000
//!   NUM_CONNECTIONS=100
//!   NUM_MESSAGES=1000
//!   CONCURRENCY=10

use futures::StreamExt;
use reqwest::Client;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;

#[derive(Clone)]
struct BenchConfig {
    gateway_url: String,
    push_url: String,
    num_connections: usize,
    num_messages: usize,
    concurrency: usize,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            gateway_url: std::env::var("GATEWAY_URL")
                .unwrap_or_else(|_| "http://localhost:8080".to_string()),
            push_url: std::env::var("PUSH_URL")
                .unwrap_or_else(|_| "http://localhost:9000".to_string()),
            num_connections: std::env::var("NUM_CONNECTIONS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
            num_messages: std::env::var("NUM_MESSAGES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1000),
            concurrency: std::env::var("CONCURRENCY")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
        }
    }
}

struct BenchResults {
    name: String,
    total_time: Duration,
    success_count: u64,
    error_count: u64,
    min_latency: Duration,
    max_latency: Duration,
    avg_latency: Duration,
    p50_latency: Duration,
    p95_latency: Duration,
    p99_latency: Duration,
}

impl BenchResults {
    fn print(&self) {
        let throughput = self.success_count as f64 / self.total_time.as_secs_f64();
        println!("\n=== {} ===", self.name);
        println!("Total time:     {:?}", self.total_time);
        println!("Success:        {}", self.success_count);
        println!("Errors:         {}", self.error_count);
        println!("Throughput:     {:.2} req/s", throughput);
        println!("Latency:");
        println!("  Min:          {:?}", self.min_latency);
        println!("  Avg:          {:?}", self.avg_latency);
        println!("  P50:          {:?}", self.p50_latency);
        println!("  P95:          {:?}", self.p95_latency);
        println!("  P99:          {:?}", self.p99_latency);
        println!("  Max:          {:?}", self.max_latency);
    }
}

fn calculate_percentile(sorted_latencies: &[Duration], percentile: f64) -> Duration {
    if sorted_latencies.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted_latencies.len() as f64 * percentile / 100.0) as usize)
        .min(sorted_latencies.len() - 1);
    sorted_latencies[idx]
}

fn compute_stats(name: &str, total_time: Duration, latencies: Vec<Duration>, errors: u64) -> BenchResults {
    let mut sorted = latencies.clone();
    sorted.sort();

    let success_count = sorted.len() as u64;
    let avg = if success_count > 0 {
        Duration::from_nanos(
            sorted.iter().map(|d| d.as_nanos() as u64).sum::<u64>() / success_count,
        )
    } else {
        Duration::ZERO
    };

    BenchResults {
        name: name.to_string(),
        total_time,
        success_count,
        error_count: errors,
        min_latency: sorted.first().copied().unwrap_or(Duration::ZERO),
        max_latency: sorted.last().copied().unwrap_or(Duration::ZERO),
        avg_latency: avg,
        p50_latency: calculate_percentile(&sorted, 50.0),
        p95_latency: calculate_percentile(&sorted, 95.0),
        p99_latency: calculate_percentile(&sorted, 99.0),
    }
}

/// Benchmark 1: SSE Connection Establishment
async fn bench_sse_connections(config: &BenchConfig) -> BenchResults {
    println!("\n[1/3] Benchmarking SSE connection establishment...");
    println!("      Connections: {}, Concurrency: {}", config.num_connections, config.concurrency);

    let client = Client::new();
    let latencies = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let errors = Arc::new(AtomicU64::new(0));
    let semaphore = Arc::new(tokio::sync::Semaphore::new(config.concurrency));

    let start = Instant::now();
    let mut handles = vec![];

    for i in 0..config.num_connections {
        let client = client.clone();
        let url = format!(
            "{}/sse/connect?channel_id=bench_{}",
            config.gateway_url, i
        );
        let latencies = latencies.clone();
        let errors = errors.clone();
        let semaphore = semaphore.clone();

        handles.push(tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            let req_start = Instant::now();

            match client
                .get(&url)
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    let latency = req_start.elapsed();
                    latencies.lock().await.push(latency);
                    // Immediately close the connection
                    drop(resp);
                }
                _ => {
                    errors.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let total_time = start.elapsed();
    let latencies = Arc::try_unwrap(latencies).unwrap().into_inner();
    let errors = errors.load(Ordering::SeqCst);

    compute_stats("SSE Connection Establishment", total_time, latencies, errors)
}

/// Benchmark 2: Direct Push Throughput
async fn bench_direct_push(config: &BenchConfig) -> BenchResults {
    println!("\n[2/3] Benchmarking direct push throughput...");
    println!("      Messages: {}, Concurrency: {}", config.num_messages, config.concurrency);

    let client = Client::new();
    let latencies = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let errors = Arc::new(AtomicU64::new(0));
    let semaphore = Arc::new(tokio::sync::Semaphore::new(config.concurrency));

    let start = Instant::now();
    let mut handles = vec![];

    for i in 0..config.num_messages {
        let client = client.clone();
        let url = format!("{}/push", config.push_url);
        let latencies = latencies.clone();
        let errors = errors.clone();
        let semaphore = semaphore.clone();

        handles.push(tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            let req_start = Instant::now();

            let payload = serde_json::json!({
                "channel_id": format!("bench_{}", i % 100),
                "event_type": "benchmark",
                "data": {
                    "seq": i,
                    "timestamp": chrono::Utc::now().timestamp_millis()
                }
            });

            match client
                .post(&url)
                .json(&payload)
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    let latency = req_start.elapsed();
                    latencies.lock().await.push(latency);
                }
                _ => {
                    errors.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let total_time = start.elapsed();
    let latencies = Arc::try_unwrap(latencies).unwrap().into_inner();
    let errors = errors.load(Ordering::SeqCst);

    compute_stats("Direct Push Throughput", total_time, latencies, errors)
}

/// Benchmark 3: End-to-End Latency (Push -> SSE)
async fn bench_e2e_latency(config: &BenchConfig) -> BenchResults {
    println!("\n[3/3] Benchmarking end-to-end latency (push -> SSE)...");
    println!("      Messages: {}", config.num_messages.min(100));

    let client = Client::new();
    let channel_id = format!("bench_e2e_{}", std::process::id());
    let num_messages = config.num_messages.min(100); // Limit for E2E test

    // Start SSE connection
    let sse_url = format!(
        "{}/sse/connect?channel_id={}",
        config.gateway_url, channel_id
    );

    let sse_client = client.clone();
    let sse_response = match sse_client.get(&sse_url).send().await {
        Ok(resp) => resp,
        Err(e) => {
            println!("      Failed to connect SSE: {}", e);
            return BenchResults {
                name: "End-to-End Latency".to_string(),
                total_time: Duration::ZERO,
                success_count: 0,
                error_count: 1,
                min_latency: Duration::ZERO,
                max_latency: Duration::ZERO,
                avg_latency: Duration::ZERO,
                p50_latency: Duration::ZERO,
                p95_latency: Duration::ZERO,
                p99_latency: Duration::ZERO,
            };
        }
    };

    // Wait for connection to be established
    tokio::time::sleep(Duration::from_millis(100)).await;

    let latencies = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let errors = Arc::new(AtomicU64::new(0));
    let received = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(2));

    // SSE receiver task
    let latencies_clone = latencies.clone();
    let received_clone = received.clone();
    let errors_clone = errors.clone();
    let barrier_clone = barrier.clone();
    let num_messages_clone = num_messages;

    let receiver_handle = tokio::spawn(async move {
        let mut stream = sse_response.bytes_stream();
        let mut buffer = String::new();

        barrier_clone.wait().await;

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    // Parse SSE events
                    while let Some(pos) = buffer.find("\n\n") {
                        let event_str = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();

                        // Extract timestamp from data
                        if let Some(data_line) = event_str
                            .lines()
                            .find(|l| l.starts_with("data:"))
                        {
                            let data = data_line.trim_start_matches("data:");
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                if let Some(ts) = json.get("send_ts").and_then(|v| v.as_i64()) {
                                    let now = chrono::Utc::now().timestamp_millis();
                                    let latency = Duration::from_millis((now - ts).max(0) as u64);
                                    latencies_clone.lock().await.push(latency);
                                    received_clone.fetch_add(1, Ordering::SeqCst);
                                }
                            }
                        }
                    }

                    if received_clone.load(Ordering::SeqCst) >= num_messages_clone as u64 {
                        break;
                    }
                }
                Err(_) => {
                    errors_clone.fetch_add(1, Ordering::SeqCst);
                    break;
                }
            }
        }
    });

    // Sender task
    let push_url = format!("{}/push", config.push_url);
    let start = Instant::now();

    barrier.wait().await;

    for i in 0..num_messages {
        let send_ts = chrono::Utc::now().timestamp_millis();
        let payload = serde_json::json!({
            "channel_id": channel_id,
            "event_type": "e2e_test",
            "data": {
                "seq": i,
                "send_ts": send_ts
            }
        });

        if let Err(_) = client.post(&push_url).json(&payload).send().await {
            errors.fetch_add(1, Ordering::SeqCst);
        }

        // Small delay to avoid overwhelming
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Wait for receiver with timeout
    let _ = tokio::time::timeout(Duration::from_secs(10), receiver_handle).await;

    let total_time = start.elapsed();
    let latencies = Arc::try_unwrap(latencies).unwrap().into_inner();
    let errors = errors.load(Ordering::SeqCst);

    compute_stats("End-to-End Latency", total_time, latencies, errors)
}

/// Quick health check
async fn health_check(config: &BenchConfig) -> bool {
    let client = Client::new();

    // Check gateway
    let gateway_health = client
        .get(format!("{}/health", config.gateway_url))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    // Check push endpoint
    let push_health = client
        .post(format!("{}/push", config.push_url))
        .json(&serde_json::json!({
            "channel_id": "health_check",
            "event_type": "ping",
            "data": {}
        }))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    gateway_health && push_health
}

#[tokio::main]
async fn main() {
    let config = BenchConfig::default();

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          SSE Gateway Performance Benchmark                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("Configuration:");
    println!("  Gateway URL:    {}", config.gateway_url);
    println!("  Push URL:       {}", config.push_url);
    println!("  Connections:    {}", config.num_connections);
    println!("  Messages:       {}", config.num_messages);
    println!("  Concurrency:    {}", config.concurrency);

    // Health check
    println!("\nChecking service health...");
    if !health_check(&config).await {
        println!("ERROR: Gateway is not responding. Please ensure:");
        println!("  1. Redis is running: docker run -d -p 6379:6379 redis");
        println!("  2. Gateway is running: cargo run");
        return;
    }
    println!("Service is healthy!");

    // Run benchmarks
    let results = vec![
        bench_sse_connections(&config).await,
        bench_direct_push(&config).await,
        bench_e2e_latency(&config).await,
    ];

    // Print summary
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                      BENCHMARK SUMMARY                       ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    for result in results {
        result.print();
    }
}
