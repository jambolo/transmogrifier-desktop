// Settings: API keys, the service and model for new apps, and the advanced limits.

import { EFFORTS, PROVIDERS } from '../api';
import type { Api, KeyTest, ModelInfo, ProviderId, Settings } from '../api';
import { button, errorMessage, h, replace, toast } from '../dom';
import {
  DEFAULT_MODELS,
  MAX_APP_INSTRUCTIONS_LENGTH,
  OUTPUT_TOKENS_RANGE,
  TOOL_ITERATIONS_RANGE,
  effortLabel,
  keyErrorText,
  providerKeySource,
  providerLabel,
  text,
} from '../strings';

export interface SettingsHandlers {
  onBack: () => void;
}

type KeyTestState = 'running' | KeyTest;

export function createSettingsView(api: Api, handlers: SettingsHandlers) {
  let settings: Settings | null = null;
  let advanced = false;
  const keyTests: Partial<Record<ProviderId, KeyTestState>> = {};
  /** Models returned by the last successful connection check, per provider. */
  const models: Partial<Record<ProviderId, ModelInfo[]>> = {};
  /** Providers whose key field is open for replacing a saved key. */
  const replacing = new Set<ProviderId>();

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
    settings = await api.getSettings();
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

  function render(): void {
    if (!settings) return;
    const current = settings;
    const withKeys = PROVIDERS.filter((provider) => current.keyHints[provider] !== undefined);
    replace(
      body,
      h(
        'section',
        { className: 'settings-section' },
        h('h2', {}, text.apiKeys),
        h('p', { className: 'muted' }, text.setupDisclosure),
        ...PROVIDERS.map((provider) => keyCard(current, provider)),
      ),
      withKeys.length > 1 ? providerChoice(current, withKeys) : null,
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

  function keyCard(current: Settings, provider: ProviderId): HTMLElement {
    const hint = current.keyHints[provider];
    const editing = hint === undefined || replacing.has(provider);
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
    const save = async () => {
      const key = input.value.trim();
      if (key === '') return;
      try {
        settings = await api.setApiKey(provider, key);
        replacing.delete(provider);
        delete keyTests[provider];
      } catch (error) {
        toast(text.keySaveFailed(errorMessage(error)), true);
      }
      render();
    };
    const form = h(
      'form',
      { className: 'key-form' },
      h('label', { className: 'field grow' }, h('span', { className: 'field-label' }, text.apiKey), input),
      reveal,
      button({ label: text.save, kind: 'primary', onClick: () => void save() }),
    );
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      void save();
    });
    const clear = async () => {
      try {
        settings = await api.setApiKey(provider, null);
        delete keyTests[provider];
        delete models[provider];
      } catch (error) {
        toast(errorMessage(error), true);
      }
      render();
    };
    return h(
      'article',
      { className: 'card' },
      h('h3', {}, providerLabel(provider)),
      h('p', { className: 'help' }, text.apiKeyHelp(providerKeySource(provider))),
      h(
        'p',
        { className: hint === undefined ? 'muted' : 'key-status' },
        hint === undefined ? text.keyMissing : text.keySaved(hint),
      ),
      editing ? form : null,
      hint === undefined
        ? null
        : h(
            'div',
            { className: 'actions start' },
            button({
              label: text.test,
              icon: 'check',
              disabled: keyTests[provider] === 'running',
              onClick: () => void testKey(provider),
            }),
            editing
              ? null
              : button({
                  label: text.replaceKey,
                  icon: 'edit',
                  onClick: () => {
                    replacing.add(provider);
                    render();
                  },
                }),
            button({ label: text.clear, icon: 'delete', kind: 'quiet', onClick: () => void clear() }),
          ),
      keyTestStatus(provider),
    );
  }

  function keyTestStatus(provider: ProviderId): HTMLElement | null {
    const test = keyTests[provider];
    if (test === undefined) return null;
    if (test === 'running') return h('p', { className: 'muted', attrs: { role: 'status' } }, text.keyTesting);
    if (test.result === 'passed') {
      return h('p', { className: 'success', attrs: { role: 'status' } }, text.keyTestPassed(test.models.length));
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
    const provider = current.defaultProvider;
    const model = current.defaultModels[provider] ?? DEFAULT_MODELS[provider];
    const setModel = (value: string) =>
      void update((next) => {
        if (value.trim() !== '') next.defaultModels[provider] = value.trim();
      });
    const known = models[provider];
    let modelField: HTMLElement;
    if (known && known.length > 0) {
      // The saved choice stays selectable even if the key can no longer see it.
      const listed = known.some((info) => info.id === model) ? known : [{ id: model, displayName: model }, ...known];
      const select = h(
        'select',
        {},
        ...listed.map((info) => h('option', { value: info.id, selected: info.id === model }, info.displayName)),
      );
      select.addEventListener('change', () => setModel(select.value));
      modelField = select;
    } else {
      const input = h('input', { type: 'text', value: model, spellcheck: false });
      input.addEventListener('change', () => setModel(input.value));
      modelField = input;
    }

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
      field(`${text.defaultModel} (${providerLabel(provider)})`, modelField),
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
