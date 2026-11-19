// The single door between the UI and the Rust backend.
//
// Every backend interaction goes through this module: nothing else in the
// front end imports from `@tauri-apps/api` directly. That keeps the Rust<->JS
// boundary in one place, so if the protocol changes there is exactly one file
// to update. The types mirror the shell's protocol (`src-tauri/src/main.rs`)
// and the core types it passes through.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type ProviderId = 'ANTHROPIC' | 'OPENAI';
export const PROVIDERS: readonly ProviderId[] = ['ANTHROPIC', 'OPENAI'];

export type Effort = 'LOW' | 'MEDIUM' | 'HIGH' | 'XHIGH' | 'MAX';
export const EFFORTS: readonly Effort[] = ['LOW', 'MEDIUM', 'HIGH', 'XHIGH', 'MAX'];

export interface Settings {
  defaultProvider: ProviderId;
  /** The user's choices; providers without one use the built-in default. */
  defaultModels: Partial<Record<ProviderId, string>>;
  effort: Effort;
  maxToolIterations: number;
  maxOutputTokens: number;
  appInstructions: string;
  allowAppNetwork: boolean;
  /** Last four characters of each saved key. The keys themselves never reach the front end. */
  keyHints: Partial<Record<ProviderId, string>>;
}

export interface ModelInfo {
  id: string;
  displayName: string;
}

export type NetworkFailure = 'HOST_NOT_FOUND' | 'TIMED_OUT' | 'CONNECTION_REFUSED' | 'INSECURE' | 'OTHER';

export interface KeyTestFailure {
  kind: 'MISSING_KEY' | 'INVALID_KEY' | 'PERMISSION_DENIED' | 'RATE_LIMITED' | 'OVERLOADED' | 'SERVER' | 'REJECTED' | 'NETWORK';
  network: NetworkFailure | null;
  status: number | null;
  detail: string | null;
}

export type KeyTest = { result: 'passed'; models: ModelInfo[] } | { result: 'failed'; error: KeyTestFailure };

export interface Project {
  id: string;
  name: string;
  provider: ProviderId;
  model: string;
  createdAtMillis: number;
  updatedAtMillis: number;
}

export type NoticeKind =
  | 'STOPPED'
  | 'ITERATION_LIMIT'
  | 'TRUNCATED'
  | 'REFUSED'
  | 'AUTH_FAILED'
  | 'FAILED'
  | 'PROVIDER_UNAVAILABLE'
  | 'IMPORTED_WITHOUT_CONVERSATION';

export type TranscriptEntry =
  | { type: 'prompt'; text: string; fixErrors?: boolean }
  | { type: 'reply'; text: string }
  | { type: 'tool'; name: string; path: string | null; error?: string }
  | { type: 'usage'; inputTokens: number; outputTokens: number; cacheReadTokens: number }
  | { type: 'notice'; kind: NoticeKind; detail?: string };

export interface FileEntry {
  path: string;
  sizeBytes: number;
}

/** What the main window shows about a project's running app. */
export interface RunnerStatus {
  projectId: string;
  running: boolean;
  errorCount: number;
  networkBlocked: boolean;
}

export interface ProjectView {
  project: Project;
  transcript: TranscriptEntry[];
  files: FileEntry[];
  generating: boolean;
  runner: RunnerStatus;
}

export type GenerationEvent =
  | { type: 'textDelta'; text: string }
  | { type: 'toolRunning'; name: string; path: string | null }
  | { type: 'turnPersisted' }
  | { type: 'retrying'; attempt: number; delaySeconds: number; reason: string };

export interface GenerationProgress {
  projectId: string;
  event: GenerationEvent;
}

export interface ConsoleMessage {
  level: 'LOG' | 'WARNING' | 'ERROR';
  message: string;
  source: string;
  line: number;
  stack: string | null;
}

export interface ConsoleView {
  messages: ConsoleMessage[];
  /** The errors in the form sent to the AI service, or null when there are none. */
  errorLog: string | null;
  networkBlocked: boolean;
  running: boolean;
}

