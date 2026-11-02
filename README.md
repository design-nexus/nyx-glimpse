# Glimpse

Nyx’s local universal search panel. Apps, filenames, arithmetic, conversions,
semantic document/media search, and plugins share one keyboard-driven interface.
Glimpse follows the suite theme. Sources start empty and models download only when requested.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/design-nexus/nyx-glimpse/main/install.sh | bash
```

## Build and run

```sh
cargo build -p nyx-glimpse
./target/debug/nyx-glimpse
./target/debug/nyx-glimpse --plugins
```

Installing (see Install above) registers the desktop
entry and one Glimpse page under Desktop in Settings. It installs a reversible floating-window
rule; shortcut replacement is a separate, explicit Settings action. Default bindings
are Super+Space for Glimpse and Ctrl+Space for the Omarchy menu, replacing the existing
menu and terminal input-mode actions. Both shortcuts can be changed in Settings.

The panel opens with only a focused search field and hides when focus moves outside it. Typing reveals results grouped by category, with applications first. Calculations and conversions show only utility results: `2+2` or `2+2=` becomes `2 + 2 = 4`, with an interactive calculator preview. `1 oz to lb` shows the requested result followed by kilograms and grams. Clearing the query collapses the panel again. Plugin windows have a Close button and support Escape.

Use arrows to select, Enter to activate, Ctrl+Enter to reveal a file’s folder, and
Escape to close. Ctrl+K opens searchable actions for the selected result; Escape
returns to the same query and selection. Ctrl+Shift+F toggles a favorite. Both
shortcuts can be changed in Settings. Up on an empty search opens saved query history.
Use Set alias in the action panel to assign or remove an alias. Applications expose
New Window or Settings when supported by their desktop entries. Utility results copy to the clipboard. Reboot and shutdown require
another activation. The panel’s folder button selects sources; indexing then starts
at login through a user service.

## Local AI

The crawler, extraction, SQLite index, ranking, and Rust/C++ inference wrapper are
Glimpse code. No Recall production code is included. The optional inference helper
uses a pinned, unmodified llama.cpp library and verified EmbeddingGemma 2 GGUF models.

```sh
nyx-glimpse models setup       # explicit build; no model download
nyx-glimpse models download    # explicit 865 MB download and checksum verification
```

For ordinary use, simply enable Local AI in Settings → Desktop → Glimpse. It prepares the engine and downloads the package automatically; the commands above remain available for manual maintenance. CPU is supported by
all runtime builds. Build optional backends with `GLIMPSE_BACKENDS="cpu cuda"` or
`"cpu vulkan"`; `GLIMPSE_BUILD_JOBS` limits compilation (default 4, maximum 16).
Automatic device selection falls back to CPU; short queries use the CPU encoder.

Poppler is needed for PDFs; ffmpeg/ffprobe for media; unzip for Office XML formats.
Tesseract OCR is optional. Missing readers are recorded as indexing errors and do
not stop filename search. Audio retrieval finds moments; it does not transcribe.
Long PDFs/documents/media continue across bounded indexing passes.

## Passwords

With [Passwords](../passwords) installed, a query starting with `pw` (or `password`)
searches your vault and nothing else: no apps, files, plugins or local AI see it.

| Query | Shows |
| --- | --- |
| `pw` | Items you copied from recently |
| `pw github` | Matching items, by name, username or site |
| `pw github pass` / `otp` / `open` | The same, with Enter copying the password, the one-time code, or opening the site |
| `pw generate` | A new password, copied |
| `pw lock` | Locks the vault |
| `pw aliases` | Opens Passwords' Email aliases page |

Enter copies the username; Ctrl+Enter the password; Alt+Enter the one-time code;
Ctrl+O opens the site. Passwords are never shown in Glimpse, and the copy is cleared
from the clipboard after Passwords' set time. A locked vault offers to unlock in
Passwords. Turn this off in Passwords' settings.

## Plugins

Open Plugins from the panel or Settings. Browse includes the bundled Quick Tools
plugin (`uuid` and `utc`), and the catalog can be refreshed explicitly. Install custom
plugins from a local folder or HTTPS Git repository plus immutable commit hash.
Installed plugins start disabled. Review permissions, then enable them.

```sh
nyx-glimpse plugins install quick-tools
nyx-glimpse plugins enable quick-tools --approve-permissions
nyx-glimpse plugins install-local ./my-plugin
nyx-glimpse plugins install-git https://example.org/plugin.git FULL_COMMIT_HASH
nyx-glimpse plugins link-local ./my-plugin     # explicit development mode
nyx-glimpse plugins disable my-plugin
nyx-glimpse plugins update my-plugin
nyx-glimpse plugins rollback my-plugin
nyx-glimpse plugins remove my-plugin          # retains private settings/data
```

Plugins add results, commands, previews, file readers, and native settings without
changing Glimpse. They run through a versioned process protocol in bubblewrap,
with bounded time, memory, CPU, and output. Network access is off unless approved;
source access is granted explicitly. New sources do not silently expand existing
grants. Dependencies are reported rather than automatically installed. Installation
never runs plugin setup scripts. See [PLUGIN_API.md](apps/glimpse/PLUGIN_API.md) and examples.

## Data and management

Preferences: `$XDG_CONFIG_HOME/nyx-glimpse/settings.toml`.
Indexes, models, plugins: `$XDG_DATA_HOME/nyx-glimpse`.
Private operational state: `$XDG_STATE_HOME/nyx-glimpse`.
The standard XDG fallbacks apply. Learning is enabled by default and stores accepted search selections locally for up to 90 days.
Vault queries and utility expressions are never retained. Disable learning or clear its
data in Settings → Desktop → Glimpse. Favorites and explicit aliases are kept separately.
There is no telemetry.
Enabled network plugins can receive queries; their permission is visible before enablement.

```sh
nyx-glimpse index pause
nyx-glimpse index resume
nyx-glimpse index scan
nyx-glimpse index status
nyx-glimpse sources exclude /absolute/folder
nyx-glimpse diagnostics
./uninstall.sh glimpse             # restores integration, retains user data
./uninstall.sh glimpse --purge     # also deletes preferences, indexes and models
```

Removed/excluded sources disappear from results immediately. Unmounted folders keep
their index records. Activation rechecks current scope and plugin permissions.
Currency conversions automatically use daily Frankfurter rates, cached locally for 24 hours. Results show the publication date; offline results are labelled as cached. Search text and amounts stay local—the only rate request is a fixed public USD-based table.

## Verification

During implementation, use compile checks. At the final checkpoint:

```sh
cargo fmt -p nyx-glimpse -p glimpse-plugin-sdk --check
cargo test -p nyx-glimpse
cargo clippy -p nyx-glimpse -p glimpse-plugin-sdk --all-targets -- -D warnings
cargo test -p nyx-glimpse --test semantic -- --ignored
```

The last test requires the explicitly installed models/runtime and exercises real
text, PDF, image, audio, video, and source filtering. Performance targets remain
unverified until measured; see [IMPLEMENTATION.md](apps/glimpse/IMPLEMENTATION.md).


Glimpse’s Settings page uses native folding sections. The page has five sections: General, Sources, Local AI, Plugins, and Appearance. General keeps the common indexing
controls visible; Sources summarizes folder/exclusion counts, with each folder’s options
inside its disclosure row. Section and folder expansion state survive polling.

Normal search supports word abbreviations and bounded typo correction, including
transposed letters. All terms must match; quoted phrases stay literal. Favorites
and decayed usage rank comparable matches without promoting unrelated results.
Exact names outrank learned terms, and applications remain the first category.
Normal search matches numeric terms exactly (`2` does not match
`2026`). Application names rank ahead of description matches. Descriptive queries can
also show up to five conservative semantic suggestions under Related results; use
`semantic:` or `code:` to request semantic retrieval explicitly. Math is recognized
through operators, parentheses, or an initial `=`; a bare number remains a search.
Unit conversion accepts attached quantities and common unit names. `oz` means mass;
`fl oz` means US fluid ounces. The launcher is 760 pixels wide.

Local AI exposes one enable toggle and a setup status. Its approximately 865 MB package
contains a text embedding model and its companion media encoder; these are managed as
one feature. Turning it off cancels pending setup while preserving downloaded assets.
Interrupted setup resumes when Glimpse next opens; failures expose Retry.

Appearance has only Follow Omarchy and a theme selector. Following Omarchy disables
the selector while retaining its selected value. Theme preferences remain shared across
Nyx apps. Changing status values updates labels in place without rebuilding Settings.
Advanced device, indexing-limit, manual-rate, diagnostic, and maintenance operations
remain available through CLI/configuration rather than occupying the main Settings page.


## Omarchy and contextual actions

Glimpse reads the installed Omarchy command metadata and menu definitions, including
user menu overrides. Supported actions run through existing Omarchy commands;
unsupported shell expressions and state conditions open the stock menu. Fonts,
kernels, and power profiles are discovered dynamically where their interfaces are
available. Counts depend on the installation. `service NAME` shows matching user
services and their status without starting or stopping them.

Passwords is a built-in provider, separate from plugins. Its action panel reuses
Passwords' authenticated copying and site opening; no vault content enters the
ordinary index, history, aliases, or third-party providers. Pin to Dock, Backup,
and opening a specific vault entry require public contracts not currently present.

```sh
nyx-glimpse favorite app:org.mozilla.firefox.desktop  # toggle
nyx-glimpse alias app:org.mozilla.firefox.desktop browser
nyx-glimpse alias app:org.mozilla.firefox.desktop     # remove
nyx-glimpse history list
nyx-glimpse history clear                           # keeps aliases/favorites
nyx-glimpse omarchy-actions                         # installed metadata, no execution
nyx-glimpse benchmark-search                        # synthetic; no models or downloads
```

The panel shortcut configuration uses GTK accelerator spelling, such as
`action_key = "<Control>k"` and `favorite_key = "<Control><Shift>f"`.
Reserved navigation, copying, Passwords, and playback keys cannot be replaced.

See [source audit](docs/GLIMPSE_AUDIT.md), [provider architecture](docs/PROVIDERS.md),
and [measurements and limitations](docs/PERFORMANCE.md).
