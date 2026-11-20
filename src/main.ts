// The app shell: switches between the three views and routes backend events to them.

import { api, connectionReady } from './api';
import { confirmDialog, errorMessage, toast } from './dom';
import { text } from './strings';
import { createProjectsView } from './views/projects';
import { createSettingsView } from './views/settings';
import { createWorkspaceView } from './views/workspace';
import './styles.css';

const main = document.getElementById('app')!;
const settingsButton = document.getElementById('open-settings')!;
const homeButton = document.getElementById('home')!;

/** Where Back in Settings returns to. */
let beforeSettings: () => void = showProjects;

const projects = createProjectsView(api, { onOpen: (id) => void openRequested(id) });
const workspace = createWorkspaceView(api, { onBack: showProjects, onOpenSettings: showSettings });
const settings = createSettingsView(api, { onBack: () => beforeSettings() });

function show(view: { root: HTMLElement }): void {
  main.replaceChildren(view.root);
}

function fail(error: unknown): void {
  toast(errorMessage(error), true);
}

function showProjects(): void {
  beforeSettings = showProjects;
  show(projects);
  projects.refresh().catch(fail);
}

function showWorkspace(id: string): void {
  beforeSettings = () => showWorkspace(id);
  show(workspace);
  workspace.load(id).catch(fail);
}

/** Opens what a shortcut or the list asked for: the app if it has files, its chat if not, or the list if it's gone. */
async function openRequested(id: string): Promise<void> {
  let requested;
  try {
    requested = await api.getProject(id);
  } catch {
    showProjects();
    toast(text.shortcutAppMissing, true);
    return;
  }
  showWorkspace(id);
  if (requested.files.length > 0) api.runApp(id).catch(fail);
}

function showSettings(): void {
  show(settings);
  settings.load().catch(fail);
}

settingsButton.addEventListener('click', showSettings);
homeButton.addEventListener('click', showProjects);

async function start(): Promise<void> {
  await Promise.all([
    api.onGeneration(workspace.onGeneration),
    api.onGenerationFinished((projectId) => void workspace.onGenerationFinished(projectId)),
    api.onRunner((status) => void workspace.onRunner(status)),
    api.onExternalLink((url) => {
      // A link out of a running app opens in the system browser only if the user agrees.
      void confirmDialog(text.openLinkTitle, url, { label: text.open, kind: 'primary' }).then((confirmed) => {
        if (confirmed) api.openExternal(url).catch(fail);
      });
    }),
    api.onLaunch((id) => void openRequested(id)),
  ]);
  const requested = await api.takeLaunchRequest();
  if (requested !== null) return openRequested(requested);
  // Without an AI service nothing can be made, so the first launch starts in Settings.
  const saved = await api.getSettings();
  if (saved.connections.some(connectionReady)) showProjects();
  else showSettings();
}

start().catch((error: unknown) => {
  showProjects();
  fail(error);
});