/** A zip the user picked, checked but not yet imported. */
export interface ArchiveInfo {
  path: string;
  name: string;
  hasConversation: boolean;
}

/** Stops listening. */
export type Unsubscribe = () => void;

function subscribe<T>(event: string, handler: (payload: T) => void): Promise<Unsubscribe> {
  return listen<T>(event, (message) => handler(message.payload));
}

export const api = {
  getSettings(): Promise<Settings> {
    return invoke<Settings>('get_settings');
  },
  /** Saves everything except API keys and their hints. */
  updateSettings(settings: Settings): Promise<Settings> {
    return invoke<Settings>('update_settings', { settings });
  },
  /** Saves the key in the system credential store, or removes it when `key` is null. */
  setApiKey(provider: ProviderId, key: string | null): Promise<Settings> {
    return invoke<Settings>('set_api_key', { provider, key });
  },
  testApiKey(provider: ProviderId): Promise<KeyTest> {
    return invoke<KeyTest>('test_api_key', { provider });
  },

  listProjects(): Promise<Project[]> {
    return invoke<Project[]>('list_projects');
  },
  createProject(name: string): Promise<Project> {
    return invoke<Project>('create_project', { name });
  },
  renameProject(id: string, name: string): Promise<void> {
    return invoke<void>('rename_project', { id, name });
  },
  deleteProject(id: string): Promise<void> {
    return invoke<void>('delete_project', { id });
  },
  getProject(id: string): Promise<ProjectView> {
    return invoke<ProjectView>('get_project', { id });
  },
  /** A site file's text, or null if it can't be read. */
  readProjectFile(id: string, path: string): Promise<string | null> {
    return invoke<string | null>('read_project_file', { id, path });
  },

  /** Starts a generation; a null prompt continues the pending request. Progress arrives as events. */
  generate(id: string, prompt: string | null, fixErrors = false): Promise<void> {
    return invoke<void>('generate', { id, prompt, fixErrors });
  },
  stopGeneration(id: string): Promise<void> {
    return invoke<void>('stop_generation', { id });
  },

  runApp(id: string): Promise<void> {
    return invoke<void>('run_app', { id });
  },
  reloadApp(id: string): Promise<void> {
    return invoke<void>('reload_app', { id });
  },
  closeApp(id: string): Promise<void> {
    return invoke<void>('close_app', { id });
  },
  getConsole(id: string): Promise<ConsoleView> {
    return invoke<ConsoleView>('get_console', { id });
  },

  /** Asks where to save, then writes the zip. Resolves to the path, or null if the user cancelled. */
  exportProject(id: string, backup: boolean): Promise<string | null> {
    return invoke<string | null>('export_project', { id, backup });
  },
  /** Asks for a zip and checks it. Resolves to null if the user cancelled. */
  pickArchive(): Promise<ArchiveInfo | null> {
    return invoke<ArchiveInfo | null>('pick_archive');
  },
  importArchive(path: string, withConversation: boolean): Promise<Project> {
    return invoke<Project>('import_archive', { path, withConversation });
  },
  openExternal(url: string): Promise<void> {
    return invoke<void>('open_external', { url });
  },

  onGeneration(handler: (progress: GenerationProgress) => void): Promise<Unsubscribe> {
    return subscribe('generation', handler);
  },
  onGenerationFinished(handler: (projectId: string) => void): Promise<Unsubscribe> {
    return subscribe<{ projectId: string }>('generation-finished', (payload) => handler(payload.projectId));
  },
  onRunner(handler: (status: RunnerStatus) => void): Promise<Unsubscribe> {
    return subscribe('runner', handler);
  },
  /** A running app asked to open a link outside itself. */
  onExternalLink(handler: (url: string) => void): Promise<Unsubscribe> {
    return subscribe<{ url: string }>('external-link', (payload) => handler(payload.url));
  },
};

export type Api = typeof api;
