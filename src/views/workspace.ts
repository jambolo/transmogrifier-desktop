// One app: the chat that makes and changes it, its files, and the controls for its window.

import { connectionFor } from '../api';
import type { Api, ConsoleView, GenerationProgress, ProjectView, RunnerStatus, Settings, TranscriptEntry } from '../api';
import { button, errorMessage, h, icon, replace, toast } from '../dom';
import { connectionLabel, noticeIsError, noticeText, retryingLabel, text, toolLabel, usageLabel } from '../strings';
import { changeService } from './service';

export interface WorkspaceHandlers {
  onBack: () => void;
  onOpenSettings: () => void;
}

/** The in-progress part of a generation that is not yet in the transcript. */
interface LiveTurn {
  text: string;
  tool: { name: string; path: string | null } | null;
  retry: { delaySeconds: number; reason: string } | null;
}

const NO_LIVE: LiveTurn = { text: '', tool: null, retry: null };
const NO_CONSOLE: ConsoleView = { messages: [], errorLog: null, networkBlocked: false, running: false };

function formatBytes(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KB`;
}

export function createWorkspaceView(api: Api, handlers: WorkspaceHandlers) {
  let view: ProjectView | null = null;
  let settings: Settings | null = null;
  let live: LiveTurn = NO_LIVE;
  let consoleView: ConsoleView = NO_CONSOLE;
  let sideTab: 'files' | 'errors' = 'files';
  let openFile: { path: string; content: string | null } | null = null;

  const title = h('div', { className: 'workspace-title' });
  const barActions = h('div', { className: 'actions' });
  const transcript = h('div', { className: 'transcript', attrs: { 'aria-live': 'polite' } });
  const fixBar = h('div', { className: 'fix-bar' });
  const promptLabel = h('span', { className: 'field-label' });
  const draft = h('textarea', { rows: 2, placeholder: text.promptHint });
  const sendSlot = h('div', { className: 'send-slot' });
  const sideTabs = h('div', { className: 'tabs', attrs: { role: 'tablist' } });
  const sidePanel = h('div', { className: 'side-panel' });

  const root = h(
    'section',
    { className: 'view workspace' },
    h(
      'header',
      { className: 'workspace-bar' },
      button({ label: text.back, icon: 'arrowLeft', kind: 'quiet', iconOnly: true, onClick: handlers.onBack }),
      title,
      barActions,
    ),
    h(
      'div',
      { className: 'workspace-body' },
      h(
        'div',
        { className: 'chat' },
        transcript,
        fixBar,
        h('div', { className: 'prompt-bar' }, h('label', { className: 'field grow' }, promptLabel, draft), sendSlot),
      ),
      h('aside', { className: 'side' }, sideTabs, sidePanel),
    ),
  );

  draft.addEventListener('input', renderPromptBar);
  draft.addEventListener('keydown', (event) => {
    // Enter sends; Shift+Enter adds a line.
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      send();
    }
  });

  const id = () => view?.project.id ?? '';
  const generating = () => view?.generating ?? false;

  async function load(projectId: string): Promise<void> {
    view = null;
    live = NO_LIVE;
    consoleView = NO_CONSOLE;
    sideTab = 'files';
    openFile = null;
    draft.value = '';
    try {
      [view, consoleView, settings] = await Promise.all([api.getProject(projectId), api.getConsole(projectId), api.getSettings()]);
    } catch (error) {
      toast(errorMessage(error), true);
      handlers.onBack();
      return;
    }
    render();
    draft.focus();
  }

  /** Reloads what a finished step changed: the transcript and the files. */
  async function refresh(): Promise<void> {
    const projectId = id();
    if (!projectId) return;
    try {
      const fresh = await api.getProject(projectId);
      if (fresh.project.id !== id()) return;
      view = fresh;
      if (openFile) await showFile(openFile.path);
      render();
    } catch {
      // The app was deleted while it was open.
      toast(text.projectMissing, true);
      handlers.onBack();
    }
  }

  function render(): void {
    renderBar();
    renderTranscript();
    renderPromptBar();
    renderSide();
  }

  function renderBar(): void {
    if (!view) return;
    const { project, runner, files } = view;
    const connection = settings && connectionFor(settings, project);
    const service = connection ? `${connectionLabel(connection)} · ${project.model}` : text.serviceMissing;
    replace(title, h('h1', {}, project.name), h('p', { className: 'muted' }, service));
    const running = runner.running;
    replace(
      barActions,
      connection ? null : button({ label: text.chooseService, icon: 'settings', onClick: () => void chooseService() }),
      running ? button({ label: text.reload, icon: 'retry', onClick: () => run(() => api.reloadApp(id())) }) : null,
      running ? button({ label: text.closeApp, icon: 'close', onClick: () => run(() => api.closeApp(id())) }) : null,
      button({
        label: running ? text.showApp : text.run,
        icon: 'play',
        kind: 'primary',
        disabled: files.length === 0 || (generating() && !running),
        onClick: () => run(() => api.runApp(id())),
      }),
    );
  }

  function renderTranscript(): void {
    if (!view) return;
    const stick = transcript.scrollHeight - transcript.scrollTop - transcript.clientHeight < 80;
    const entries = view.transcript;
    if (entries.length === 0 && !generating()) {
      replace(transcript, h('div', { className: 'chat-empty' }, h('p', {}, text.chatEmpty)));
      return;
    }
    const last = entries[entries.length - 1];
    replace(
      transcript,
      ...entries.map(transcriptItem),
      generating() ? liveEntry() : null,
      !generating() && last?.type === 'notice' ? noticeActions(last.kind) : null,
    );
    if (stick) transcript.scrollTop = transcript.scrollHeight;
  }

  function transcriptItem(entry: TranscriptEntry): HTMLElement {
    switch (entry.type) {
      case 'prompt':
        return h(
          'div',
          { className: 'entry prompt' },
          entry.fixErrors ? h('strong', {}, text.fixErrors) : null,
          h(entry.fixErrors ? 'pre' : 'p', {}, entry.text),
        );
      case 'reply':
        return h('div', { className: 'entry reply' }, h('p', {}, entry.text));
      case 'tool':
        return h(
          'div',
          { className: entry.error ? 'entry tool failed' : 'entry tool' },
          icon(entry.error ? 'close' : 'check'),
          h('span', {}, toolLabel(entry.name, entry.path, false)),
          entry.error ? h('span', { className: 'tool-error' }, entry.error) : null,
        );
      case 'usage':
        return h('div', { className: 'entry usage' }, usageLabel(entry));
      case 'notice':
        return h(
          'div',
          { className: noticeIsError(entry.kind) ? 'entry notice error' : 'entry notice' },
          noticeText(entry.kind, entry.detail),
        );
    }
  }

  function liveEntry(): HTMLElement {
    const status = live.retry
      ? retryingLabel(live.retry.delaySeconds, live.retry.reason)
      : live.tool
        ? toolLabel(live.tool.name, live.tool.path, true)
        : text.thinking;
    return h(
      'div',
      { className: 'entry reply live' },
      live.text ? h('p', {}, live.text) : null,
      live.tool || live.retry || !live.text
        ? h('div', { className: 'progress' }, h('span', { className: 'spinner' }), h('span', {}, status))
        : null,
    );
  }

  function noticeActions(kind: Extract<TranscriptEntry, { type: 'notice' }>['kind']): HTMLElement | null {
    if (kind === 'STOPPED' || kind === 'ITERATION_LIMIT' || kind === 'FAILED') {
      return h(
        'div',
        { className: 'entry' },
        button({ label: text.continueGeneration, icon: 'arrowRight', onClick: () => generate(null, false) }),
      );
    }
    if (kind === 'AUTH_FAILED') {
      return h(
        'div',
        { className: 'entry' },
        button({ label: text.openSettings, icon: 'settings', onClick: handlers.onOpenSettings }),
      );
    }
    if (kind === 'PROVIDER_UNAVAILABLE') {
      return h(
        'div',
        { className: 'entry' },
        button({ label: text.chooseService, icon: 'settings', onClick: () => void chooseService() }),
      );
    }
    return null;
  }

  /** Moves the app to a service that is set up, then shows it. */
  async function chooseService(): Promise<void> {
    if (!view || !(await changeService(api, view.project))) return;
    settings = await api.getSettings();
    await refresh();
  }

  function renderPromptBar(): void {
    if (!view) return;
    const firstRequest = view.files.length === 0 && !view.transcript.some((entry) => entry.type === 'prompt');
    promptLabel.textContent = firstRequest ? text.firstRequest : text.changeRequest;
    replace(
      sendSlot,
      generating()
        ? button({ label: text.stop, icon: 'stop', kind: 'primary', onClick: () => run(() => api.stopGeneration(id())) })
        : button({
            label: firstRequest ? text.makeApp : text.send,
            icon: 'arrowRight',
            kind: 'primary',
            disabled: draft.value.trim() === '',
            onClick: send,
          }),
    );
    replace(
      fixBar,
      consoleView.errorLog !== null && !generating()
        ? h(
            'div',
            { className: 'fix-row' },
            button({ label: text.fixErrors, icon: 'retry', onClick: fixErrors }),
            h('span', { className: 'help' }, text.fixDisclosure),
          )
        : null,
    );
  }

  function renderSide(): void {
    if (!view) return;
    const errors = view.runner.errorCount;
    const tab = (name: 'files' | 'errors', label: string, badge: number) => {
      const element = h(
        'button',
        { type: 'button', className: sideTab === name ? 'tab selected' : 'tab' },
        label,
        badge > 0 ? h('span', { className: 'badge' }, String(badge)) : null,
      );
      element.setAttribute('role', 'tab');
      element.setAttribute('aria-selected', String(sideTab === name));
      element.addEventListener('click', () => {
        sideTab = name;
        renderSide();
      });
      return element;
    };
    replace(sideTabs, tab('files', text.tabFiles(view.files.length), 0), tab('errors', text.tabErrors, errors));
    if (sideTab === 'files') renderFiles(view);
    else renderErrors();
  }

  function renderFiles(current: ProjectView): void {
    if (current.files.length === 0) {
      replace(sidePanel, h('p', { className: 'muted pad' }, text.noFiles));
      return;
    }
    const rows = current.files.map((file) => {
      const row = h(
        'button',
        { type: 'button', className: openFile?.path === file.path ? 'file selected' : 'file' },
        h('span', { className: 'file-path' }, file.path),
        h('span', { className: 'file-size' }, formatBytes(file.sizeBytes)),
      );
      row.addEventListener('click', () => {
        void showFile(file.path).then(renderSide);
      });
      return row;
    });
    replace(
      sidePanel,
      h('div', { className: 'file-list' }, ...rows),
      openFile
        ? h(
            'div',
            { className: 'file-view' },
            h(
              'div',
              { className: 'file-view-bar' },
              h('strong', {}, openFile.path),
              button({
                label: text.close,
                icon: 'close',
                kind: 'quiet',
                iconOnly: true,
                onClick: () => {
                  openFile = null;
                  renderSide();
                },
              }),
            ),
            openFile.content === null
              ? h('p', { className: 'muted pad' }, text.fileUnreadable)
              : h('pre', { className: 'code' }, openFile.content),
          )
        : null,
    );
  }

  function renderErrors(): void {
    const messages = consoleView.messages.map((message) =>
      h(
        'div',
        { className: `console-line ${message.level.toLowerCase()}` },
        h('span', { className: 'console-message' }, message.message),
        message.source ? h('span', { className: 'console-source' }, `${message.source}:${message.line}`) : null,
        message.stack ? h('pre', { className: 'console-stack' }, message.stack) : null,
      ),
    );
    replace(
      sidePanel,
      consoleView.networkBlocked ? h('p', { className: 'banner' }, text.networkBlocked) : null,
      messages.length === 0
        ? h('p', { className: 'muted pad' }, consoleView.running ? text.noErrors : text.consoleEmpty)
        : h('div', { className: 'console' }, ...messages),
    );
  }

  async function showFile(path: string): Promise<void> {
    try {
      openFile = { path, content: await api.readProjectFile(id(), path) };
    } catch {
      openFile = { path, content: null };
    }
  }

  function send(): void {
    const prompt = draft.value.trim();
    if (prompt === '' || generating()) return;
    draft.value = '';
    generate(prompt, false);
  }

  /** Sends the last run's errors to the AI service. */
  function fixErrors(): void {
    const log = consoleView.errorLog;
    if (log === null || generating()) return;
    consoleView = { ...consoleView, errorLog: null };
    generate(log, true);
  }

  function generate(prompt: string | null, fix: boolean): void {
    if (!view || view.generating) return;
    view.generating = true;
    live = NO_LIVE;
    if (prompt !== null) view.transcript = [...view.transcript, { type: 'prompt', text: prompt, fixErrors: fix }];
    render();
    transcript.scrollTop = transcript.scrollHeight;
    api.generate(view.project.id, prompt, fix).catch((error: unknown) => {
      toast(errorMessage(error), true);
      void finishGeneration(id());
    });
  }

  /** Runs a backend action, showing a failure instead of dropping it. */
  function run(action: () => Promise<unknown>): void {
    action().catch((error: unknown) => toast(errorMessage(error), true));
  }

  function onGeneration(progress: GenerationProgress): void {
    if (!view || progress.projectId !== view.project.id) return;
    view.generating = true;
    const event = progress.event;
    switch (event.type) {
      case 'textDelta':
        live = { ...live, text: live.text + event.text, retry: null };
        break;
      case 'toolRunning':
        live = { ...live, tool: { name: event.name, path: event.path }, retry: null };
        break;
      case 'turnPersisted':
        live = NO_LIVE;
        void refresh();
        return;
      case 'retrying':
        // The failed attempt's partial output is discarded.
        live = { ...NO_LIVE, retry: { delaySeconds: event.delaySeconds, reason: event.reason } };
        break;
    }
    renderTranscript();
  }

  async function finishGeneration(projectId: string): Promise<void> {
    if (!view || projectId !== view.project.id) return;
    view.generating = false;
    live = NO_LIVE;
    // An app that is open shows the changes at once.
    if (view.runner.running) run(() => api.reloadApp(projectId));
    await refresh();
  }

  async function onRunner(status: RunnerStatus): Promise<void> {
    if (!view || status.projectId !== view.project.id) return;
    view.runner = status;
    try {
      consoleView = await api.getConsole(status.projectId);
    } catch {
      consoleView = NO_CONSOLE;
    }
    renderBar();
    renderPromptBar();
    renderSide();
  }

  return { root, load, onGeneration, onGenerationFinished: finishGeneration, onRunner, projectId: id };
}
