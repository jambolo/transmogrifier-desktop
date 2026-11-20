// Behavioural tests for the UI against a mocked backend. The views are
// DOM-driven glue, so these run in jsdom with `./api` replaced by a mock.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Connection, ConsoleView, GenerationProgress, Project, ProjectView, RunnerStatus, Settings } from './api';

type Handler<T> = (payload: T) => void;

const events = vi.hoisted(() => ({
  generation: undefined as Handler<GenerationProgress> | undefined,
  finished: undefined as Handler<string> | undefined,
  runner: undefined as Handler<RunnerStatus> | undefined,
  link: undefined as Handler<string> | undefined,
  launch: undefined as Handler<string> | undefined,
}));

const mockApi = vi.hoisted(() => ({
  getSettings: vi.fn(),
  updateSettings: vi.fn(),
  addConnection: vi.fn(),
  editConnection: vi.fn(),
  removeConnection: vi.fn(),
  setApiKey: vi.fn(),
  testApiKey: vi.fn(),
  listProjects: vi.fn(),
  createProject: vi.fn(),
  setProjectService: vi.fn(),
  renameProject: vi.fn(),
  deleteProject: vi.fn(),
  resetConversation: vi.fn(),
  getProject: vi.fn(),
  readProjectFile: vi.fn(),
  generate: vi.fn(),
  stopGeneration: vi.fn(),
  runApp: vi.fn(),
  reloadApp: vi.fn(),
  closeApp: vi.fn(),
  getConsole: vi.fn(),
  exportProject: vi.fn(),
  pickArchive: vi.fn(),
  importArchive: vi.fn(),
  openExternal: vi.fn(),
  createShortcut: vi.fn(),
  takeLaunchRequest: vi.fn(),
  onGeneration: vi.fn(),
  onGenerationFinished: vi.fn(),
  onRunner: vi.fn(),
  onExternalLink: vi.fn(),
  onLaunch: vi.fn(),
}));

vi.mock('./api', async (importOriginal) => ({ ...(await importOriginal<typeof import('./api')>()), api: mockApi }));
vi.mock('./styles.css', () => ({}));

/** Settings with a key saved for each built-in connection named in `keyHints`, and the given servers. */
const settings = (
  keyHints: Record<string, string> = { anthropic: 'abcd' },
  servers: Connection[] = [],
  overrides: Partial<Settings> = {},
): Settings => ({
  connections: [
    ...(['ANTHROPIC', 'OPENAI', 'GEMINI'] as const).map((protocol): Connection => {
      const id = protocol.toLowerCase();
      return { id, name: '', protocol, model: '', ...(id in keyHints ? { keyHint: keyHints[id] } : {}) };
    }),
    ...servers,
  ],
  lastConnection: 'anthropic',
  effort: 'HIGH',
  maxToolIterations: 30,
  maxOutputTokens: 64_000,
  appInstructions: '',
  allowAppNetwork: false,
  ...overrides,
});

const LM_STUDIO = 'http://localhost:1234/v1';

/** A server the user added. */
const server = (overrides: Partial<Connection> = {}): Connection => ({
  id: 'c1',
  name: 'LM Studio',
  protocol: 'OPENAI_COMPATIBLE',
  endpoint: LM_STUDIO,
  model: '',
  ...overrides,
});

const project: Project = {
  id: 'p1',
  name: 'Tip jar',
  provider: 'ANTHROPIC',
  model: 'claude-opus-5-5',
  createdAtMillis: 0,
  updatedAtMillis: Date.now(),
};

const runner = (overrides: Partial<RunnerStatus> = {}): RunnerStatus => ({
  projectId: 'p1',
  running: false,
  errorCount: 0,
  networkBlocked: false,
  ...overrides,
});

const view = (overrides: Partial<ProjectView> = {}): ProjectView => ({
  project,
  transcript: [],
  files: [],
  generating: false,
  runner: runner(),
  ...overrides,
});

const noConsole: ConsoleView = { messages: [], errorLog: null, networkBlocked: false, running: false };

