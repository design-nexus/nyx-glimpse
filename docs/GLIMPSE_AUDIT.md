# Glimpse audit and delivery

Audit baseline: `design-nexus/nyx-glimpse` at
`3ed5d0477d20a9e610064740a5d2c6ad87149112`, initially clean. Local origin and
workspace repository metadata agree. Repository AGENTS.md and the pinned
nyx-design revision `144e77df1b6561fd8d0667e0d58306696c3e4d63` govern styling.
The live panel was already 760 px wide; older README layout descriptions were stale.

Oma-Cast reference: `typecraft-dev/oma-cast` at
`582bc73466ca25903630d2ebe76c4d63f7b86a68`. Actual source inspected:
[Ranker.js](https://github.com/typecraft-dev/oma-cast/blob/582bc73466ca25903630d2ebe76c4d63f7b86a68/plugin/core/Ranker.js),
[Frecency.js](https://github.com/typecraft-dev/oma-cast/blob/582bc73466ca25903630d2ebe76c4d63f7b86a68/plugin/core/Frecency.js),
[OmarchyMenu.js](https://github.com/typecraft-dev/oma-cast/blob/582bc73466ca25903630d2ebe76c4d63f7b86a68/plugin/core/OmarchyMenu.js),
[OmarchyMenuSource.qml](https://github.com/typecraft-dev/oma-cast/blob/582bc73466ca25903630d2ebe76c4d63f7b86a68/plugin/services/OmarchyMenuSource.qml),
[ActionPanel.qml](https://github.com/typecraft-dev/oma-cast/blob/582bc73466ca25903630d2ebe76c4d63f7b86a68/plugin/ui/ActionPanel.qml),
and the Rust matcher/engine. Its separation of textual relevance, usage learning,
and contextual actions informed this work. No reference code, fork, QML frontend,
or external search helper was incorporated.

## Source-level gaps

| Capability | Baseline evidence | Delivery |
|---|---|---|
| Apps, files, content, utilities | `apps/glimpse/src/ui.rs`, `index.rs`, `utility.rs` | Reused; math and conversion remain exclusive |
| Numeric/phrase/conjunctive matching | `query.rs`; `tests/core.rs::lexical_queries_require_all_terms_and_exact_numbers` | Preserved, with separate bounded fuzzy name matching |
| Fuzzy/typo matching | Baseline `name_rank` only exact/prefix terms | Added Matcher with abbreviations, subsequences, edit distance and transpositions |
| Frequency/recency, learned terms | No usage store | Private local SQLite; 14-day half-life, 90-day retention |
| Favorites and aliases | Missing | Explicit preferences; native action panel and CLI |
| Incremental search | Existing worker, generations, reusable DB, 70 ms coalescing, asynchronous enrichment | Reused; 10 ms coalescing, immediate SearchEntry changes, apps published before file lookup |
| Context panel | Existing actions embedded in key handlers | Searchable native Ctrl+K panel; existing shortcuts preserved |
| Shortcuts | Global search/menu bindings already configurable | Separate validated panel bindings added |
| Omarchy actions | Four system operations plus theme/shortcut integration | Installed command/menu discovery, supported dynamic choices and safe menu fallback |
| Passwords | Built-in `pw.rs`/`passwords.rs`, scoped Passwords protocol | Reused; never converted to a plugin or general index |
| Extensions | SDK v1, bubblewrap, grants, cancellation, local/Git installs | Optional contextual_actions capability; renewed approval and fresh dispatch checks |
| Provider abstraction | UI-specific enumeration/dispatch | Native data-only provider interface and explicit effects; specialized utility/semantic/Passwords paths retained |
| Performance | Historical test notes; requested large-index figures absent | Reproducible matcher/provider measurements and opt-in rendered-frame instrumentation |

## Compatibility and security

Existing IDs, binary names, data locations, plugin Hit payloads, inference/indexing
services, source exclusions, and global shortcut configuration remain intact.
Defaulted configuration additions need no destructive migration. The new usage
store is separate from existing filename/content indexes. Learning defaults on by
explicit user choice; README and Settings describe disabling and clearing it.

The existing bounded subprocess implementation, grant registry, sandbox, XDG
private directories, atomic preference writes, generation cancellation, shared
widgets, theme tokens, and Passwords copy service were reused. `gio-unix` adds the
standard desktop-action binding to the existing GIO runtime, not a new service.

Unlike Oma-Cast's shell-based menu guard execution, this adapter supports typed,
read-only toggle checks. Unsupported expressions and script actions hand off to
Omarchy's menu. Commands receive argument arrays; user query text is never shell
code. Dynamic choices are only exposed after supported ancestor availability checks.
Sensitive credentials are not returned to Glimpse by Passwords copy operations.

## Remaining work and verification boundaries

- Pin to Nyx Dock: the local Dock has UI pinning but no public pin CLI/D-Bus contract.
- Nyx Backup and a separate Nyx Vault: no corresponding public app contracts found.
- Open a specific Passwords entry: the existing launcher contract supports copying,
  site opening, and authenticated app handoff, not entry navigation.
- Arbitrary menu shell conditions and unknown providers intentionally use the stock
  menu; this is supported fallback, not full direct-execution parity.
- Full service control, unrestricted D-Bus method discovery/execution, and a general
  extension host-command permission are not exposed.
- Physical hotkey/input latency, real authenticated Passwords actions, and every
  discovered Omarchy command have not been exercised. No destructive command,
  model download, or inference was used for verification.
- The pre-existing core test harness references absent `apps/settings/src/ext/protocol.rs`.
  Focused standalone workflow tests avoid that missing sibling-source dependency.
