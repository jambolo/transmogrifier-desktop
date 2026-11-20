# Development

How to build, test, and change Transmogrifier.

## Requirements

- [Rust](https://rustup.rs/) (stable) and [Node.js](https://nodejs.org/) (LTS) with [pnpm](https://pnpm.io/)
- Windows: the MSVC build tools and the WebView2 runtime (part of Windows 10 and later)
- Linux: Tauri's system libraries (WebKitGTK 4.1 and friends) and OpenSSL headers; see the [Tauri prerequisites](https://tauri.app/start/prerequisites/). On Debian or Ubuntu:

  ```bash
  sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
  ```

Tauri doesn't cross-compile: build the Windows app on Windows and the Linux app on Linux.

## Building

```bash
pnpm install
pnpm tauri dev      # run the app with hot reload for the front end
pnpm tauri build    # build installers for the current platform
```

Installers are written to `target/release/bundle/`. To build and check without packaging:

```bash
pnpm build                                   # type-check and bundle the front end into dist/
cargo build --workspace --all-targets
```

Build the front end first: `tauri::generate_context!` embeds `dist/` at compile time, so the Rust build fails without it.

Formatting and linting:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
pnpm lint
pnpm format          # or pnpm format:check
```

Rust lines are at most 132 characters (`rustfmt.toml`); TypeScript follows `.prettierrc.json`.

## Tests

- **Core tests** (`src-core`) run with `cargo test --workspace`. They need no window, network, or API key: provider tests replay SSE fixtures from `src-core/tests/fixtures`, and the generation loop runs against a scripted provider.
- **Shell tests** (`src-tauri`) pin the JSON shapes the front end reads.
- **Front-end tests** (`src/**/*.test.ts`) run with `pnpm test` in jsdom. `api.test.ts` checks each command's name and arguments with Tauri's IPC mocks; `main.test.ts` drives the views against a mocked `api` module.

Nothing automated opens a real window. After changing `runner.rs`, the `site` protocol, or the reporter script, run the app and open a generated app on both platforms.

Coverage:

```bash
cargo llvm-cov --workspace --lcov --output-path lcov.info
pnpm coverage
```

## CI

GitHub Actions (`.github/workflows/ci.yml`) builds and tests both stacks on Linux, Windows, and macOS for every push to `master`, `develop`, and `release/**` and for every pull request. Formatting and lint checks run on pull requests. Pushes to `develop` also upload coverage from both stacks to Codecov, which needs the repository secret `CODECOV_TOKEN`.

`.github/workflows/cd.yml` runs when `package.json` changes on `master`: it builds and tests, tags `v<version>` if that tag doesn't exist, and merges `master` into `develop`. When it created a new tag, a `publish` job then builds the installers on Windows (NSIS `.exe`) and Linux (`.deb`, `.rpm`, and AppImage) with `tauri-apps/tauri-action` and attaches them to a GitHub release for that tag. The installers are unsigned. The bundle formats are set in `bundle.targets` in `src-tauri/tauri.conf.json`.

To release: bump `version` in `package.json`, `src-tauri/tauri.conf.json`, and both crate `Cargo.toml`s in one commit on the `release/*` branch, verify the build, and merge to `master` with `--no-ff`.

## Architecture

| Layer | Responsibility |
| --- | --- |
| `src-core` | All application logic and its tests. No Tauri dependency. |
| `src-tauri` | The shell: owns the windows, exposes the core as commands, defines the JSON protocol |
| `src` | The front end. `api.ts` is the only module that imports `@tauri-apps/api` and mirrors the protocol types |

Core modules:

| Module | Responsibility |
| --- | --- |
| `llm` | Provider-neutral `LlmProvider` trait, with `anthropic`, `openai`, `gemini`, and `chat_completions` adapters over `reqwest` |
| `agent` | Generation loop (`AgentEngine`), system prompt, tool definitions and executor |
| `project` | Project storage (files, metadata, conversation history) and zip export/import |
| `settings` | Provider and model selection, server addresses, limits, and the `SecretStore` trait for API keys |
| `runtime` | Serving a generated app (file lookup, content security policy) and its console |

The shell has `main.rs` (state, commands, protocol types), `runner.rs` (windows for generated apps), `shortcut.rs` (desktop shortcuts and the launch argument), and `secrets.rs` (the credential store). The front end has one module per view under `src/views`, `strings.ts` for all interface wording, and `dom.ts` for element helpers and dialogs.

Long work never runs on the UI thread: a generation is a task on the async runtime that reports progress to the main window as `generation` events and ends with `generation-finished`; file dialogs, zips, and the credential store run under `spawn_blocking`.

### The shared specification

Transmogrifier has more than one implementation, and projects and backups move between them. These parts are a shared specification, not this app's own choices, and must stay identical everywhere:

- **The system prompt and tool definitions**, byte for byte (`agent/prompt.rs`). This is also the prompt-caching invariant. Tests pin the prompt's length and hash and every tool's serialized schema, including key order.
- **The project layout**: `project.json`, `transcript.jsonl`, `conversation.json`, and `site/`, with the field names, enum spellings, and omitted defaults that the tests in `project/store.rs` pin.
- **The workspace limits**: `.html`, `.css`, `.js`, `.mjs`, `.json`, `.svg`, `.txt`, and `.md`; paths of at most 200 characters and depth 6; 100 files; 512 KiB per file; 4 MiB in total.
- **The tool-call semantics**: what each tool does and the exact result and error text the model sees (`agent/tools.rs`, `project/workspace.rs`), and the notes the engine adds to user text (`agent/engine.rs`).

A failing pin test means the specification changed here alone. Change every implementation together or not at all.

This app adds two things to the layout: `formFactor` in `conversation.json` (see the design notes), and the providers `GEMINI`, `OPENAI_COMPATIBLE`, and `LOCAL`. An implementation without one of those providers can't read that app's `project.json`, so its backup only opens where the provider exists; its exported app files open anywhere. Fields this version doesn't know in `project.json` and `conversation.json` are kept when the file is rewritten, and transcript lines it can't read are skipped, so data from another version survives.

### Key invariants

- Conversation history is stored per provider in that provider's own wire format, as opaque append-only JSON. Never translate it between providers or edit it: Claude's thinking blocks must round-trip unchanged.
- The model changes the generated site only through tools (`list_files`, `read_file`, `write_file`, `edit_file`, `delete_file`). `Workspace` rejects absolute paths, `..`, disallowed extensions, and over-limit writes, and these rejections go back to the model as error tool results.
- API keys are kept in the system credential store through the `keyring` crate and never leave `settings`, `llm`, and `secrets.rs`. `settings.json` and the front end hold only the last four characters. Server addresses aren't secrets and are kept in `settings.json`.
- Only the `main` window may invoke commands. The invoke handler in `main.rs` refuses every other window, and no capability lists them. Tauri counts the `site` protocol as a local origin, so this guard, not the origin, is what keeps generated code away from the commands.
- The system prompt and tool definitions must stay byte-identical between requests so prompt caching hits.

### Design notes

- **Generated apps:** a static site in `site/` with the entry point `index.html`, relative paths, and no build step.
- **Running an app:** each app opens in its own window, served by the `site` protocol handler from its `site/` folder at `site://<project id>/` (Windows shows this as `http://site.<project id>/`), so every project has its own origin and storage. Every response carries a Content Security Policy that blocks other origins. With "Let apps use the network" on (read when the window opens), https `fetch`/WebSocket/image/media requests pass; remote scripts, styles, fonts, and plain http stay blocked. Unlike a request interceptor, the policy is the only network block, so every response must carry it. Navigation away from the origin is cancelled; http, https, mailto, and tel links are offered to the main window, which opens them in the system browser after the user confirms.
- **Desktop shortcuts:** a shortcut starts Transmogrifier with `--open <project id>`. The front end takes that request once at startup (`take_launch_request`): an app with files opens in the workspace and runs, one without files opens at its chat, and a missing one shows the list with an error. `tauri-plugin-single-instance` keeps one copy running; a later launch focuses the main window and arrives as a `launch` event. On Linux the shortcut is `transmogrifier-<id>.desktop` on the XDG desktop, made executable and marked trusted with `gio` for GNOME, with the icon copied to the app data folder; from an AppImage it runs `$APPIMAGE`. On Windows it is a `.lnk` named after the app, written through `IShellLinkW`, with a numbered name rather than replacing an existing file. The ID is checked to be a UUID-shaped string before it goes into a shortcut or is accepted from one.
- **The app's console:** webviews don't expose the console to the host, so a script installed at document start wraps `console`, listens for uncaught errors, unhandled rejections, and policy violations, and reports each as a GET to `/.transmogrifier/report` on the app's own origin. The workspace rejects path segments that start with a dot, so no site file can have that path. The handler also logs a 404 for any missing file. A page can forge these reports, which only affects its own log.
- **Storage:** each project lives in `<app data>/projects/<id>/`, where the app data folder is `%APPDATA%\io.github.jambolo.transmogrifier` on Windows and `~/.local/share/io.github.jambolo.transmogrifier` on Linux. Settings are in `settings.json` beside it. If the credential store loses a key, its hint is cleared and the key must be entered again.
- **Export and import:** `project::archive` writes two kinds of zip. *Export app files* holds only the site, at the zip's root, ready to host anywhere; it is separate from the backup so that publishing a site never publishes the conversation. *Back up* holds `manifest.json` (`format` and `version`), `project.json`, `transcript.jsonl`, `conversation.json`, and the site under `site/`. Import accepts either kind and always creates a new project ID, so the app's saved browser data (localStorage, IndexedDB) starts empty; that data isn't exported. Archives are untrusted: the zip is read into memory with limits on file count and size, OS junk files (`__MACOSX`, dotfiles) are skipped, every site file goes through `Workspace`, and a failed import deletes the new project. The history is copied unchanged. A backup's conversation can be dropped on import (the transcript then ends with an `IMPORTED_WITHOUT_CONVERSATION` notice), in case a conversation can't be continued on another install. *Reset conversation* (`ProjectStore::reset_conversation`) deletes `conversation.json` the same way, keeps the site and transcript, and appends a `CONVERSATION_RESET` notice. An implementation that doesn't know that kind skips the line. A site-only zip takes its name from the file, with the new-app provider and model; if all its files are in one folder, that folder is the site.
- **The system prompt never changes.** Editing it changes the prefix of every existing Claude conversation, which `prefix_mismatch_behavior: "error"` rejects. Instructions for all apps, the network setting, and the app's name ride on the next user text instead, and `Conversation` records what was last delivered. This is why the prompt still describes network access as blocked.
- **Desktop form factor:** the shared system prompt describes a phone. This app's generated apps open in a desktop window, so the engine announces that on the next user text, the same way as the other settings, and records it as `formFactor: "desktop"` in `conversation.json`. A conversation that arrives without the field gets the note once. The note asks for layouts that still work in a narrow window, so apps stay usable on a phone.
- **Generation loop:** `AgentEngine` saves the input it is about to send as `pending` before each turn, so Stop, a failure, or the step limit can resume with **Continue**. Only complete turns are appended. Tool calls from a turn cut off by `max_tokens` or a refusal are never run. The HTTP client makes one attempt; the engine retries rate limits, server errors, and network failures after 2, 5, and 15 seconds. Stop cancels a token: dropping the turn's future closes the stream, and tool calls not yet run get an error result so the conversation can continue.
- **Claude:** requests go to the beta Messages API. Each beta header is sent only to models that accept it. The assistant message is accumulated from the stream's content blocks and stored as received. `pause_turn` maps to `StopReason::Paused`, and the engine resumes with `TurnInput::resume()`.
- **OpenAI:** requests replay the history statelessly (`store: false`). The request drops null fields from the copy it sends: output items carry `status: null`, which OpenAI rejects as input. The stored history keeps them. `*codex*` model IDs are deprecated, so the model list is filtered to the coding families.
- **Gemini:** requests go to the streaming `generateContent` API, and the history is its `contents`. Streamed text parts are joined, but a part with a `thoughtSignature` is stored exactly as sent and never joined to another, because the signature is only valid for that part. A function call without an `id` gets one from its position in the history (`call_<content>_<part>`), and a function response repeats its call's name and `id`, found in the model's last turn. Responses go in a content of their own, ahead of any user text. A turn without output adds no model content, since a content must have parts. `thinkingLevel` is sent only to Gemini 3 and later, and effort above high maps to `high`. Google Search isn't offered to the model.
- **OpenAI-compatible and local servers:** one adapter, `chat_completions`, registered under two providers. It speaks the Chat Completions format to the address in `Settings::endpoints`, which request paths are appended to (so it usually ends in `/v1`); the API key is optional and sent as a bearer token when saved. The request holds only `model`, `messages`, `tools`, and `stream`: effort, the output limit, and usage options are left out because some servers reject them, so those settings don't apply. The adapter tolerates common departures from the format: tool calls without IDs or all at index 0, arguments sent as an object, and `stop` as the finish reason of a tool turn. Arguments that aren't a JSON object are stored as `{}`, because servers parse every replayed call. Reasoning text (`reasoning_content` and the like) isn't stored, so a service that requires it back during a tool loop won't work. These providers have no default model and are ready for new apps only once an address and a model are saved (`Settings::is_ready`).
- **Provider fixtures:** the SSE fixtures are written by hand to the documented wire format, so they can miss what the live API really sends. After changing a provider, run a real generation.
- **Platform differences:** `alert()`, `confirm()`, `prompt()`, and `speechSynthesis` in a generated app are whatever the system webview provides (WebView2 on Windows, WebKitGTK on Linux).

### Adding an LLM provider

Providers are adapters behind `LlmProvider`, and each wire format stays inside its own `llm` submodule. To add one:

1. Add a variant, with its default model, to `ProviderId`. If the user must supply its address, add it to `uses_endpoint`.
2. Implement `LlmProvider` in a new `llm/<name>.rs`, or register an existing adapter under the new ID if the wire format is the same. Map the service's errors to `LlmError`, and store history in the provider's own wire format.
3. Register the adapter in `AppState::new` in `src-tauri/src/main.rs`, and give it an account name in `secrets.rs`.
4. Add it to `ProviderId` and `PROVIDERS` in `src/api.ts` (and to `usesEndpoint` if it has an address), and its entry, with the services it covers, to `PROVIDER_INFO` and `DEFAULT_MODELS` in `src/strings.ts`; the compiler flags any that are missing. The settings page builds its card from those.

## Branding

The identity is documented in [docs/branding](docs/branding/README.md). The theme is `src/styles.css`, interface wording is `src/strings.ts`, and the icon set in `src-tauri/icons/` is generated with:

```bash
pnpm tauri icon docs/branding/assets/icons/app-icon.svg
```