/** Let all pending promise chains settle. */
const flush = async () => {
  for (let i = 0; i < 5; i++) await new Promise<void>((resolve) => setTimeout(resolve, 0));
};

/** Build the DOM main.ts expects, then (re-)import it fresh. */
async function boot(): Promise<void> {
  document.body.innerHTML = `
    <button id="home"></button>
    <button id="open-settings"></button>
    <main id="app"></main>
  `;
  vi.resetModules();
  await import('./main');
  await flush();
}

const app = () => document.getElementById('app')!;
const buttonNamed = (name: string, within: ParentNode = document) =>
  [...within.querySelectorAll('button')].find(
    (button) => button.textContent?.trim() === name || button.getAttribute('aria-label') === name,
  );
/** The settings card for a connection, or the one for adding a server (`add`). */
const card = (id: string) => app().querySelector(`[data-connection="${id}"]`)!.closest('.card')!;
/** Opens or closes a settings card. */
const toggle = (id: string) => (app().querySelector(`[data-connection="${id}"]`) as HTMLButtonElement).click();
const click = (name: string, within?: ParentNode) => {
  const button = buttonNamed(name, within);
  if (!button) throw new Error(`No button named ${name}`);
  button.click();
};

async function openWorkspace(initial: ProjectView): Promise<void> {
  mockApi.getProject.mockResolvedValue(initial);
  await boot();
  (app().querySelector('.app-open') as HTMLButtonElement).click();
  await flush();
}

beforeEach(() => {
  vi.clearAllMocks();
  mockApi.getSettings.mockResolvedValue(settings());
  mockApi.listProjects.mockResolvedValue([project]);
  mockApi.getConsole.mockResolvedValue(noConsole);
  mockApi.testApiKey.mockResolvedValue({ result: 'passed', models: [] });
  mockApi.generate.mockResolvedValue(undefined);
  mockApi.reloadApp.mockResolvedValue(undefined);
  mockApi.runApp.mockResolvedValue(undefined);
  mockApi.onGeneration.mockImplementation(async (handler: Handler<GenerationProgress>) => {
    events.generation = handler;
  });
  mockApi.onGenerationFinished.mockImplementation(async (handler: Handler<string>) => {
    events.finished = handler;
  });
  mockApi.onRunner.mockImplementation(async (handler: Handler<RunnerStatus>) => {
    events.runner = handler;
  });
  mockApi.onExternalLink.mockImplementation(async (handler: Handler<string>) => {
    events.link = handler;
  });
  mockApi.onLaunch.mockImplementation(async (handler: Handler<string>) => {
    events.launch = handler;
  });
  mockApi.takeLaunchRequest.mockResolvedValue(null);
});

describe('start', () => {
  it('opens Settings on first launch, when no service is set up', async () => {
    mockApi.getSettings.mockResolvedValue(settings({}));
    await boot();
    expect(app().querySelector('h1')?.textContent).toBe('Settings');
    expect(app().textContent).toContain('Not set up');
  });

  it('lists the apps once a key is saved', async () => {
    await boot();
    expect(app().querySelector('h1')?.textContent).toBe('Your apps');
    expect(app().querySelector('.app-name')?.textContent).toBe('Tip jar');
    expect(app().querySelector('.app-meta')?.textContent).toBe('Edited just now · Claude');
  });

  it('opens the app a shortcut asked for', async () => {
    mockApi.takeLaunchRequest.mockResolvedValue('p1');
    mockApi.getProject.mockResolvedValue(view({ files: [{ path: 'index.html', sizeBytes: 10 }] }));
    await boot();
    expect(app().querySelector('h1')?.textContent).toBe('Tip jar');
    expect(mockApi.runApp).toHaveBeenCalledWith('p1');
  });

  it('opens the chat when a shortcut’s app has no files yet', async () => {
    mockApi.takeLaunchRequest.mockResolvedValue('p1');
    mockApi.getProject.mockResolvedValue(view());
    await boot();
    expect(app().querySelector('h1')?.textContent).toBe('Tip jar');
    expect(mockApi.runApp).not.toHaveBeenCalled();
  });

  it('lists the apps with an error when a shortcut’s app is gone', async () => {
    mockApi.takeLaunchRequest.mockResolvedValue('p9');
    mockApi.getProject.mockRejectedValue('This app no longer exists');
    await boot();
    expect(app().querySelector('h1')?.textContent).toBe('Your apps');
    expect(document.querySelector('.toast.error')?.textContent).toContain('The app this shortcut opens no longer exists.');
  });

  it('opens the app a shortcut asks for while running', async () => {
    mockApi.getProject.mockResolvedValue(view({ files: [{ path: 'index.html', sizeBytes: 10 }] }));
    await boot();
    events.launch!('p1');
    await flush();
    expect(app().querySelector('h1')?.textContent).toBe('Tip jar');
    expect(mockApi.runApp).toHaveBeenCalledWith('p1');
  });

  it('invites a first app when there are none', async () => {
    mockApi.listProjects.mockResolvedValue([]);
    await boot();
    expect(app().textContent).toContain('What needs a little app?');
  });
});

