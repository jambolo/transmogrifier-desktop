// The list of apps: create, open, rename, export, back up, import, and delete.

import { connectionFor } from '../api';
import type { Api, Project, ServiceChoice, Settings } from '../api';
import { button, confirmDialog, errorMessage, h, promptDialog, replace, showDialog, toast } from '../dom';
import { connectionLabel, providerLabel, relativeTime, text } from '../strings';
import emptyState from '../assets/empty-state.svg';
import { changeService, readyConnections, servicePicker } from './service';

export interface ProjectsHandlers {
  onOpen: (id: string) => void;
}

const ACCENTS = ['citrus', 'sky', 'lilac', 'coral'];

/** A stable accent for an app, so its card keeps its color. */
function accent(id: string): string {
  let hash = 0;
  for (const char of id) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return ACCENTS[hash % ACCENTS.length];
}

export function createProjectsView(api: Api, handlers: ProjectsHandlers) {
  let settings: Settings | null = null;
  const list = h('div', { className: 'app-grid' });
  const root = h(
    'section',
    { className: 'view projects' },
    h(
      'header',
      { className: 'view-header' },
      h('div', {}, h('h1', {}, text.yourApps), h('p', { className: 'muted' }, text.appsLifecycle)),
      h(
        'div',
        { className: 'actions' },
        button({ label: text.importApp, icon: 'upload', onClick: () => void importApp() }),
        button({ label: text.newApp, icon: 'plus', kind: 'primary', onClick: () => void newApp() }),
      ),
    ),
    list,
    h('footer', { className: 'view-footer muted' }, text.version(__APP_VERSION__)),
  );

  async function refresh(): Promise<void> {
    const [projects, saved] = await Promise.all([api.listProjects(), api.getSettings()]);
    settings = saved;
    if (projects.length === 0) {
      replace(
        list,
        h(
          'div',
          { className: 'empty' },
          h('img', { src: emptyState, alt: '', width: 240, height: 180 }),
          h('h2', {}, text.emptyAppsTitle),
          h('p', { className: 'muted' }, text.noApps),
        ),
      );
      return;
    }
    replace(list, ...projects.map(card));
  }

  /** The name of the app's service, or of its kind when the service isn't set up here. */
  function serviceLabel(project: Project): string {
    const connection = settings && connectionFor(settings, project);
    return connection ? connectionLabel(connection) : providerLabel(project.provider);
  }

  function card(project: Project): HTMLElement {
    const menu = h('div', { className: 'menu', hidden: true, attrs: { role: 'menu' } });
    const item = (label: string, action: () => void, danger = false) => {
      const element = h('button', { type: 'button', className: danger ? 'menu-item danger' : 'menu-item' }, label);
      element.setAttribute('role', 'menuitem');
      element.addEventListener('click', () => {
        menu.hidden = true;
        action();
      });
      return element;
    };
    menu.append(
      item(text.rename, () => void rename(project)),
      item(text.exportSite, () => void exportApp(project, false)),
      item(text.backUp, () => void exportApp(project, true)),
      item(text.createShortcut, () => void createShortcut(project)),
      item(text.changeService, () => void moveService(project)),
      item(text.resetConversation, () => void reset(project)),
      item(text.deleteApp, () => void remove(project), true),
    );
    const more = button({
      label: text.moreOptions(project.name),
      icon: 'more',
      kind: 'quiet',
      iconOnly: true,
      onClick: () => {
        const show = menu.hidden;
        for (const other of list.querySelectorAll<HTMLElement>('.menu')) other.hidden = true;
        menu.hidden = !show;
      },
    });
    more.setAttribute('aria-haspopup', 'menu');
    const open = h(
      'button',
      { type: 'button', className: 'app-open' },
      h('span', { className: 'app-name' }, project.name),
      h('span', { className: 'app-meta' }, text.projectSubtitle(relativeTime(project.updatedAtMillis), serviceLabel(project))),
    );
    open.addEventListener('click', () => handlers.onOpen(project.id));
    return h('article', { className: `app-card accent-${accent(project.id)}` }, open, more, menu);
  }

  /** Asks for the app's name and the service and model that will make it. */
  async function newApp(): Promise<void> {
    try {
      const saved = await api.getSettings();
      if (readyConnections(saved).length === 0) {
        toast(text.noServiceReady, true);
        return;
      }
      const name = h('input', { type: 'text', maxLength: 80, autocomplete: 'off' });
      const picker = servicePicker(api, saved);
      const choice = await showDialog(
        text.newApp,
        [
          h('label', { className: 'field' }, h('span', { className: 'field-label' }, text.name), name),
          h('p', { className: 'help' }, text.nameHint),
          ...picker.fields,
        ],
        [
          {
            label: text.newApp,
            kind: 'primary',
            value: () => {
              const service = picker.value();
              return name.value.trim() !== '' && service ? { name: name.value.trim(), ...service } : undefined;
            },
          },
        ],
      );
      if (choice === null) return;
      handlers.onOpen((await api.createProject(choice.name, choice.connection, choice.model)).id);
    } catch (error) {
      toast(errorMessage(error), true);
    }
  }

  async function rename(project: Project): Promise<void> {
    const name = await promptDialog(text.rename, { label: text.name, value: project.name, confirm: text.save });
    if (name === null || name === project.name) return;
    try {
      await api.renameProject(project.id, name);
      await refresh();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  }

  async function moveService(project: Project): Promise<void> {
    if (!(await changeService(api, project))) return;
    try {
      await refresh();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  }

  async function reset(project: Project): Promise<void> {
    const confirmed = await confirmDialog(text.resetTitle(project.name), text.resetBody, { label: text.resetConversation });
    if (!confirmed) return;
    try {
      await api.resetConversation(project.id);
      toast(text.resetDone);
    } catch (error) {
      toast(errorMessage(error), true);
    }
  }

  async function remove(project: Project): Promise<void> {
    const confirmed = await confirmDialog(text.deleteTitle(project.name), text.deleteBody, {
      label: text.deleteApp,
      kind: 'danger',
    });
    if (!confirmed) return;
    try {
      await api.deleteProject(project.id);
      await refresh();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  }

  async function exportApp(project: Project, backup: boolean): Promise<void> {
    try {
      const path = await api.exportProject(project.id, backup);
      if (path !== null) toast(text.exported(path));
    } catch (error) {
      toast(text.exportFailed(errorMessage(error)), true);
    }
  }

  async function createShortcut(project: Project): Promise<void> {
    try {
      await api.createShortcut(project.id);
      toast(text.shortcutCreated);
    } catch (error) {
      toast(text.shortcutFailed(errorMessage(error)), true);
    }
  }

  async function importApp(): Promise<void> {
    try {
      const archive = await api.pickArchive();
      if (archive === null) return;
      let withConversation = false;
      if (archive.hasConversation) {
        // A backup's conversation may not continue on another install, so the user chooses.
        const choice = await showDialog(
          text.importConversationTitle(archive.name),
          [h('p', {}, text.importConversationBody)],
          [
            { label: text.importWithConversation, kind: 'primary', value: () => true },
            { label: text.importWithoutConversation, value: () => false },
          ],
        );
        if (choice === null) return;
        withConversation = choice;
      }
      let service: ServiceChoice | null = null;
      // App files alone don't say what service made them. With one service there is nothing to choose.
      const saved = archive.hasProject ? null : await api.getSettings();
      if (saved && readyConnections(saved).length > 1) {
        const picker = servicePicker(api, saved);
        service = await showDialog(
          text.importServiceTitle(archive.name),
          [h('p', {}, text.importServiceBody), ...picker.fields],
          [{ label: text.importApp, kind: 'primary', value: picker.value }],
        );
        if (service === null) return;
      }
      handlers.onOpen((await api.importArchive(archive.path, withConversation, service)).id);
    } catch (error) {
      toast(text.importFailed(errorMessage(error)), true);
    }
  }

  // Menus close when the user clicks elsewhere.
  root.addEventListener('click', (event) => {
    if ((event.target as Element).closest('.icon-only')) return;
    for (const menu of list.querySelectorAll<HTMLElement>('.menu')) menu.hidden = true;
  });

  return { root, refresh };
}
