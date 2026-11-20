// Settings: the AI services, the service for new apps, and the advanced limits.

import { EFFORTS, PROVIDERS, providerReady, usesEndpoint } from '../api';
import type { Api, KeyTest, ModelInfo, ProviderId, Settings } from '../api';
import { button, errorMessage, h, icon, replace, toast } from '../dom';
import {
  DEFAULT_MODELS,
  MAX_APP_INSTRUCTIONS_LENGTH,
  OUTPUT_TOKENS_RANGE,
  TOOL_ITERATIONS_RANGE,
  effortLabel,
  keyErrorText,
  providerHelp,
  providerLabel,
  providerServices,
  text,
} from '../strings';

export interface SettingsHandlers {
  onBack: () => void;
}

type KeyTestState = 'running' | KeyTest;

export function createSettingsView(api: Api, handlers: SettingsHandlers) {
  let settings: Settings | null = null;
  let advanced = false;
  /** Providers whose card is open: at first, the ones the user has set up. */
  let expanded: Set<ProviderId> | null = null;
  const keyTests: Partial<Record<ProviderId, KeyTestState>> = {};
  /** Models returned by the last successful connection check, per provider. */
  const models: Partial<Record<ProviderId, ModelInfo[]>> = {};
  /** Providers whose key field is open for replacing a saved key. */
  const replacing = new Set<ProviderId>();
  /** Server addresses typed or picked but not yet saved. */
  const addressDrafts: Partial<Record<ProviderId, string>> = {};

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
    expanded ??= new Set(PROVIDERS.filter((provider) => hasKey(loaded, provider) || hasAddress(loaded, provider)));
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
  function forgetCheck(provider: ProviderId): void {
    delete keyTests[provider];
    delete models[provider];
  }

  function hasKey(current: Settings, provider: ProviderId): boolean {
    return current.keyHints[provider] !== undefined;
  }

  function hasAddress(current: Settings, provider: ProviderId): boolean {
    return current.endpoints[provider] !== undefined;
  }

  function render(): void {
    if (!settings) return;
    const current = settings;
    const ready = PROVIDERS.filter((provider) => providerReady(current, provider));
    replace(
      body,
      h(
        'section',
        { className: 'settings-section' },
        h('h2', {}, text.services),
        h('p', { className: 'muted' }, text.setupDisclosure),
        ...PROVIDERS.map((provider) => providerCard(current, provider)),
      ),
      ready.length > 1 ? providerChoice(current, ready) : null,
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

  /** One provider: a heading that opens and closes it, and its setup when open. */
  function providerCard(current: Settings, provider: ProviderId): HTMLElement {
    const open = expanded?.has(provider) ?? false;
    const ready = providerReady(current, provider);
    const status = ready ? text.serviceReady : hasAddress(current, provider) ? text.serviceNeedsModel : text.serviceNotSetUp;
    const toggle = h(
      'button',
      { type: 'button', className: 'provider-toggle', attrs: { 'aria-expanded': String(open), 'data-provider': provider } },
      h('span', { className: 'provider-name' }, providerLabel(provider)),
      h('span', { className: ready ? 'provider-status ready' : 'provider-status' }, status),
      icon('chevronDown'),
    );
    toggle.addEventListener('click', () => {
      if (open) expanded?.delete(provider);
      else expanded?.add(provider);
      render();
      body.querySelector<HTMLElement>(`[data-provider="${provider}"]`)?.focus();
    });
    const services = providerServices(provider);
    const names = services.map((service) => service.name).join(', ');
    // An open card for a server lists its services as buttons instead.
    const pickable = open && usesEndpoint(provider);
    return h(
      'article',
      { className: open ? 'card provider open' : 'card provider' },
      h('h3', {}, toggle),
      pickable ? null : h('p', { className: 'help' }, `${text.worksWith} ${names}`),
      ...(open ? providerSetup(current, provider) : []),
    );
  }

  function providerSetup(current: Settings, provider: ProviderId): Array<HTMLElement | null> {
    const server = usesEndpoint(provider);
    const hint = current.keyHints[provider];
    const editing = hint === undefined || replacing.has(provider);
    const connected = server ? hasAddress(current, provider) : hint !== undefined;
    const actions = [
      connected
        ? button({
            label: text.test,
            icon: 'check',
            disabled: keyTests[provider] === 'running',
            onClick: () => void testKey(provider),
          })
        : null,
      hint !== undefined && !editing
        ? button({
            label: text.replaceKey,
            icon: 'edit',
            onClick: () => {
              replacing.add(provider);
              render();
            },
          })
        : null,
      hint !== undefined
        ? button({ label: text.clearKey, icon: 'delete', kind: 'quiet', onClick: () => void saveKey(provider, null) })
        : null,
      server && hasAddress(current, provider)
        ? button({ label: text.removeAddress, icon: 'delete', kind: 'quiet', onClick: () => void saveAddress(provider, null) })
        : null,
    ].filter((action) => action !== null);
    // A local server rarely needs a key, so only a saved one is reported.
    const keyStatus =
      hint !== undefined
        ? h('p', { className: 'key-status' }, text.keySaved(hint))
        : provider === 'LOCAL'
          ? null
          : h('p', { className: 'muted' }, text.keyMissing);
    return [
      h('p', { className: 'help' }, providerHelp(provider)),
      ...(server ? [presets(provider), addressForm(current, provider)] : []),
      keyStatus,
      editing ? keyForm(provider) : null,
      actions.length > 0 ? h('div', { className: 'actions start' }, ...actions) : null,
      keyTestStatus(provider),
      connected ? modelField(current, provider) : null,
    ];
  }

  /** The services a server adapter covers; choosing one fills in its usual address. */
  function presets(provider: ProviderId): HTMLElement {
    const chips = providerServices(provider).map((service) => {
      const chip = h('button', { type: 'button', className: 'chip' }, service.name);
      chip.setAttribute('aria-label', text.usePreset(service.name));
      chip.addEventListener('click', () => {
        addressDrafts[provider] = service.endpoint ?? '';
        render();
      });
      return chip;
    });
    return h(
      'div',
      { className: 'presets' },
      h('span', { className: 'field-label' }, text.worksWith),
      h('div', { className: 'chips' }, ...chips),
      h('span', { className: 'help' }, text.presetHint),
    );
  }

  async function saveAddress(provider: ProviderId, address: string | null): Promise<void> {
    try {
      settings = await api.setEndpoint(provider, address);
      delete addressDrafts[provider];
      forgetCheck(provider);
    } catch (error) {
      toast(address === null ? errorMessage(error) : text.addressSaveFailed(errorMessage(error)), true);
    }
    render();
  }

  function addressForm(current: Settings, provider: ProviderId): HTMLElement {
    const input = h('input', {
      type: 'text',
      inputMode: 'url',
      autocomplete: 'off',
      spellcheck: false,
      value: addressDrafts[provider] ?? current.endpoints[provider] ?? '',
      placeholder: providerServices(provider)[0]?.endpoint ?? '',
    });
    input.addEventListener('input', () => (addressDrafts[provider] = input.value));
    const save = () => {
      const address = input.value.trim();
      if (address !== '') void saveAddress(provider, address);
    };
    const form = h(
      'form',
      { className: 'key-form' },
      h(
        'label',
        { className: 'field grow' },
        h('span', { className: 'field-label' }, text.address),
        input,
        h('span', { className: 'help' }, text.addressHelp),
      ),
      button({ label: text.save, kind: 'primary', onClick: save }),
    );
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      save();
    });
    return form;
  }

  /** Saves the key, or removes the saved one when it is null. */
  async function saveKey(provider: ProviderId, key: string | null): Promise<void> {
    try {
      settings = await api.setApiKey(provider, key);
      replacing.delete(provider);
      forgetCheck(provider);
    } catch (error) {
      toast(key === null ? errorMessage(error) : text.keySaveFailed(errorMessage(error)), true);
    }
    render();
  }

  function keyForm(provider: ProviderId): HTMLElement {
    const label = provider === 'LOCAL' ? text.apiKeyOptional : text.apiKey;
    const input = h('input', { type: 'password', autocomplete: 'off', spellcheck: false });
    input.setAttribute('aria-label', `${providerLabel(provider)} ${text.apiKey}`);
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
      if (key !== '') void saveKey(provider, key);
    };
    // A server's card already has a main action: saving its address.
    const kind = usesEndpoint(provider) ? 'secondary' : 'primary';
    const form = h(
      'form',
      { className: 'key-form' },
      h('label', { className: 'field grow' }, h('span', { className: 'field-label' }, label), input),
      reveal,
      button({ label: text.save, kind, onClick: save }),
    );
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      save();
    });
    return form;
  }

  function keyTestStatus(provider: ProviderId): HTMLElement | null {
    const test = keyTests[provider];
    if (test === undefined) return null;
    if (test === 'running') return h('p', { className: 'muted', attrs: { role: 'status' } }, text.keyTesting);
    if (test.result === 'passed') {
      const passed = usesEndpoint(provider) ? text.serverTestPassed : text.keyTestPassed;
      return h('p', { className: 'success', attrs: { role: 'status' } }, passed(test.models.length));
    }
    return h('p', { className: 'error', attrs: { role: 'alert' } }, keyErrorText(provider, test.error));
  }

  async function testKey(provider: ProviderId): Promise<void> {
    keyTests[provider] = 'running';
    render();
    try {
      const result = await api.testApiKey(provider);
      if (result.result === 'passed') models[provider] = result.models;
      keyTests[provider] = result;
    } catch (error) {
      delete keyTests[provider];
      toast(errorMessage(error), true);
    }
    // A check that found the key missing also cleared its hint.
    settings = await api.getSettings();
    render();
  }

  /** The model new apps use with this provider: a list after a connection check, or a name to type. */
  function modelField(current: Settings, provider: ProviderId): HTMLElement {
    const model = current.defaultModels[provider] ?? DEFAULT_MODELS[provider];
    const setModel = (value: string) =>
      void update((next) => {
        if (value.trim() !== '') next.defaultModels[provider] = value.trim();
      });
    const known = models[provider];
    if (known && known.length > 0) {
      // The saved choice stays selectable even if the key can no longer see it.
      const first = model === '' ? { id: '', displayName: text.chooseModel } : { id: model, displayName: model };
      const listed = known.some((info) => info.id === model) ? known : [first, ...known];
      const select = h(
        'select',
        {},
        ...listed.map((info) => h('option', { value: info.id, selected: info.id === model }, info.displayName)),
      );
      select.addEventListener('change', () => setModel(select.value));
      return field(text.defaultModel, select);
    }
    const input = h('input', { type: 'text', value: model, spellcheck: false });
    input.addEventListener('change', () => setModel(input.value));
    return field(text.defaultModel, input, usesEndpoint(provider) ? text.modelNeeded : text.modelHelp);
  }

  function providerChoice(current: Settings, available: ProviderId[]): HTMLElement {
    const options = available.map((provider) => {
      const radio = h('input', { type: 'radio', name: 'default-provider', checked: current.defaultProvider === provider });
      radio.addEventListener('change', () => void update((next) => (next.defaultProvider = provider)));
      return h('label', { className: 'choice' }, radio, h('span', {}, providerLabel(provider)));
    });
    return h(
      'section',
      { className: 'settings-section' },
      h('h2', {}, text.defaultProvider),
      h('p', { className: 'muted' }, text.defaultProviderHelp),
      h('div', { className: 'choices' }, ...options),
    );
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
