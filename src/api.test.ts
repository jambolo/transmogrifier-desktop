// Verifies the api module's side of the wire protocol: that each helper
// invokes the right Tauri command with the right argument shape, and passes
// the backend's response through untouched. Uses Tauri's official IPC mocks,
// so no backend is needed. The Rust side of the same contract is pinned down
// in `src-tauri/src/main.rs`'s tests.

import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';
import { afterEach, describe, expect, it } from 'vitest';
import { api } from './api';

afterEach(() => {
  clearMocks();
});

/** Records every command the api invokes and answers each with `response`. */
function record(response: unknown = null): Array<{ cmd: string; args: unknown }> {
  const calls: Array<{ cmd: string; args: unknown }> = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    return response;
  });
  return calls;
}

describe('api', () => {
  it('passes the backend response through untouched', async () => {
    const project = { id: 'p1', name: 'Tips' };
    const calls = record(project);
    expect(await api.createProject('Tips', 'anthropic', 'claude-opus-5-5')).toEqual(project);
    expect(calls).toEqual([{ cmd: 'create_project', args: { name: 'Tips', connection: 'anthropic', model: 'claude-opus-5-5' } }]);
  });

  it('invokes each command with the argument shape the shell expects', async () => {
    const calls = record();
    await api.getSettings();
    await api.addConnection('Ollama', 'http://localhost:11434/v1');
    await api.editConnection('c1', { model: 'gemma4' });
    await api.removeConnection('c1');
    await api.setApiKey('openai', null);
    await api.testApiKey('anthropic');
    await api.setProjectService('p1', 'c1', 'gemma4');
    await api.renameProject('p1', 'Tip jar');
    await api.resetConversation('p1');
    await api.deleteProject('p1');
    await api.getProject('p1');
    await api.readProjectFile('p1', 'js/app.js');
    await api.generate('p1', 'A timer');
    await api.generate('p1', null, true);
    await api.stopGeneration('p1');
    await api.runApp('p1');
    await api.reloadApp('p1');
    await api.closeApp('p1');
    await api.getConsole('p1');
    await api.exportProject('p1', true);
    await api.pickArchive();
    await api.importArchive('/tmp/a.zip', false);
    await api.importArchive('/tmp/b.zip', false, { connection: 'c1', model: 'gemma4' });
    await api.createShortcut('p1');
    await api.takeLaunchRequest();
    await api.openExternal('https://example.com');
    expect(calls).toEqual([
      { cmd: 'get_settings', args: {} },
      { cmd: 'add_connection', args: { name: 'Ollama', endpoint: 'http://localhost:11434/v1' } },
      { cmd: 'edit_connection', args: { id: 'c1', edit: { model: 'gemma4' } } },
      { cmd: 'remove_connection', args: { id: 'c1' } },
      { cmd: 'set_api_key', args: { connection: 'openai', key: null } },
      { cmd: 'test_api_key', args: { connection: 'anthropic' } },
      { cmd: 'set_project_service', args: { id: 'p1', connection: 'c1', model: 'gemma4' } },
      { cmd: 'rename_project', args: { id: 'p1', name: 'Tip jar' } },
      { cmd: 'reset_conversation', args: { id: 'p1' } },
      { cmd: 'delete_project', args: { id: 'p1' } },
      { cmd: 'get_project', args: { id: 'p1' } },
      { cmd: 'read_project_file', args: { id: 'p1', path: 'js/app.js' } },
      { cmd: 'generate', args: { id: 'p1', prompt: 'A timer', fixErrors: false } },
      { cmd: 'generate', args: { id: 'p1', prompt: null, fixErrors: true } },
      { cmd: 'stop_generation', args: { id: 'p1' } },
      { cmd: 'run_app', args: { id: 'p1' } },
      { cmd: 'reload_app', args: { id: 'p1' } },
      { cmd: 'close_app', args: { id: 'p1' } },
      { cmd: 'get_console', args: { id: 'p1' } },
      { cmd: 'export_project', args: { id: 'p1', backup: true } },
      { cmd: 'pick_archive', args: {} },
      { cmd: 'import_archive', args: { path: '/tmp/a.zip', withConversation: false, service: null } },
      {
        cmd: 'import_archive',
        args: { path: '/tmp/b.zip', withConversation: false, service: { connection: 'c1', model: 'gemma4' } },
      },
      { cmd: 'create_shortcut', args: { id: 'p1' } },
      { cmd: 'take_launch_request', args: {} },
      { cmd: 'open_external', args: { url: 'https://example.com' } },
    ]);
  });

  it('rejects when the backend rejects', async () => {
    mockIPC(() => {
      throw 'backend error';
    });
    await expect(api.listProjects()).rejects.toBe('backend error');
  });

  it('delivers backend events to subscribers until they unsubscribe', async () => {
    mockIPC(() => null, { shouldMockEvents: true });
    const finished: string[] = [];
    const links: string[] = [];
    const launches: string[] = [];
    await api.onLaunch((projectId) => launches.push(projectId));
    const stop = await api.onGenerationFinished((projectId) => finished.push(projectId));
    await api.onExternalLink((url) => links.push(url));
    await emit('generation-finished', { projectId: 'p1' });
    await emit('external-link', { url: 'https://example.com' });
    await emit('launch', { projectId: 'p3' });
    stop();
    await emit('generation-finished', { projectId: 'p2' });
    expect(finished).toEqual(['p1']);
    expect(links).toEqual(['https://example.com']);
    expect(launches).toEqual(['p3']);
  });
});
