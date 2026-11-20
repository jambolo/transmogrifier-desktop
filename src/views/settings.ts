// Settings: the connections to AI services, and the advanced limits.

import { EFFORTS, connectionReady, usesEndpoint } from '../api';
import type { Api, Connection, KeyTest, ModelInfo, Settings } from '../api';
import { button, confirmDialog, errorMessage, h, icon, promptDialog, replace, toast } from '../dom';
import {
  MAX_APP_INSTRUCTIONS_LENGTH,
  OUTPUT_TOKENS_RANGE,
  SERVER_PRESETS,
  TOOL_ITERATIONS_RANGE,
  connectionLabel,
  connectionModel,
  effortLabel,
  keyErrorText,
  providerHelp,
  providerServices,
  text,
} from '../strings';
import type { ProviderService } from '../strings';

export interface SettingsHandlers {
  onBack: () => void;
}

type KeyTestState = 'running' | KeyTest;

/** What marks the card for adding a server, where a connection's card has its ID. */
const ADD_CARD = 'add';

export function createSettingsView(api: Api, handlers: SettingsHandlers) {
  let settings: Settings | null = null;
  let advanced = false;
  /** Connections whose card is open, by ID: at first, the ones the user has set up. */
  let expanded: Set<string> | null = null;
  const keyTests: Record<string, KeyTestState> = {};
  /** Models returned by the last successful connection check, per connection. */
  const models: Record<string, ModelInfo[]> = {};
  /** Connections whose key field is open for replacing a saved key. */
  const replacing = new Set<string>();
  /** Server addresses typed but not yet saved. */
  const addressDrafts: Record<string, string> = {};
  /** The server being added. */
  let adding = false;
  let newServer = { name: '', endpoint: '' };

  const body = h('div', { className: 'settings-body' });
  const root = h(
    'section',
    { className: 'view settings' },
    h(
      'header',
      { className: 'view-header' },
      h(
        'div',
        { className: 'title-row' },
        button({ label: text.back, icon: 'arrowLeft', kind: 'quiet', iconOnly: true, onClick: handlers.onBack }),
        h('h1', {}, text.settings),
      ),
    ),
    body,
  );

  async function load(): Promise<void> {
    const loaded = await api.getSettings();
    settings = loaded;
    expanded ??= new Set(loaded.connections.filter(isSetUp).map((connection) => connection.id));
    render();
  }

  /** Saves a change to the settings and shows the result. */
  async function update(change: (settings: Settings) => void, rerender = true): Promise<void> {
    if (!settings) return;
    const next = structuredClone(settings);
    change(next);
    try {
      settings = await api.updateSettings(next);
    } catch (error) {
      toast(errorMessage(error), true);
    }
    if (rerender) render();
  }

  /** Forgets what the last connection check found, after the key or address changed. */
  function forgetCheck(id: string): void {
    delete keyTests[id];
    delete models[id];
  }

  /** Whether the user has given the connection what reaches its service: a key, or an address. */
  function isSetUp(connection: Connection): boolean {
    return connection.keyHint !== undefined || connection.endpoint !== undefined;
  }

  function render(): void {
    if (!settings) return;
    const current = settings;
    replace(
      body,
      h(
        'section',
        { className: 'settings-section' },
        h('h2', {}, text.services),
        h('p', { className: 'muted' }, text.setupDisclosure),
        ...current.connections.map(connectionCard),
        addCard(),
        h('p', { className: 'muted' }, text.servicePerApp),
      ),
      h(
        'section',
        { className: 'settings-section' },
        button({
          label: advanced ? text.hideAdvancedSettings : text.advancedSettings,
          icon: 'settings',
          onClick: () => {
            advanced = !advanced;
            render();
          },
        }),
        advanced ? advancedSettings(current) : null,
      ),
    );
  }

  /** A card's heading, which opens and closes it. */
  function cardToggle(id: string, label: string, open: boolean, status: string, ready: boolean, onToggle: () => void): HTMLElement {
    const toggle = h(
      'button',
      { type: 'button', className: 'provider-toggle', attrs: { 'aria-expanded': String(open), 'data-connection': id } },
      h('span', { className: 'provider-name' }, label),
      h('span', { className: ready ? 'provider-status ready' : 'provider-status' }, status),
      icon('chevronDown'),
    );
    toggle.addEventListener('click', () => {
      onToggle();
      render();
      body.querySelector<HTMLElement>(`[data-connection="${id}"]`)?.focus();
    });
    return h('h3', {}, toggle);
  }

  /** One connection: a heading that opens and closes it, and its setup when open. */
  function connectionCard(connection: Connection): HTMLElement {
    const { id } = connection;
    const open = expanded?.has(id) ?? false;
    const ready = connectionReady(connection);
    const status = ready ? text.serviceReady : connection.endpoint !== undefined ? text.serviceNeedsModel : text.serviceNotSetUp;
    const server = usesEndpoint(connection.protocol);
    const names = providerServices(connection.protocol).map((service) => service.name);
    return h(
      'article',
      { className: open ? 'card provider open' : 'card provider' },
      cardToggle(id, connectionLabel(connection), open, status, ready, () => {
        if (open) expanded?.delete(id);
        else expanded?.add(id);
      }),
      // A server is known by its address; an open one shows it in a field instead.
      server
        ? open
          ? null
          : h('p', { className: 'help' }, connection.endpoint ?? '')
        : h('p', { className: 'help' }, `${text.worksWith} ${names.join(', ')}`),
      ...(open ? connectionSetup(connection) : []),
    );
  }

  function connectionSetup(connection: Connection): Array<HTMLElement | null> {
    const { id, keyHint } = connection;
    const server = usesEndpoint(connection.protocol);
    const editing = keyHint === undefined || replacing.has(id);
    const actions = [
      server || keyHint !== undefined
        ? button({ label: text.test, icon: 'check', disabled: keyTests[id] === 'running', onClick: () => void testKey(id) })
        : null,
      keyHint !== undefined && !editing
        ? button({
            label: text.replaceKey,
            icon: 'edit',
            onClick: () => {
              replacing.add(id);
              render();
            },
          })
        : null,
      keyHint !== undefined
        ? button({ label: text.clearKey, icon: 'delete', kind: 'quiet', onClick: () => void saveKey(id, null) })
        : null,
      server ? button({ label: text.renameServer, icon: 'edit', kind: 'quiet', onClick: () => void rename(connection) }) : null,
      server ? button({ label: text.removeServer, icon: 'delete', kind: 'quiet', onClick: () => void remove(connection) }) : null,
    ].filter((action) => action !== null);
    const keyStatus =
      keyHint !== undefined
        ? h('p', { className: 'key-status' }, text.keySaved(keyHint))
        : h('p', { className: 'muted' }, server ? text.keyMissingServer : text.keyMissing);
    return [
      h('p', { className: 'help' }, providerHelp(connection.protocol)),
      server ? addressForm(connection) : null,
      keyStatus,
      editing ? keyForm(connection) : null,
      actions.length > 0 ? h('div', { className: 'actions start' }, ...actions) : null,
      keyTestStatus(connection),
      server || keyHint !== undefined ? modelField(connection) : null,
    ];
  }

  /** The card that adds a server: well-known ones to pick from, and its name and address. */
  function addCard(): HTMLElement {
    const name = h('input', { type: 'text', autocomplete: 'off', maxLength: 60, value: newServer.name });
    name.addEventListener('input', () => (newServer.name = name.value));
    const address = h('input', {
      type: 'text',
      inputMode: 'url',
      autocomplete: 'off',
      spellcheck: false,
      value: newServer.endpoint,
      placeholder: SERVER_PRESETS.local[0].endpoint,
    });
    address.addEventListener('input', () => (newServer.endpoint = address.value));
    const add = () => {
      if (name.value.trim() !== '' && address.value.trim() !== '') void addServer(name.value, address.value);
    };
    const form = h(
      'form',
      { className: 'key-form' },
      labeled(text.serverName, name),
      labeled(text.address, address, text.addressHelp),
      button({ label: text.add, icon: 'plus', kind: 'primary', onClick: add }),
    );
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      add();
    });
    return h(
      'article',
      { className: adding ? 'card provider open' : 'card provider' },
      cardToggle(ADD_CARD, text.addServer, adding, '', false, () => (adding = !adding)),
      ...(adding
        ? [
            h('p', { className: 'help' }, text.addServerHelp),
            presets(text.hostedPresets, SERVER_PRESETS.hosted),
            presets(text.localPresets, SERVER_PRESETS.local),
            form,
          ]
        : []),
    );
  }

  /** Well-known servers; choosing one fills in its name and usual address. */
  function presets(label: string, services: readonly ProviderService[]): HTMLElement {
    const chips = services.map((service) => {
      const chip = h('button', { type: 'button', className: 'chip' }, service.name);
      chip.setAttribute('aria-label', text.usePreset(service.name));
      chip.addEventListener('click', () => {
        newServer = { name: service.name, endpoint: service.endpoint ?? '' };
        render();
      });
      return chip;
    });
    return h(
      'div',
      { className: 'presets' },
      h('span', { className: 'field-label' }, label),
      h('div', { className: 'chips' }, ...chips),
    );
  }

  async function addServer(name: string, endpoint: string): Promise<void> {
    try {
      settings = await api.addConnection(name.trim(), endpoint.trim());
      const added = settings.connections[settings.connections.length - 1];
      expanded?.add(added.id);
      adding = false;
      newServer = { name: '', endpoint: '' };
    } catch (error) {
      toast(text.serverAddFailed(errorMessage(error)), true);
    }
    render();
  }

  async function rename(connection: Connection): Promise<void> {
    const name = await promptDialog(text.renameServer, { label: text.serverName, value: connection.name, confirm: text.save });
    if (name === null || name === connection.name) return;
    try {
      settings = await api.editConnection(connection.id, { name });
    } catch (error) {
      toast(errorMessage(error), true);
    }
    render();
  }

  async function remove(connection: Connection): Promise<void> {
    const confirmed = await confirmDialog(text.removeServerTitle(connection.name), text.removeServerBody, {
      label: text.removeServer,
      kind: 'danger',
    });
    if (!confirmed) return;
    try {
      settings = await api.removeConnection(connection.id);
      expanded?.delete(connection.id);
      replacing.delete(connection.id);
      delete addressDrafts[connection.id];
      forgetCheck(connection.id);
    } catch (error) {
      toast(errorMessage(error), true);
    }
    render();
  }

  async function saveAddress(id: string, endpoint: string): Promise<void> {
    try {
      settings = await api.editConnection(id, { endpoint });
      delete addressDrafts[id];
      forgetCheck(id);
    } catch (error) {
      toast(text.addressSaveFailed(errorMessage(error)), true);
    }
    render();
  }

  function addressForm(connection: Connection): HTMLElement {
    const { id } = connection;
    const input = h('input', {
      type: 'text',
      inputMode: 'url',
      autocomplete: 'off',
      spellcheck: false,
      value: addressDrafts[id] ?? connection.endpoint ?? '',
    });
    input.addEventListener('input', () => (addressDrafts[id] = input.value));
    const save = () => {
      const address = input.value.trim();
      if (address !== '' && address !== connection.endpoint) void saveAddress(id, address);
    };
    const form = h(
      'form',
      { className: 'key-form' },
      labeled(text.address, input, text.addressHelp),
      button({ label: text.save, kind: 'primary', onClick: save }),
    );
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      save();
    });
    return form;
  }

  /** Saves the key, or removes the saved one when it is null. */
  async function saveKey(id: string, key: string | null): Promise<void> {
    try {
      settings = await api.setApiKey(id, key);
      replacing.delete(id);
      forgetCheck(id);
    } catch (error) {
      toast(key === null ? errorMessage(error) : text.keySaveFailed(errorMessage(error)), true);
    }
    render();
  }

  function keyForm(connection: Connection): HTMLElement {
    const input = h('input', { type: 'password', autocomplete: 'off', spellcheck: false });
    input.setAttribute('aria-label', `${connectionLabel(connection)} ${text.apiKey}`);
    const reveal = button({
      label: text.show,
      kind: 'quiet',
      onClick: () => {
        const hidden = input.type === 'password';
        input.type = hidden ? 'text' : 'password';
        reveal.querySelector('span')!.textContent = hidden ? text.hide : text.show;
      },
    });
    const save = () => {
      const key = input.value.trim();
      if (key !== '') void saveKey(connection.id, key);
    };
    // A server's card already has a main action: saving its address.
    const kind = usesEndpoint(connection.protocol) ? 'secondary' : 'primary';
    const form = h(
      'form',
      { className: 'key-form' },
      h('label', { className: 'field grow' }, h('span', { className: 'field-label' }, text.apiKey), input),
      reveal,
      button({ label: text.save, kind, onClick: save }),
    );
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      save();
    });
    return form;
  }

  function keyTestStatus(connection: Connection): HTMLElement | null {
    const test = keyTests[connection.id];
    if (test === undefined) return null;
    if (test === 'running') return h('p', { className: 'muted', attrs: { role: 'status' } }, text.keyTesting);
    if (test.result === 'passed') {
      const passed = usesEndpoint(connection.protocol) ? text.serverTestPassed : text.keyTestPassed;
      return h('p', { className: 'success', attrs: { role: 'status' } }, passed(test.models.length));
    }
    return h('p', { className: 'error', attrs: { role: 'alert' } }, keyErrorText(connection.protocol, test.error));
  }

  async function testKey(id: string): Promise<void> {
    keyTests[id] = 'running';
    render();
    try {
      const result = await api.testApiKey(id);
      if (result.result === 'passed') models[id] = result.models;
      keyTests[id] = result;
    } catch (error) {
      delete keyTests[id];
      toast(errorMessage(error), true);
    }
    // A check that found the key missing also cleared its hint.
    settings = await api.getSettings();
    render();
  }

  /** The model new apps start with on this connection: a list after a connection check, or a name to type. */
  function modelField(connection: Connection): HTMLElement {
    const model = connectionModel(connection);
    const setModel = async (value: string) => {
      if (value.trim() === '') return;
      try {
        settings = await api.editConnection(connection.id, { model: value.trim() });
      } catch (error) {
        toast(errorMessage(error), true);
      }
      render();
    };
    const known = models[connection.id];
    if (known && known.length > 0) {
      // The saved choice stays selectable even if the key can no longer see it.
      const first = model === '' ? { id: '', displayName: text.chooseModel } : { id: model, displayName: model };
      const listed = known.some((info) => info.id === model) ? known : [first, ...known];
      const select = h(
        'select',
        {},
        ...listed.map((info) => h('option', { value: info.id, selected: info.id === model }, info.displayName)),
      );
      select.addEventListener('change', () => void setModel(select.value));
      return field(text.defaultModel, select);
    }
    const input = h('input', { type: 'text', value: model, spellcheck: false });
    input.addEventListener('change', () => void setModel(input.value));
    return field(text.defaultModel, input, usesEndpoint(connection.protocol) ? text.modelNeeded : text.modelHelp);
  }

  function advancedSettings(current: Settings): HTMLElement {
    const effort = h(
      'select',
      {},
      ...EFFORTS.map((value) => h('option', { value, selected: value === current.effort }, effortLabel(value))),
    );
    effort.addEventListener('change', () => void update((next) => (next.effort = effort.value as Settings['effort'])));

    const instructions = h('textarea', { rows: 4, value: current.appInstructions, maxLength: MAX_APP_INSTRUCTIONS_LENGTH });
    const count = h('span', { className: 'help' });
    const showCount = () => {
      count.textContent = text.characterCount(instructions.value.length, MAX_APP_INSTRUCTIONS_LENGTH);
    };
    showCount();
    instructions.addEventListener('input', showCount);
    // Saved when the field loses focus, without redrawing what the user is typing in.
    instructions.addEventListener('change', () => void update((next) => (next.appInstructions = instructions.value), false));

    const network = h('input', { type: 'checkbox', checked: current.allowAppNetwork });
    network.addEventListener('change', () => void update((next) => (next.allowAppNetwork = network.checked)));

    return h(
      'div',
      { className: 'advanced' },
      h('p', { className: 'muted' }, text.advancedHelp),
      field(text.effort, effort, text.effortHelp),
      h('h3', {}, text.limits),
      h('p', { className: 'help' }, text.limitsHelp),
      numberField(text.maxToolIterations, current.maxToolIterations, TOOL_ITERATIONS_RANGE, (value) =>
        update((next) => (next.maxToolIterations = value)),
      ),
      numberField(text.maxOutputTokens, current.maxOutputTokens, OUTPUT_TOKENS_RANGE, (value) =>
        update((next) => (next.maxOutputTokens = value)),
      ),
      field(text.appInstructions, instructions, text.appInstructionsHelp),
      count,
      h('h3', {}, text.appNetwork),
      h('label', { className: 'choice' }, network, h('span', {}, text.appNetworkLabel)),
      h('p', { className: 'help' }, text.appNetworkHelp),
    );
  }

  /** A field that shares a row with others. */
  function labeled(label: string, control: HTMLElement, help?: string): HTMLElement {
    const element = field(label, control, help);
    element.classList.add('grow');
    return element;
  }

  function field(label: string, control: HTMLElement, help?: string): HTMLElement {
    return h(
      'label',
      { className: 'field' },
      h('span', { className: 'field-label' }, label),
      control,
      help ? h('span', { className: 'help' }, help) : null,
    );
  }

  function numberField(
    label: string,
    value: number,
    [low, high]: readonly [number, number],
    onValue: (value: number) => Promise<void>,
  ): HTMLElement {
    const input = h('input', { type: 'number', value: String(value), min: String(low), max: String(high), step: '1' });
    input.addEventListener('change', () => {
      const parsed = Math.round(Number(input.value));
      // Out-of-range entries snap to the nearest allowed value.
      void onValue(Number.isFinite(parsed) ? Math.min(high, Math.max(low, parsed)) : value);
    });
    return field(label, input, text.range(low, high));
  }

  return { root, load };
}
