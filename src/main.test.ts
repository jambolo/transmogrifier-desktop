// Behavioural tests for the UI against a mocked backend. The views are
// DOM-driven glue, so these run in jsdom with `./api` replaced by a mock.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ConsoleView, GenerationProgress, Project, ProjectView, RunnerStatus, Settings } from './api';

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
  setApiKey: vi.fn(),
  testApiKey: vi.fn(),
  listProjects: vi.fn(),
  createProject: vi.fn(),
  renameProject: vi.fn(),
  deleteProject: vi.fn(),
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

vi.mock('./api', () => ({
  api: mockApi,
  PROVIDERS: ['ANTHROPIC', 'OPENAI'],
  EFFORTS: ['LOW', 'MEDIUM', 'HIGH', 'XHIGH', 'MAX'],
}));
vi.mock('./styles.css', () => ({}));

const settings = (keyHints: Settings['keyHints'] = { ANTHROPIC: 'abcd' }): Settings => ({
  defaultProvider: 'ANTHROPIC',
  defaultModels: {},
  effort: 'HIGH',
  maxToolIterations: 30,
  maxOutputTokens: 64_000,
  appInstructions: '',
  allowAppNetwork: false,
  keyHints,
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
  it('opens Settings on first launch, when no key is saved', async () => {
    mockApi.getSettings.mockResolvedValue(settings({}));
    await boot();
    expect(app().querySelector('h1')?.textContent).toBe('Settings');
    expect(app().textContent).toContain('No key saved');
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

  it('creates an app from the name dialog and opens it', async () => {
    mockApi.createProject.mockResolvedValue(project);
    mockApi.getProject.mockResolvedValue(view());
    await boot();
    click('New app');
    const dialog = document.querySelector('dialog')!;
    (dialog.querySelector('input') as HTMLInputElement).value = '  Tip jar ';
    click('New app', dialog);
    await flush();
    expect(mockApi.createProject).toHaveBeenCalledWith('Tip jar');
    expect(app().querySelector('h1')?.textContent).toBe('Tip jar');
    expect(app().textContent).toContain('Claude · claude-opus-5-5');
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

  it('asks whether to keep a backup’s conversation when importing', async () => {
    mockApi.pickArchive.mockResolvedValue({ path: '/tmp/a.zip', name: 'Tip jar', hasConversation: true });
    mockApi.importArchive.mockResolvedValue(project);
    mockApi.getProject.mockResolvedValue(view());
    await boot();
    click('Import app');
    await flush();
    click('Without', document.querySelector('dialog')!);
    await flush();
    expect(mockApi.importArchive).toHaveBeenCalledWith('/tmp/a.zip', false);
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
  it('offers examples for a first request and sends what the user writes', async () => {
    await openWorkspace(view());
    expect(buttonNamed('Open app')?.disabled).toBe(true);
    expect(buttonNamed('Make app')?.disabled).toBe(true);
    click('Game night');
    const draft = app().querySelector('textarea')!;
    expect(draft.value).toBe('A scorekeeper for two teams, with big plus and minus buttons.');
    click('Make app');
    expect(mockApi.generate).toHaveBeenCalledWith(
      'p1',
      draft.value || 'A scorekeeper for two teams, with big plus and minus buttons.',
      false,
    );
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
    mockApi.setApiKey.mockResolvedValue(settings({ OPENAI: '1234' }));
    mockApi.testApiKey.mockResolvedValue({ result: 'passed', models: [{ id: 'gpt-6.1-sol', displayName: 'gpt-6.1-sol' }] });
    await boot();
    const cards = app().querySelectorAll('.card');
    (cards[1].querySelector('input') as HTMLInputElement).value = ' sk-test-1234 ';
    click('Save', cards[1]);
    await flush();
    expect(mockApi.setApiKey).toHaveBeenCalledWith('OPENAI', 'sk-test-1234');
    expect(app().textContent).toContain('Saved key ending in …1234');
    mockApi.getSettings.mockResolvedValue(settings({ OPENAI: '1234' }));
    click('Check connection');
    await flush();
    expect(app().querySelector('.success')?.textContent).toBe('Connected. Your key works with 1 model.');
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
