# Provider architecture

`apps/glimpse/src/providers.rs` defines the native contract:

- `Provider`: stable ID, cancellable `discover`, contextual `actions`, explicit `resolve`.
- `Candidate`: stable namespaced identity, searchable title/keywords, display metadata,
  and typed target. Targets never contain a general shell execution request.
- `SearchContext`: current paths, configuration, query, and cancellation predicate.
- `Registry`: rejects duplicate IDs, isolates provider errors, applies shared relevance
  and personalization ranking, and gives deterministic ties.
- `Effect`: app launch/desktop action, validated file open/reveal/copy, supported command,
  or a validated plugin action. The GTK host applies UI and clipboard effects.

Application and filename discovery run on the existing primary worker. Applications
are published before the filename cache or document-content retrieval finishes.
Omarchy and read-only user-service discovery run on the enrichment worker. The
extension adapter reuses the existing plugin process protocol. Category grouping
and selection stability remain owned by the UI. Utility intent bypasses ordinary
providers; semantic retrieval retains its existing session and Related threshold.

Passwords deliberately bypasses general discovery, ranking, aliases and plugins:
its exclusive query prefix reaches only the existing Passwords service and the GTK
client. Its action panel delegates to that same client. Authentication, reprompts,
clipboard lifetime and copying remain owned by Passwords.

## Local learning

`settings.toml` adds defaulted `learning`, `favorites`, `aliases`, `alias_queries`,
`action_key`, and `favorite_key`. Explicit plugin aliases retain their original
query so a fresh result can be requested without persisting action tokens.
`$XDG_STATE_HOME/nyx-glimpse/learning.sqlite` contains usage, accepted query terms,
and history. Its directory is private and the file is created with mode 0600.

Only accepted nonutility, nonvault activations are recorded. There is no keystroke
log. Scores decay with a 14-day half-life, terms/history expire after 90 days,
history is capped at 100, and learned terms at 20 per result. Clear removes the
three tables' data with SQLite secure deletion/VACUUM; disabling learning stops
using and adding learned state while retaining explicit preferences. Pending
recording is allowed to finish before the dismissed launcher exits.

Match quality precedes favorites and capped decayed-usage bonuses. Explicit exact
aliases lead; exact names outrank learned exact terms; prefix/word, abbreviation,
subsequence and bounded typo tiers follow. Description/keyword-only hits rank below
name hits. Unrelated favorites or high usage cannot qualify a result. Quoted
phrases and numeric terms remain exact, and all requested terms must qualify.

The filename catalogue is loaded lazily and invalidated by SQLite data_version or
scope changes. Only matching result rows are cloned. Current source/symlink policy
is checked before showing candidate files and again when opening/revealing/copying.
The cache lives only for the active launcher window; no daemon was introduced.

## Omarchy

Discovery reads `omarchy commands --json`, the installed root's default menu JSONC,
and the XDG user extension menu. Metadata resolves native binaries to Omarchy CLI
routes. Menu IDs, labels, aliases and ancestry supply searchable data; arbitrary
shell source is not searchable text. JSONC parsing respects strings and comments.

Refresh runs off the GTK thread, with a short cache and source modification checks.
Fonts, kernels and power profiles use existing list/current/set interfaces.
Supported state checks are bounded and memoized. Unrecognized actions, guards or
provider types open the matching stock menu. Resolution refreshes availability
before returning an explicit command effect; disruptive system/kernel actions
require confirmation. Existing scripts retain their authentication behavior.

## Extensions

The SDK keeps API_VERSION=1 and existing Hit/Action payloads. Its additive
`ContextAction` type is used by the optional contextual_actions capability.
New capability approval is recorded in grants; adding it disables an existing
package until approval, even when filesystem/network requests are unchanged.
Enumeration, re-enumeration on selection, token activation and final host effects
all use current grants. Existing timeout, resource, output and sandbox restrictions
apply. Updates or revocation invalidate pending replies.

The provider layer exposes no generic remote method proxy and no background
execution authority. Future app integrations must use app-owned public contracts;
Glimpse-side UI confirmation cannot replace authorization in a sensitive receiver.
