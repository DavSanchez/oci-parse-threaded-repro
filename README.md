# oci-parse-threaded-repro

Proof-of-concept demonstrating unbounded per-thread memory growth in
[`oci-spec-rs`](https://github.com/youki-dev/oci-spec-rs) when
`Reference::from_str()` is called from many short-lived threads over the
lifetime of a process.

Accompanies the PR to `youki-dev/oci-spec-rs` that replaces the regex-based
parser with a hand-written procedural one.

---

## The problem

`Reference::from_str()` in `oci-spec-rs` validates the input with a compiled
`regex::Regex`. The `regex` crate maintains a `Pool<meta::Cache>` — a pool of
NFA/DFA execution-state caches — that I assume is keyed internally by OS thread ID. The pool
grows on demand: the first time a thread touches the regex it is assigned a new
slot (~5–6 MB). When the thread exits its slot is **returned** to the pool but
**never freed or reused by a different thread ID**. A subsequent thread with a
different ID seems to allocate a new slot.

The pool has a maximum number of slots before it starts re-using entries, but
the memory already committed to earlier slots remains live for the lifetime of
the process. In practice this means every "generation" of new threads (up to
the pool's internal limit) causes a permanent RSS increase of ~5–6 MB, with no
path to reclamation.

### Why this matters in practice

Any long-running Rust application that spawns short-lived worker threads and
parses OCI image references (e.g. an agent runtime, a container scheduler, a
CI runner) will leak several megabytes of RSS per "generation" of threads.  The
growth stops once the regex pool is fully populated, but the committed pages are
never returned to the OS.

We have observed this also increases with the length of the set of OCI references parsed (in this reproducer we use 2 OCI refs, but using 5, 8, etc, increased the RSS more).

---

## Repository layout

```
src/main.rs            # PoC harness
Cargo.toml             # depends on both oci-spec 0.9.0 and the patched fork
darwin-reports/        # captures from macOS (Apple Silicon)
  output-report.txt      # original crate, 80 sequential threads
  output-report-fork.txt # patched fork,   80 sequential threads
  dhat-heap.json         # dhat heap profile, original crate
  dhat-heap-fork.json    # dhat heap profile, patched fork
linux-reports/         # captures from Linux (x86-64)
  output-report.txt
  output-report-fork.txt
  dhat-heap.json
  dhat-heap-fork.json
```

---

## How the PoC works

`src/main.rs` spawns **80 sequential threads** (one at a time, fully joined
before the next is started). Each thread:

1. Calls `Reference::from_str()` on a real Docker Hub reference string.
2. Sleeps 300 ms to simulate downstream work.
3. Exits.

After each thread exits, the main thread samples the process RSS with `ps`.
With `--use-fork` the patched version of the crate is used instead.

The binary links both versions of the crate simultaneously so both can be
compared in a single build:

```toml
[dependencies]
oci-spec      = "0.9.0"
oci-spec-fork = { package = "oci-spec", git = "https://github.com/DavSanchez/oci-spec-rs", branch = "refactor/no-regexp-reference-from-str" }
```

---

## Running it

```sh
# Original crate (shows memory growth)
cargo run --release

# Patched fork (flat memory)
cargo run --release -- --use-fork

# Optional: heap profile with dhat
cargo run --release --features dhat-heap
cargo run --release --features dhat-heap -- --use-fork
# Then open the resulting dhat-heap.json in https://nnethercote.github.io/dh_view/dh_view.html
```

---

## Results

### macOS (Apple Silicon)

| Cycle | RSS — original (KB) | RSS — fork (KB) |
|------:|--------------------:|----------------:|
| 0     | 29,440              | 2,208           |
| 2     | 34,640              | 2,352           |
| 4     | 39,808              | 2,352           |
| 6     | 45,152              | 1,920           |
| 8     | 50,816              | 1,936           |
| 79    | 50,912              | 2,128           |

The original crate reaches ~50.9 MB after 9 thread generations, a **+21.5 MB**
increase over its starting RSS of ~29.4 MB (itself inflated by the initial
regex compilation). The fork stays flat at ~2 MB throughout.

### Linux (aarch64)

| Cycle | RSS — original (KB) | RSS — fork (KB) |
|------:|--------------------:|----------------:|
| 0     | 15,092              | 512             |
| 2     | 20,980              | 512             |
| 4     | 26,868              | 512             |
| 6     | 32,884              | 512             |
| 8     | 38,900              | 512             |
| 79    | 38,900              | 512             |

The original crate reaches ~38.9 MB, a **+23.8 MB** increase from the baseline
of ~15.1 MB. The fork remains flat at 512 KB for all 80 cycles.

Full per-cycle output is in `darwin-reports/` and `linux-reports/`.

---

## Root cause summary

| | Original | Patched fork |
|---|---|---|
| Parser | `regex::Regex` with `OnceLock` | Hand-written procedural parser |
| Thread-local state | `Pool<meta::Cache>` (one slot per thread ID, never freed) | None |
| RSS after 80 threads (Linux) | ~38.9 MB | ~512 KB |
| RSS after 80 threads (macOS) | ~50.9 MB | ~2.1 MB |

---

## Links

- Patched fork: [`DavSanchez/oci-spec-rs` — `refactor/no-regexp-reference-from-str`](https://github.com/DavSanchez/oci-spec-rs/tree/refactor/no-regexp-reference-from-str)
- Upstream repo: [`youki-dev/oci-spec-rs`](https://github.com/youki-dev/oci-spec-rs)
