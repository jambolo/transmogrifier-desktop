// Small DOM helpers: element construction, icons, and modal dialogs.

import { text } from './strings';

type Child = Node | string | null | undefined | false;

type Props<K extends keyof HTMLElementTagNameMap> = Partial<Omit<HTMLElementTagNameMap[K], 'style' | 'dataset'>> & {
  /** Attributes set with `setAttribute`: ARIA, `data-*`, and anything without a property. */
  attrs?: Record<string, string>;
};

/** Creates an element with properties and children. Strings become text nodes, never markup. */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Props<K> = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  const { attrs, ...properties } = props;
  Object.assign(element, properties);
  for (const [name, value] of Object.entries(attrs ?? {})) element.setAttribute(name, value);
  element.append(...children.filter((child): child is Node | string => child !== null && child !== undefined && child !== false));
  return element;
}

// The functional icons from docs/branding/assets/icons: 24-unit grid, 2-unit strokes, round joins and caps.
// Inlined so they take the surrounding text color.
const ICON_PATHS = {
  plus: 'M12 5v14M5 12h14',
  check: 'm5 12 4 4L19 6',
  close: 'm6 6 12 12M6 18 18 6',
  play: 'm8 4 12 8-12 8z',
  edit: 'm14 5 5 5M4 20l1-6L16 3a2 2 0 0 1 3 0l2 2a2 2 0 0 1 0 3L10 19z',
  delete: 'M3 6h18M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7M14 10v7',
  retry: 'M20 7A8 8 0 1 0 21 14M20 3v5h-5',
  arrowRight: 'M4 12h16m-6-6 6 6-6 6',
  // Drawn to match the set.
  arrowLeft: 'M20 12H4m6-6-6 6 6 6',
  more: 'M12 5h.01M12 12h.01M12 19h.01',
  settings: 'M4 7h9M17 7h3M4 17h3M11 17h9M15 4v6M9 14v6',
  stop: 'M7 7h10v10H7z',
  upload: 'M12 16V4m-5 5 5-5 5 5M4 20h16',
  chevronDown: 'm6 9 6 6 6-6',
} as const;

export type IconName = keyof typeof ICON_PATHS;

/** A decorative icon; label the control that holds it. */
export function icon(name: IconName): SVGSVGElement {
  const namespace = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(namespace, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('class', 'icon');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(namespace, 'path');
  path.setAttribute('d', ICON_PATHS[name]);
  svg.append(path);
  return svg;
}

export interface ButtonOptions {
  label: string;
  icon?: IconName;
  /** `primary` is the one main action of an area; `danger` is destructive. */
  kind?: 'primary' | 'secondary' | 'danger' | 'quiet';
  /** Shows only the icon; the label becomes the accessible name. */
  iconOnly?: boolean;
  disabled?: boolean;
  onClick: () => void;
}

export function button(options: ButtonOptions): HTMLButtonElement {
  const element = h(
    'button',
    { type: 'button', className: `button ${options.kind ?? 'secondary'}`, disabled: options.disabled ?? false },
    options.icon ? icon(options.icon) : null,
    options.iconOnly ? null : h('span', {}, options.label),
  );
  if (options.iconOnly) {
    element.classList.add('icon-only');
    element.setAttribute('aria-label', options.label);
    element.title = options.label;
  }
  element.addEventListener('click', options.onClick);
  return element;
}

/** Replaces an element's children. */
export function replace(parent: Element, ...children: Child[]): void {
  parent.replaceChildren(
    ...children.filter((child): child is Node | string => child !== null && child !== undefined && child !== false),
  );
}

interface DialogAction<T> {
  label: string;
  kind?: ButtonOptions['kind'];
  /** The dialog's result; called when the action is chosen. Returning `undefined` keeps the dialog open. */
  value: () => T | undefined;
}

/**
 * Shows a modal dialog and resolves with the chosen action's value, or `null` if it was dismissed. The first
 * action is the default: it runs when a form inside the body is submitted.
 */
export function showDialog<T>(title: string, body: Child[], actions: Array<DialogAction<T>>): Promise<T | null> {
  return new Promise((resolve) => {
    let result: T | null = null;
    let finished = false;
    const finish = () => {
      if (finished) return;
      finished = true;
      dialog.remove();
      resolve(result);
    };
    // jsdom has no modal dialogs, so the dialog also works as a plain open element there.
    const modal = typeof HTMLDialogElement !== 'undefined' && typeof HTMLDialogElement.prototype.showModal === 'function';
    const close = () => (modal ? dialog.close() : finish());
    const choose = (action: DialogAction<T>) => {
      const value = action.value();
      if (value === undefined) return;
      result = value;
      close();
    };
    const buttons = actions.map((action) =>
      button({ label: action.label, kind: action.kind ?? 'secondary', onClick: () => choose(action) }),
    );
    const form = h('form', { method: 'dialog', className: 'dialog-body' }, ...body);
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      choose(actions[0]);
    });
    const dialog = h(
      'dialog',
      { className: 'dialog' },
      h('h2', {}, title),
      form,
      h(
        'div',
        { className: 'dialog-actions' },
        button({ label: text.cancel, kind: 'quiet', onClick: close }),
        ...[...buttons].reverse(),
      ),
    );
    dialog.addEventListener('close', finish);
    document.body.append(dialog);
    if (modal) dialog.showModal();
    else dialog.setAttribute('open', '');
    const field = dialog.querySelector<HTMLElement>('input, select, textarea');
    (field ?? buttons[0])?.focus();
  });
}

/** Asks a yes/no question. Resolves to true only if the confirming action was chosen. */
export async function confirmDialog(
  title: string,
  message: string,
  confirm: { label: string; kind?: ButtonOptions['kind'] },
): Promise<boolean> {
  const answer = await showDialog(title, [h('p', {}, message)], [{ ...confirm, value: () => true }]);
  return answer === true;
}

/** Asks for a line of text. Resolves to the trimmed text, or `null` if dismissed. */
export function promptDialog(
  title: string,
  options: { label: string; hint?: string; value?: string; confirm: string },
): Promise<string | null> {
  const input = h('input', { type: 'text', value: options.value ?? '', maxLength: 80, autocomplete: 'off' });
  input.setAttribute('aria-label', options.label);
  const body = [
    h('label', { className: 'field' }, h('span', { className: 'field-label' }, options.label), input),
    options.hint ? h('p', { className: 'help' }, options.hint) : null,
  ];
  return showDialog(title, body, [{ label: options.confirm, kind: 'primary', value: () => input.value.trim() || undefined }]);
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;

/** Shows a brief message at the bottom of the window; errors stay until dismissed. */
export function toast(message: string, isError = false): void {
  document.querySelector('.toast')?.remove();
  clearTimeout(toastTimer);
  const element = h(
    'div',
    { className: isError ? 'toast error' : 'toast', attrs: { role: isError ? 'alert' : 'status' } },
    h('span', {}, message),
    button({ label: text.close, icon: 'close', kind: 'quiet', iconOnly: true, onClick: () => element.remove() }),
  );
  document.body.append(element);
  if (!isError) toastTimer = setTimeout(() => element.remove(), 6_000);
}

/** The readable part of whatever a rejected backend call threw. */
export function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
