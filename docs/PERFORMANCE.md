# Measurements and verification

Measured on November 1, 2026 (US/Central): Intel Core Ultra 9 285H, 16 logical CPUs,
about 31 GiB RAM, x86_64 Linux `7.2.8-5-omarchy-bore`, release optimization,
Rust/GTK4. Workloads are synthetic; models and inference were not used. This desktop
also had other work running, including compilation during part of the provider run.
These numbers are observations, not guaranteed budgets or broad relevance claims.

## Matcher baseline and final matcher

`nyx-glimpse benchmark-search` compares the unchanged previous lexical `name_rank`
with the new Matcher in one process, using identical synthetic names and 20 scans
per query. This compares matching work, not the complete previous launcher.
Raw samples: [matcher-benchmark.json](matcher-benchmark.json).

100,000-name scans, milliseconds:

| Query | Previous median / p95 | Fuzzy median / p95 | Previous / new matches |
|---|---:|---:|---:|
| project report | 12.93 / 13.11 | 7.01 / 7.13 | 100000 / 100000 |
| porject report | 11.85 / 12.43 | 40.06 / 40.85 | 0 / 100000 |
| final | 10.66 / 11.05 | 7.98 / 8.10 | 100000 / 100000 |
| report 2026 | 12.16 / 12.78 | 6.59 / 7.10 | 1 / 1 |
| unrelated | 10.12 / 11.10 | 33.05 / 34.08 | 0 / 0 |

The new matcher constructs the query once. Typo/nonmatch scans cost more because
they evaluate candidate corrections. The small fixed eight-case relevance corpus
covers exact names, transpositions, subsequences, initials, exact numeric tokens,
and no match; all expected new top results pass the focused test. This is a
regression fixture, not an independently annotated real-world search evaluation.

## Complete filename provider

`cargo run --release --locked -p nyx-glimpse --example benchmark-providers` creates
10k/100k synthetic SQLite rows inside private temporary directories. It measures
catalogue loading, shared personalization/relevance scoring, sorting, scope checks,
and result materialization. No real files, sources, services, or models are indexed.
There are 20 warm requests per query and one cold catalogue load per size.
Raw results: [provider-benchmark.json](provider-benchmark.json).

| Entries | Query | Median / p95, ms |
|---:|---|---:|
| 10,000 | project report | 1.76 / 1.79 |
| 10,000 | porject report | 4.99 / 5.57 |
| 10,000 | report 2026 | 1.19 / 1.22 |
| 10,000 | unrelated | 4.07 / 4.25 |
| 100,000 | project report | 28.14 / 29.10 |
| 100,000 | porject report | 72.19 / 160.75 |
| 100,000 | report 2026 | 15.70 / 20.14 |
| 100,000 | unrelated | 61.05 / 68.44 |

Cold loads: 8.48 ms / 103.12 ms for 10k / 100k. Catalogue RSS deltas were
6,244 / 44,120 KiB; full benchmark-process RSS was approximately 14 / 57 MiB.
These are Linux RSS observations, not precise allocation accounting. The cache is
lazy and window-local; it does not add an always-running process. Applications are
published before filename lookup completes so this work does not gate app results.

## Native rendering and actions

`python3 tools/check-glimpse-ui.py` runs one ignored native GTK regression using
scratch XDG directories, a synthetic desktop entry, and its own temporary headless
output. It leaves the physical keyboard alone and removes its output afterward.
It exercises opening the panel through the same native action, alias editing and
search, the panel's Escape handler, and preserving the query. A screenshot was
inspected locally: a solid themed action panel with Open, New Window, favorites,
alias editing, and details.

The recorded release-test sample took 148.90 ms from test startup (before GTK
initialization) to the first painted frame. First result frames followed synthetic
text changes in 63.64 ms and 34.62 ms. Later enrichment/refresh frames arrived around
244–555 ms. GUI-process RSS was 170,736 KiB. This is one workflow, with two text
changes; it has no meaningful GUI p95. The native test harness is not the production
CLI's startup path, so this does not establish production startup overhead.
Raw frame events: [native-ui-timings.json](native-ui-timings.json).

`GLIMPSE_TRACE=1` opts into timing-only stderr events with no queries, filenames,
result identities, or credentials. `search-to-frame` events include later updates;
consumers must distinguish first results from enrichment. Timings start at text
change and end at GTK's after-paint signal, not compositor presentation feedback.

The existing 70 ms worker coalescing delay was reduced to 10 ms and SearchEntry's
additional delay removed. The previous UI probe showed a first search frame after
114.90 ms, but its startup/accessibility environment differed, so it is not a
controlled before/after comparison. No speedup percentage is claimed.

## Checks and remaining measurements

- Nine standalone workflow tests passed: matching, relevance, learned aliases,
  bounded personalization, privacy, clear/disable behavior, file refresh/revocation,
  shortcut validation, JSONC/injection, provider registration, and sandboxed plugin
  contextual dispatch with renewed grants and revocation.
- The native action/alias/Escape regression passed on the isolated output.
- Targeted library/binary/workflow/example Clippy passed with warnings denied.
- Release binaries built; formatting was applied directly to affected Rust files.
  Workspace-wide cargo fmt/core tests encounter a pre-existing absent Settings
  protocol source referenced by `tests/core.rs`; that harness was not repaired here.

Physical hotkey-to-visible latency, physical keystroke-to-presented-results latency,
controlled production startup overhead versus the baseline binary, live vault
authentication, and execution of every discovered Omarchy action remain unverified.
The accessibility bus was unavailable during an initial automation attempt; the
native regression avoids it and does not establish accessibility conformance.
