//! PoC: demonstrates that regex Pool<meta::Cache> accumulates one permanent entry per distinct
//! OS thread that calls Reference::from_str(), even when threads are purely sequential.
//!
//! Each "cycle" mimics a `recreate_sub_agent()` event: the old "Subagent runtime" thread exits
//! and a new one is spawned. The new thread calls Reference::from_str() (via template_with /
//! assemble_agent in production), which checks out a Pool<meta::Cache> slot indexed by thread ID.
//! When the thread exits the cache is returned to the pool but NOT freed. A new thread with a
//! different ID may land in a different slot → new permanent ~5.87 MB allocation.

use std::str::FromStr;
// use oci_spec::distribution::Reference;
// use std::str::FromStr;
use std::thread;
use std::time::Duration;

use oci_spec::distribution::Reference;
use oci_spec_fork::distribution::Reference as ReferenceFork;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const CYCLES: usize = 80;
const SLEEP_MS: u64 = 300; // simulates download/process work

const OCI_REFS: [&str; 2] = [
    "docker.io/newrelic/infrastructure-agent-artifacts:1.71.1",
    "docker.io/newrelic/nrdot-agent-artifacts:1.11.0",
];

fn main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    let use_fork = std::env::args().any(|a| a == "--use-fork");
    let use_fork_str_mark = if use_fork { "FORKED VERSION " } else { "" };

    println!("{use_fork_str_mark}RUN START\n");
    println!("Spawning {CYCLES} sequential threads, each calling Reference::from_str() once.");
    println!(
        "Each thread represents one 'Subagent runtime' cycle (old thread exits, new spawned).\n"
    );
    #[cfg(target_family = "unix")]
    println!("{:>6}  {:>12}  thread_id", "cycle", "rss (KB)");
    #[cfg(target_family = "windows")]
    println!("{:>6}  {:>12}  thread_id", "cycle", "ws (KB)");

    let mut oci_refs = OCI_REFS.into_iter().cycle();

    for i in 0..CYCLES {
        let oci_ref = oci_refs.next().unwrap(); // cycle always ensures this is safe
        let handle = thread::Builder::new()
            .name(format!("subagent-runtime-{i}"))
            .spawn(move || {
                let tid = format!("{:?}", thread::current().id());
                // Mimics: assemble_agent() → Oci::template_with() → Reference::from_str()
                // This checks out a Pool<meta::Cache> slot keyed to this thread's ID.
                if use_fork {
                    let _unused_parse = ReferenceFork::from_str(oci_ref).expect("valid reference");
                } else {
                    let _unused_parse = Reference::from_str(oci_ref).expect("valid reference");
                };

                // Simulate download / supervisor startup work
                thread::sleep(Duration::from_millis(SLEEP_MS));
                tid
            })
            .expect("spawn failed");

        let tid = handle.join().expect("thread panicked");
        let rss = rss_kb();
        println!("{:>6}  {:>12}  {}", i, rss, tid);
    }

    println!("\n{use_fork_str_mark}Done.");
}

fn rss_kb() -> u64 {
    let pid = std::process::id();

    #[cfg(target_family = "unix")]
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();

    #[cfg(target_family = "windows")]
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("[int64]((Get-Process -Id {pid}).WorkingSet64 / 1024)"),
        ])
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();

    #[cfg(not(any(target_family = "unix", target_family = "windows")))]
    let out = b"0".to_vec();

    String::from_utf8_lossy(&out)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
}
