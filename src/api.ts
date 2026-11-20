// The single door between the UI and the Rust backend.
//
// Every backend interaction goes through this module: nothing else in the
// front end imports from `@tauri-apps/api` directly. That keeps the Rust<->JS
// boundary in one place, so if the protocol changes there is exactly one file
// to update. The types mirror the shell's protocol (`src-tauri/src/main.rs`)
// and the core types it passes through.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** A wire format. Which service speaks it, and where, is a `Connection`. */
export type ProviderId = 'ANTHROPIC' | 'OPENAI' | 'GEMINI' | 'OPENAI_COMPATIBLE';
export const PROVIDERS: readonly ProviderId[] = ['ANTHROPIC', 'OPENAI', 'GEMINI', 'OPENAI_COMPATIBLE'];

/** Whether the provider is reached at an address the user saves; mirrors `ProviderId::uses_endpoint`. */
export function usesEndpoint(provider: ProviderId): boolean {
  return provider === 'OPENAI_COMPATIBLE';
}

export type Effort = 'LOW' | 'MEDIUM' | 'HIGH' | 'XHIGH' | 'MAX';
export const EFFORTS: readonly Effort[] = ['LOW', 'MEDIUM', 'HIGH', 'XHIGH', 'MAX'];

/** One AI service the user can make apps with: a built-in one, or a server the user added. */
export interface Connection {
  id: string;
  /** The user's name for a server; empty for a built-in connection. */
  name: string;
  protocol: ProviderId;
  /** A server's address. */
  endpoint?: string;
  /** The user's choice of model for new apps; empty to use the provider's default. */
  model: string;
  /** Last four characters of the saved key. The key itself never reaches the front end. */
  keyHint?: string;
}

/** Changes to a connection; a missing field is left as it is. */
export interface ConnectionEdit {
  name?: string;
  endpoint?: string;
  model?: string;
}

export interface Settings {
  /** The built-in connections, then the user's servers. */
  connections: Connection[];
  /** The connection the last new app used. */
  lastConnection: string;
  effort: Effort;
  maxToolIterations: number;
  maxOutputTokens: number;
  appInstructions: string;
  allowAppNetwork: boolean;
}

/** Whether new apps can use the connection; mirrors `Connection::is_ready`. */
export function connectionReady(connection: Connection): boolean {
  if (!usesEndpoint(connection.protocol)) return connection.keyHint !== undefined;
  return connection.endpoint !== undefined && connection.model !== '';
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
  /** The saved connection the app uses, when its provider can have several. */
  connection?: string;
  model: string;
  createdAtMillis: number;
  updatedAtMillis: number;
}

/** The connection an app uses, or undefined if it isn't set up here; mirrors `Settings::connection_for`. */
export function connectionFor(settings: Settings, project: Project): Connection | undefined {
  const candidates = settings.connections.filter((connection) => connection.protocol === project.provider);
  if (project.connection !== undefined) return candidates.find((connection) => connection.id === project.connection);
  return candidates.length === 1 ? candidates[0] : undefined;
}

export type NoticeKind =
  | 'STOPPED'
  | 'ITERATION_LIMIT'
  | 'TRUNCATED'
  | 'REFUSED'
  | 'AUTH_FAILED'
  | 'FAILED'
  | 'PROVIDER_UNAVAILABLE'
  | 'IMPORTED_WITHOUT_CONVERSATION'
  | 'CONVERSATION_RESET';

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
  /** Whether the zip is a backup, which says what service made the app; app files alone don't. */
  hasProject: boolean;
  hasConversation: boolean;
}

/** The connection and model chosen for an app. */
export interface ServiceChoice {
  connection: string;
  model: string;
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
  /** Saves everything except the connections. */
  updateSettings(settings: Settings): Promise<Settings> {
    return invoke<Settings>('update_settings', { settings });
  },
  /** Saves a connection to a server. It is the last of the returned connections. */
  addConnection(name: string, endpoint: string): Promise<Settings> {
    return invoke<Settings>('add_connection', { name, endpoint });
  },
  /** Changes a connection's model, or a server's name or address. */
  editConnection(id: string, edit: ConnectionEdit): Promise<Settings> {
    return invoke<Settings>('edit_connection', { id, edit });
  },
  /** Forgets a server and its key. */
  removeConnection(id: string): Promise<Settings> {
    return invoke<Settings>('remove_connection', { id });
  },
  /** Saves the connection's key in the system credential store, or removes it when `key` is null. */
  setApiKey(connection: string, key: string | null): Promise<Settings> {
    return invoke<Settings>('set_api_key', { connection, key });
  },
  /** Checks the connection's saved key and address by listing the models they can use. */
  testApiKey(connection: string): Promise<KeyTest> {
    return invoke<KeyTest>('test_api_key', { connection });
  },

  listProjects(): Promise<Project[]> {
    return invoke<Project[]>('list_projects');
  },
  /** Creates an app that uses a saved connection and one of its models. */
  createProject(name: string, connection: string, model: string): Promise<Project> {
    return invoke<Project>('create_project', { name, connection, model });
  },
  /** Moves an app to another connection and model. */
  setProjectService(id: string, connection: string, model: string): Promise<void> {
    return invoke<void>('set_project_service', { id, connection, model });
  },
  renameProject(id: string, name: string): Promise<void> {
    return invoke<void>('rename_project', { id, name });
  },
  resetConversation(id: string): Promise<void> {
    return invoke<void>('reset_conversation', { id });
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
  /** Imports the zip as a new app. `service` is for app files alone; without it they use the last new app's. */
  importArchive(path: string, withConversation: boolean, service: ServiceChoice | null = null): Promise<Project> {
    return invoke<Project>('import_archive', { path, withConversation, service });
  },
  /** Puts a shortcut on the desktop that opens Transmogrifier at this app. Resolves to the shortcut's path. */
  createShortcut(id: string): Promise<string> {
    return invoke<string>('create_shortcut', { id });
  },
  /** The project the shortcut that started Transmogrifier asked for, or null. Answers once. */
  takeLaunchRequest(): Promise<string | null> {
    return invoke<string | null>('take_launch_request');
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
  /** A shortcut opened while Transmogrifier was running asked for a project. */
  onLaunch(handler: (projectId: string) => void): Promise<Unsubscribe> {
    return subscribe<{ projectId: string }>('launch', (payload) => handler(payload.projectId));
  },
};

export type Api = typeof api;