describe('projects', () => {
  it('shows the app version in the footer', async () => {
    await boot();
    expect(app().querySelector('footer')?.textContent).toBe(`Version ${__APP_VERSION__}`);
  });

  it('opens the chat and runs the app when a generated app is clicked', async () => {
    await openWorkspace(view({ files: [{ path: 'index.html', sizeBytes: 10 }] }));
    expect(app().querySelector('h1')?.textContent).toBe('Tip jar');
    expect(mockApi.runApp).toHaveBeenCalledWith('p1');
  });

  it('opens only the chat when the clicked app has no files yet', async () => {
    await openWorkspace(view());
    expect(app().querySelector('h1')?.textContent).toBe('Tip jar');
    expect(mockApi.runApp).not.toHaveBeenCalled();
  });

  it('creates an app from the name dialog and opens it', async () => {
    mockApi.createProject.mockResolvedValue(project);
    mockApi.getProject.mockResolvedValue(view());
    await boot();
    click('New app');
    await flush();
    const dialog = document.querySelector('dialog')!;
    (dialog.querySelector('input') as HTMLInputElement).value = '  Tip jar ';
    click('New app', dialog);
    await flush();
    // With one service ready there is nothing to choose, and its model is filled in.
    expect(dialog.querySelector('select')).toBeNull();
    expect(mockApi.createProject).toHaveBeenCalledWith('Tip jar', 'anthropic', 'claude-opus-5-5');
    expect(app().querySelector('h1')?.textContent).toBe('Tip jar');
    expect(app().textContent).toContain('Claude · claude-opus-5-5');
  });

  it('asks which service and model make a new app when several are ready', async () => {
    const saved = settings({ anthropic: 'abcd' }, [server({ model: 'gemma4' })], { lastConnection: 'c1' });
    mockApi.getSettings.mockResolvedValue(saved);
    mockApi.testApiKey.mockResolvedValue({ result: 'passed', models: [{ id: 'qwen3-coder', displayName: 'qwen3-coder' }] });
    mockApi.createProject.mockResolvedValue(project);
    mockApi.getProject.mockResolvedValue(view());
    await boot();
    click('New app');
    await flush();
    const dialog = document.querySelector('dialog')!;
    const service = dialog.querySelector('select')!;
    const model = dialog.querySelector('input[list]') as HTMLInputElement;
    // The dialog starts with the service the last new app used, and offers the models it has.
    expect([...service.options].map((option) => option.textContent)).toEqual(['Claude', 'LM Studio']);
    expect([service.value, model.value]).toEqual(['c1', 'gemma4']);
    expect([...dialog.querySelectorAll('datalist option')].map((option) => option.getAttribute('value'))).toEqual(['qwen3-coder']);
    service.value = 'anthropic';
    service.dispatchEvent(new Event('change'));
    expect(model.value).toBe('claude-opus-5-5');
    model.value = ' claude-haiku-5-5 ';
    (dialog.querySelector('input') as HTMLInputElement).value = 'Tip jar';
    click('New app', dialog);
    await flush();
    expect(mockApi.createProject).toHaveBeenCalledWith('Tip jar', 'anthropic', 'claude-haiku-5-5');
  });

  it('asks for a service to be set up before making an app', async () => {
    mockApi.getSettings.mockResolvedValue(settings({}));
    await boot();
    document.getElementById('home')!.click();
    await flush();
    click('New app');
    await flush();
    expect(document.querySelector('dialog')).toBeNull();
    expect(document.querySelector('.toast.error')?.textContent).toContain('Connect an AI service in Settings first.');
  });

  it('names the server an app uses, and moves the app to another service', async () => {
    const local: Project = { ...project, provider: 'OPENAI_COMPATIBLE', connection: 'c1', model: 'gemma4' };
    mockApi.getSettings.mockResolvedValue(settings({ anthropic: 'abcd' }, [server({ model: 'gemma4' })]));
    mockApi.listProjects.mockResolvedValue([local]);
    mockApi.setProjectService.mockResolvedValue(undefined);
    await boot();
    expect(app().querySelector('.app-meta')?.textContent).toBe('Edited just now · LM Studio');
    click('More options for Tip jar');
    click('Change AI service');
    await flush();
    const dialog = document.querySelector('dialog')!;
    const service = dialog.querySelector('select')!;
    const model = dialog.querySelector('input[list]') as HTMLInputElement;
    // The dialog starts with what the app uses now.
    expect([service.value, model.value]).toEqual(['c1', 'gemma4']);
    expect(mockApi.setProjectService).not.toHaveBeenCalled();
    service.value = 'anthropic';
    service.dispatchEvent(new Event('change'));
    click('Change AI service', dialog);
    await flush();
    expect(mockApi.setProjectService).toHaveBeenCalledWith('p1', 'anthropic', 'claude-opus-5-5');
    expect(document.querySelector('.toast')?.textContent).toContain('AI service changed.');
  });

  it('adds a desktop shortcut from the menu', async () => {
    mockApi.createShortcut.mockResolvedValue('/home/me/Desktop/transmogrifier-p1.desktop');
    await boot();
    click('More options for Tip jar');
    click('Add desktop shortcut');
    await flush();
    expect(mockApi.createShortcut).toHaveBeenCalledWith('p1');
    expect(document.querySelector('.toast')?.textContent).toContain('Added a shortcut to your desktop.');
  });

  it('deletes an app only after confirmation', async () => {
    mockApi.deleteProject.mockResolvedValue(undefined);
    await boot();
    click('More options for Tip jar');
    click('Delete app');
    const dialog = document.querySelector('dialog')!;
    expect(dialog.textContent).toContain('Delete “Tip jar”?');
    expect(mockApi.deleteProject).not.toHaveBeenCalled();
    click('Delete app', dialog);
    await flush();
    expect(mockApi.deleteProject).toHaveBeenCalledWith('p1');
  });

  it('resets a conversation only after confirmation', async () => {
    mockApi.resetConversation.mockResolvedValue(undefined);
    await boot();
    click('More options for Tip jar');
    click('Reset conversation');
    const dialog = document.querySelector('dialog')!;
    expect(dialog.textContent).toContain('Reset the conversation for “Tip jar”?');
    expect(mockApi.resetConversation).not.toHaveBeenCalled();
    click('Reset conversation', dialog);
    await flush();
    expect(mockApi.resetConversation).toHaveBeenCalledWith('p1');
  });

  it('asks whether to keep a backup’s conversation when importing', async () => {
    mockApi.pickArchive.mockResolvedValue({ path: '/tmp/a.zip', name: 'Tip jar', hasProject: true, hasConversation: true });
    mockApi.importArchive.mockResolvedValue(project);
    mockApi.getProject.mockResolvedValue(view());
    await boot();
    click('Import app');
    await flush();
    click('Without', document.querySelector('dialog')!);
    await flush();
    // A backup keeps the service that made it.
    expect(mockApi.importArchive).toHaveBeenCalledWith('/tmp/a.zip', false, null);
  });

  it('asks which service changes imported app files when several are ready', async () => {
    mockApi.getSettings.mockResolvedValue(settings({ anthropic: 'abcd' }, [server({ model: 'gemma4' })]));
    mockApi.pickArchive.mockResolvedValue({ path: '/tmp/site.zip', name: 'My site', hasProject: false, hasConversation: false });
    mockApi.importArchive.mockResolvedValue(project);
    mockApi.getProject.mockResolvedValue(view());
    await boot();
    click('Import app');
    await flush();
    const dialog = document.querySelector('dialog')!;
    expect(dialog.textContent).toContain('Which AI service should change “My site”?');
    expect(mockApi.importArchive).not.toHaveBeenCalled();
    const service = dialog.querySelector('select')!;
    service.value = 'c1';
    service.dispatchEvent(new Event('change'));
    click('Import app', dialog);
    await flush();
    expect(mockApi.importArchive).toHaveBeenCalledWith('/tmp/site.zip', false, { connection: 'c1', model: 'gemma4' });
  });

  it('imports app files without asking when one service is ready', async () => {
    mockApi.pickArchive.mockResolvedValue({ path: '/tmp/site.zip', name: 'My site', hasProject: false, hasConversation: false });
    mockApi.importArchive.mockResolvedValue(project);
    mockApi.getProject.mockResolvedValue(view());
    await boot();
    click('Import app');
    await flush();
    expect(document.querySelector('dialog')).toBeNull();
    expect(mockApi.importArchive).toHaveBeenCalledWith('/tmp/site.zip', false, null);
  });

  it('shows why an import failed', async () => {
    mockApi.pickArchive.mockRejectedValue('The zip has no index.html');
    await boot();
    click('Import app');
    await flush();
    expect(document.querySelector('.toast')?.textContent).toContain("Couldn't import the app: The zip has no index.html");
  });
});

