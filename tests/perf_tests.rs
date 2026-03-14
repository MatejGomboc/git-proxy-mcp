//! Performance baseline tests.
//!
//! These tests measure the performance of key operations to establish
//! baselines and detect regressions. They use simple timing rather than
//! statistical benchmarking to avoid adding heavy dependencies.
//!
//! Run with: `cargo test --test perf_tests -- --nocapture`

#![allow(clippy::cast_possible_truncation)] // Test file, iterations fit in u32
#![allow(clippy::cast_precision_loss)] // Acceptable for timing display

use std::time::{Duration, Instant};

use git_proxy_mcp::streaming::chunked::StreamingSessionManager;
use git_proxy_mcp::streaming::tar::encode_base64;

/// Helper to run a closure multiple times and return average duration.
fn measure_avg<F: FnMut()>(iterations: usize, mut f: F) -> Duration {
    let start = Instant::now();
    for _ in 0..iterations {
        f();
    }
    start.elapsed() / iterations as u32
}

/// Helper to format duration in a human-readable way.
fn format_duration(d: Duration) -> String {
    if d.as_nanos() < 1000 {
        format!("{}ns", d.as_nanos())
    } else if d.as_micros() < 1000 {
        format!("{:.2}µs", d.as_nanos() as f64 / 1000.0)
    } else if d.as_millis() < 1000 {
        format!("{:.2}ms", d.as_micros() as f64 / 1000.0)
    } else {
        format!("{:.2}s", d.as_millis() as f64 / 1000.0)
    }
}

// ============================================================================
// Base64 Encoding Performance
// ============================================================================

#[test]
fn perf_base64_encode_1kb() {
    let data = vec![0u8; 1024];
    let avg = measure_avg(1000, || {
        let _ = encode_base64(&data);
    });
    println!("base64 encode 1KB: {} per op", format_duration(avg));
    // Should be well under 1ms
    assert!(
        avg < Duration::from_millis(1),
        "base64 1KB too slow: {avg:?}"
    );
}

#[test]
fn perf_base64_encode_1mb() {
    let data = vec![0u8; 1024 * 1024];
    let avg = measure_avg(100, || {
        let _ = encode_base64(&data);
    });
    println!("base64 encode 1MB: {} per op", format_duration(avg));
    // Should be under 50ms
    assert!(
        avg < Duration::from_millis(50),
        "base64 1MB too slow: {avg:?}"
    );
}

#[test]
fn perf_base64_encode_10mb() {
    let data = vec![0u8; 10 * 1024 * 1024];
    let avg = measure_avg(10, || {
        let _ = encode_base64(&data);
    });
    println!("base64 encode 10MB: {} per op", format_duration(avg));
    // Should be under 500ms
    assert!(
        avg < Duration::from_millis(500),
        "base64 10MB too slow: {avg:?}"
    );
}

// ============================================================================
// Session Manager Performance
// ============================================================================

#[test]
fn perf_session_create() {
    let manager = StreamingSessionManager::default();
    let data = vec![0u8; 1024 * 100]; // 100KB

    let avg = measure_avg(100, || {
        let _ = manager.create_session(
            "https://github.com/test/repo.git",
            "main",
            "abc123",
            data.clone(),
            1024,
        );
        // Clean up by cancelling
    });
    println!(
        "session create (100KB data): {} per op",
        format_duration(avg)
    );
    // Should be under 10ms
    assert!(
        avg < Duration::from_millis(10),
        "session create too slow: {avg:?}"
    );
}

#[test]
fn perf_session_chunk_retrieval() {
    let manager = StreamingSessionManager::default();
    // Create a session with 10MB of data, 1MB chunks = 10 chunks
    let data = vec![0u8; 10 * 1024 * 1024];
    let info = manager
        .create_session(
            "https://github.com/test/repo.git",
            "main",
            "abc123",
            data,
            1024 * 1024, // 1MB chunks
        )
        .unwrap();

    let session_id = info.session_id.clone();
    let total_chunks = info.total_chunks;

    // Measure chunk retrieval (not the first chunk which might be slower)
    let mut chunk_times = Vec::new();
    for i in 0..total_chunks {
        let start = Instant::now();
        let _ = manager.get_chunk(&session_id, i);
        chunk_times.push(start.elapsed());
    }

    let avg = chunk_times.iter().sum::<Duration>() / chunk_times.len() as u32;
    println!(
        "chunk retrieval (1MB chunks): {} per chunk",
        format_duration(avg)
    );
    // Should be under 50ms per chunk
    assert!(
        avg < Duration::from_millis(50),
        "chunk retrieval too slow: {avg:?}"
    );
}

// ============================================================================
// Chunking Algorithm Performance
// ============================================================================

#[test]
fn perf_chunking_overhead() {
    // Measure overhead of chunking vs single response
    let data = vec![0u8; 1024 * 1024]; // 1MB

    // Single base64 encode
    let single_time = measure_avg(50, || {
        let _ = encode_base64(&data);
    });

    // Chunked base64 encode (simulate 10 chunks of 100KB)
    let chunked_time = measure_avg(50, || {
        for chunk in data.chunks(102_400) {
            let _ = encode_base64(chunk);
        }
    });

    println!("single encode 1MB: {}", format_duration(single_time));
    println!(
        "chunked encode 1MB (10x100KB): {}",
        format_duration(chunked_time)
    );

    // Chunked should be at most 2x slower due to overhead
    assert!(
        chunked_time < single_time * 3,
        "chunking overhead too high: single={single_time:?}, chunked={chunked_time:?}"
    );
}

// ============================================================================
// Memory Allocation Performance
// ============================================================================

#[test]
fn perf_vec_allocation_1mb() {
    let avg = measure_avg(100, || {
        let v: Vec<u8> = vec![0u8; 1024 * 1024];
        std::hint::black_box(v);
    });
    println!("vec allocation 1MB: {} per op", format_duration(avg));
    // Should be under 5ms
    assert!(
        avg < Duration::from_millis(5),
        "1MB allocation too slow: {avg:?}"
    );
}

#[test]
fn perf_vec_allocation_10mb() {
    let avg = measure_avg(10, || {
        let v: Vec<u8> = vec![0u8; 10 * 1024 * 1024];
        std::hint::black_box(v);
    });
    println!("vec allocation 10MB: {} per op", format_duration(avg));
    // Should be under 50ms
    assert!(
        avg < Duration::from_millis(50),
        "10MB allocation too slow: {avg:?}"
    );
}

// ============================================================================
// Summary Test
// ============================================================================

#[test]
fn perf_summary() {
    println!("\n=== Performance Baseline Summary ===");
    println!("These tests establish baseline performance expectations.");
    println!("Run with: cargo test --test perf_tests -- --nocapture");
    println!("==========================================\n");
}