describe('workspace', () => {
  it('invites a first request and sends what the user writes', async () => {
    await openWorkspace(view());
    expect(app().querySelector('.chat-empty')?.textContent).toBe(
      'Describe a little app you would like to use. You can ask for changes as you go.',
    );
    expect(buttonNamed('Open app')?.disabled).toBe(true);
    expect(buttonNamed('Make app')?.disabled).toBe(true);
    const draft = app().querySelector('textarea')!;
    draft.value = 'A scorekeeper for two teams';
    draft.dispatchEvent(new Event('input'));
    click('Make app');
    expect(mockApi.generate).toHaveBeenCalledWith('p1', 'A scorekeeper for two teams', false);
    expect(app().querySelector('.entry.prompt')?.textContent).toContain('A scorekeeper for two teams');
    expect(app().querySelector('.progress')?.textContent).toBe('Making your app…');
    expect(buttonNamed('Stop')).toBeDefined();
  });

  it('streams progress, then shows the saved transcript and files', async () => {
    await openWorkspace(view());
    app().querySelector('textarea')!.value = 'A tip calculator';
    app().querySelector('textarea')!.dispatchEvent(new Event('input'));
    click('Make app');
    events.generation!({ projectId: 'p1', event: { type: 'textDelta', text: "I'll create the page." } });
    events.generation!({ projectId: 'p1', event: { type: 'toolRunning', name: 'write_file', path: 'index.html' } });
    expect(app().querySelector('.live')?.textContent).toBe("I'll create the page.Writing index.html…");
    // Progress for another app is ignored.
    events.generation!({ projectId: 'other', event: { type: 'textDelta', text: 'nope' } });
    expect(app().textContent).not.toContain('nope');

    mockApi.getProject.mockResolvedValue(
      view({
        transcript: [
          { type: 'prompt', text: 'A tip calculator' },
          { type: 'reply', text: 'Done.' },
          { type: 'tool', name: 'write_file', path: 'index.html' },
          { type: 'usage', inputTokens: 1200, outputTokens: 80, cacheReadTokens: 0 },
        ],
        files: [{ path: 'index.html', sizeBytes: 2048 }],
      }),
    );
    events.finished!('p1');
    await flush();
    expect(app().querySelector('.entry.tool')?.textContent).toBe('Wrote index.html');
    expect(app().querySelector('.tab')?.textContent).toBe('Files (1)');
    expect(app().querySelector('.file')?.textContent).toBe('index.html2.0 KB');
    expect(buttonNamed('Open app')?.disabled).toBe(false);
    expect(buttonNamed('Send')).toBeDefined();
  });

  it('offers Continue after a stop and resumes the pending request', async () => {
    await openWorkspace(
      view({
        transcript: [
          { type: 'prompt', text: 'A timer' },
          { type: 'notice', kind: 'STOPPED' },
        ],
      }),
    );
    expect(app().querySelector('.notice')?.textContent).toBe("Stopped. You can continue when you're ready.");
    click('Continue');
    expect(mockApi.generate).toHaveBeenCalledWith('p1', null, false);
  });

  it('sends a run’s errors to be fixed', async () => {
    mockApi.getConsole.mockResolvedValue({
      messages: [{ level: 'ERROR', message: 'total is not defined', source: 'js/app.js', line: 9, stack: null }],
      errorLog: 'total is not defined (js/app.js:9)',
      networkBlocked: false,
      running: true,
    });
    await openWorkspace(view({ files: [{ path: 'index.html', sizeBytes: 10 }], runner: runner({ running: true, errorCount: 1 }) }));
    expect(buttonNamed('Show app')).toBeDefined();
    expect(app().querySelector('.badge')?.textContent).toBe('1');
    (app().querySelectorAll('.tab')[1] as HTMLButtonElement).click();
    expect(app().querySelector('.console-line')?.textContent).toBe('total is not definedjs/app.js:9');
    click('Try to fix it');
    expect(mockApi.generate).toHaveBeenCalledWith('p1', 'total is not defined (js/app.js:9)', true);
    // An open app reloads when the fix is done.
    events.finished!('p1');
    await flush();
    expect(mockApi.reloadApp).toHaveBeenCalledWith('p1');
  });

  it('offers another service when the app’s own is not set up here', async () => {
    const orphan: Project = { ...project, provider: 'OPENAI_COMPATIBLE', connection: 'gone', model: 'gemma4' };
    mockApi.listProjects.mockResolvedValue([orphan]);
    mockApi.setProjectService.mockResolvedValue(undefined);
    await openWorkspace(
      view({
        project: orphan,
        transcript: [
          { type: 'prompt', text: 'A timer' },
          { type: 'notice', kind: 'PROVIDER_UNAVAILABLE' },
        ],
      }),
    );
    expect(app().querySelector('.workspace-title p')?.textContent).toBe('AI service not set up');
    expect(app().querySelector('.notice.error')?.textContent).toContain("This app's AI service isn't set up on this computer.");
    mockApi.getProject.mockResolvedValue(view({ project: { ...project, model: 'claude-opus-5-5' } }));
    click('Choose AI service');
    await flush();
    click('Change AI service', document.querySelector('dialog')!);
    await flush();
    expect(mockApi.setProjectService).toHaveBeenCalledWith('p1', 'anthropic', 'claude-opus-5-5');
    expect(app().querySelector('.workspace-title p')?.textContent).toBe('Claude · claude-opus-5-5');
  });

  it('shows a file read-only when it is picked', async () => {
    mockApi.readProjectFile.mockResolvedValue('<h1>Tips</h1>');
    await openWorkspace(view({ files: [{ path: 'index.html', sizeBytes: 13 }] }));
    (app().querySelector('.file') as HTMLButtonElement).click();
    await flush();
    expect(app().querySelector('.code')?.textContent).toBe('<h1>Tips</h1>');
  });

  it('asks before opening a link from a running app', async () => {
    mockApi.openExternal.mockResolvedValue(undefined);
    await boot();
    events.link!('https://example.com/');
    const dialog = document.querySelector('dialog')!;
    expect(dialog.textContent).toContain('https://example.com/');
    expect(mockApi.openExternal).not.toHaveBeenCalled();
    click('Open', dialog);
    await flush();
    expect(mockApi.openExternal).toHaveBeenCalledWith('https://example.com/');
  });
});

describe('settings', () => {
  it('saves a key and reports the connection check', async () => {
    mockApi.getSettings.mockResolvedValue(settings({}));
    mockApi.setApiKey.mockResolvedValue(settings({ openai: '1234' }));
    mockApi.testApiKey.mockResolvedValue({ result: 'passed', models: [{ id: 'gpt-6.1-sol', displayName: 'gpt-6.1-sol' }] });
    await boot();
    // Nothing is set up yet, so every card starts closed.
    expect(app().querySelectorAll('.card.open')).toHaveLength(0);
    expect(card('openai').textContent).toContain('Not set up');
    toggle('openai');
    (card('openai').querySelector('input') as HTMLInputElement).value = ' sk-test-1234 ';
    click('Save', card('openai'));
    await flush();
    expect(mockApi.setApiKey).toHaveBeenCalledWith('openai', 'sk-test-1234');
    expect(card('openai').textContent).toContain('Saved key ending in …1234');
    expect(card('openai').textContent).toContain('Ready');
    mockApi.getSettings.mockResolvedValue(settings({ openai: '1234' }));
    click('Check connection');
    await flush();
    expect(mockApi.testApiKey).toHaveBeenCalledWith('openai');
    expect(app().querySelector('.success')?.textContent).toBe('Connected. Your key works with 1 model.');
    // The model for new apps is chosen from what the check found.
    expect(card('openai').querySelector('select')?.value).toBe('gpt-6.1-sol');
  });

  it('shows the services each connection covers', async () => {
    mockApi.getSettings.mockResolvedValue(settings({ anthropic: 'abcd' }, [server({ model: 'gemma4' })]));
    await boot();
    document.getElementById('open-settings')!.click();
    await flush();
    // The services that are set up start open; the rest are closed.
    expect([...app().querySelectorAll('.card.open')]).toEqual([card('anthropic'), card('c1')]);
    expect(card('anthropic').textContent).toContain('Works with Anthropic API');
    expect(card('gemini').textContent).toContain('Works with Google AI Studio (Gemini API)');
    expect(card('c1').querySelector('.provider-name')?.textContent).toBe('LM Studio');
    expect(card('c1').textContent).toContain('Ready');
    toggle('c1');
    // A closed server shows where it is.
    expect(card('c1').querySelector('.help')?.textContent).toBe(LM_STUDIO);
    toggle('add');
    const chips = [...card('add').querySelectorAll('.chip')].map((chip) => chip.textContent);
    expect(chips).toEqual(expect.arrayContaining(['OpenRouter', 'Groq', 'Cerebras', 'Ollama', 'LM Studio', 'Jan']));
    toggle('anthropic');
    expect([...app().querySelectorAll('.card.open')]).toEqual([card('add')]);
  });

  it('adds a local server with an address, no key, and a model', async () => {
    mockApi.getSettings.mockResolvedValue(settings({}));
    mockApi.addConnection.mockResolvedValue(settings({}, [server()]));
    mockApi.editConnection.mockResolvedValue(settings({}, [server({ model: 'qwen3-coder' })]));
    mockApi.testApiKey.mockResolvedValue({
      result: 'passed',
      models: [
        { id: 'gemma4', displayName: 'gemma4' },
        { id: 'qwen3-coder', displayName: 'qwen3-coder' },
      ],
    });
    await boot();
    toggle('add');
    // Choosing a server fills in its name and usual address.
    click('LM Studio', card('add'));
    const [name, address] = [...card('add').querySelectorAll('input')];
    expect([name.value, address.value]).toEqual(['LM Studio', LM_STUDIO]);
    click('Add', card('add'));
    await flush();
    expect(mockApi.addConnection).toHaveBeenCalledWith('LM Studio', LM_STUDIO);
    // The new server's card opens for the rest of its setup.
    expect(card('add').classList.contains('open')).toBe(false);
    expect(card('c1').classList.contains('open')).toBe(true);
    expect(card('c1').textContent).toContain('Needs a model');
    expect(card('c1').textContent).toContain("No key saved. A local server usually doesn't need one.");
    mockApi.getSettings.mockResolvedValue(settings({}, [server()]));
    click('Check connection', card('c1'));
    await flush();
    expect(mockApi.testApiKey).toHaveBeenCalledWith('c1');
    expect(app().querySelector('.success')?.textContent).toBe('Connected. The server has 2 models.');
    const model = card('c1').querySelector('select') as HTMLSelectElement;
    expect(model.value).toBe('');
    model.value = 'qwen3-coder';
    model.dispatchEvent(new Event('change'));
    await flush();
    expect(mockApi.editConnection).toHaveBeenCalledWith('c1', { model: 'qwen3-coder' });
    expect(card('c1').textContent).toContain('Ready');
    expect(mockApi.setApiKey).not.toHaveBeenCalled();
  });

  it('keeps several servers, each with its own address, and renames or removes one', async () => {
    const ollama = server({ id: 'c2', name: 'Ollama', endpoint: 'http://localhost:11434/v1', model: 'gemma4' });
    mockApi.getSettings.mockResolvedValue(settings({}, [server(), ollama]));
    mockApi.editConnection.mockResolvedValue(settings({}, [server({ name: 'Studio' }), ollama]));
    mockApi.removeConnection.mockResolvedValue(settings({}, [server({ name: 'Studio' })]));
    await boot();
    document.getElementById('open-settings')!.click();
    await flush();
    expect((card('c1').querySelector('input') as HTMLInputElement).value).toBe(LM_STUDIO);
    expect((card('c2').querySelector('input') as HTMLInputElement).value).toBe('http://localhost:11434/v1');

    click('Rename', card('c1'));
    let dialog = document.querySelector('dialog')!;
    (dialog.querySelector('input') as HTMLInputElement).value = 'Studio';
    click('Save', dialog);
    await flush();
    expect(mockApi.editConnection).toHaveBeenCalledWith('c1', { name: 'Studio' });
    expect(card('c1').querySelector('.provider-name')?.textContent).toBe('Studio');

    click('Remove', card('c2'));
    dialog = document.querySelector('dialog')!;
    expect(dialog.textContent).toContain('Remove “Ollama”?');
    expect(mockApi.removeConnection).not.toHaveBeenCalled();
    click('Remove', dialog);
    await flush();
    expect(mockApi.removeConnection).toHaveBeenCalledWith('c2');
    expect(app().querySelector('[data-connection="c2"]')).toBeNull();
    // The built-in services can't be renamed or removed.
    toggle('anthropic');
    expect(buttonNamed('Remove', card('anthropic'))).toBeUndefined();
  });

  it('explains a failed connection check', async () => {
    mockApi.testApiKey.mockResolvedValue({
      result: 'failed',
      error: { kind: 'INVALID_KEY', network: null, status: 401, detail: 'invalid x-api-key' },
    });
    await boot();
    document.getElementById('open-settings')!.click();
    await flush();
    click('Check connection');
    await flush();
    expect(app().querySelector('.error')?.textContent).toContain("Claude didn't recognize this key.");
  });

  it('saves advanced changes within their limits', async () => {
    mockApi.updateSettings.mockImplementation(async (next: Settings) => next);
    await boot();
    document.getElementById('open-settings')!.click();
    await flush();
    click('Advanced settings');
    const steps = app().querySelector('input[type="number"]') as HTMLInputElement;
    steps.value = '500';
    steps.dispatchEvent(new Event('change'));
    await flush();
    expect(mockApi.updateSettings).toHaveBeenCalledWith(expect.objectContaining({ maxToolIterations: 200 }));
    const network = app().querySelector('input[type="checkbox"]') as HTMLInputElement;
    network.checked = true;
    network.dispatchEvent(new Event('change'));
    await flush();
    expect(mockApi.updateSettings).toHaveBeenLastCalledWith(expect.objectContaining({ allowAppNetwork: true }));
  });
});
