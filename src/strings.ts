// Interface wording. Voice and vocabulary follow docs/branding/03-voice-and-messaging.md.

import type { Effort, KeyTestFailure, NoticeKind, ProviderId, TranscriptEntry } from './api';

export const text = {
  appName: 'Transmogrifier',
  tagline: 'Little apps. For right now.',
  welcome: 'A scorekeeper for tonight. A checklist for the weekend. Describe a little app you would like to use.',

  back: 'Back',
  cancel: 'Cancel',
  clear: 'Clear',
  close: 'Close',
  ok: 'OK',
  rename: 'Rename',
  save: 'Save',
  settings: 'Settings',
  show: 'Show',
  hide: 'Hide',

  // Projects
  yourApps: 'Your apps',
  appsLifecycle: 'Your apps stay until you delete them.',
  version: (version: string) => `Version ${version}`,
  newApp: 'New app',
  importApp: 'Import app',
  emptyAppsTitle: 'What needs a little app?',
  noApps: 'Start with something small. A scorekeeper, a packing list, a lunch picker.',
  name: 'App name',
  nameHint: "Something you'll recognize later.",
  moreOptions: (name: string) => `More options for ${name}`,
  exportSite: 'Export app files',
  backUp: 'Back up',
  createShortcut: 'Add desktop shortcut',
  shortcutCreated: 'Added a shortcut to your desktop.',
  shortcutFailed: (detail: string) => `Couldn't add the shortcut: ${detail}`,
  shortcutAppMissing: 'The app this shortcut opens no longer exists.',
  deleteApp: 'Delete app',
  keepApp: 'Keep app',
  deleteTitle: (name: string) => `Delete “${name}”?`,
  deleteBody: "This removes the app, its files, and its conversation from this computer. This can't be undone.",
  exported: (path: string) => `Saved to ${path}`,
  exportFailed: (detail: string) => `Couldn't save the file: ${detail}`,
  importFailed: (detail: string) => `Couldn't import the app: ${detail}`,
  importConversationTitle: (name: string) => `Import “${name}” with its conversation?`,
  importConversationBody:
    "With the conversation, your AI service remembers how the app was made. Without it, your AI service starts again from the app's files. If changes fail after importing with the conversation, import again without it.",
  importWithConversation: 'With conversation',
  importWithoutConversation: 'Without',
  projectSubtitle: (when: string, provider: string) => `Edited ${when} · ${provider}`,
  justNow: 'just now',

  // Workspace
  run: 'Open app',
  showApp: 'Show app',
  closeApp: 'Close app',
  reload: 'Reload',
  tabFiles: (count: number) => `Files (${count})`,
  tabErrors: 'Error details',
  chatEmpty: 'Describe a little app you would like to use. You can ask for changes as you go.',
  promptHint: 'Describe an app or a change',
  firstRequest: 'What should your app do?',
  changeRequest: 'What would you like to change?',
  makeApp: 'Make app',
  send: 'Send',
  stop: 'Stop',
  fixErrors: 'Try to fix it',
  fixDisclosure: 'Sends error details and relevant app context to your AI service. Usage charges apply.',
  thinking: 'Making your app…',
  toolPreparing: 'Preparing a change…',
  continueGeneration: 'Continue',
  openSettings: 'Open settings',
  noFiles: 'Your app files will appear here after you describe what to make.',
  fileUnreadable: "This file can't be read.",
  noErrors: 'No errors so far.',
  consoleEmpty: 'Open the app to see its log here. Errors can be sent to your AI service to fix.',
  networkBlocked:
    'This app tried to use the network, but network access for apps is turned off. Turn it on in Settings, under Advanced, then reload.',
  openLinkTitle: 'Open this link in your browser?',
  open: 'Open',
  projectMissing: 'This app no longer exists.',
  examples: [
    { label: 'Game night', prompt: 'A scorekeeper for two teams, with big plus and minus buttons.' },
    { label: 'Weekend away', prompt: 'A packing checklist for a weekend away. Let me add and check off items.' },
    { label: 'Pick lunch', prompt: "A lunch picker. I'll enter the choices, and it picks one at random." },
  ],

  // Settings
  apiKeys: 'Connect your AI service',
  setupDisclosure:
    "You'll need an API key from your AI service. Creating and changing apps is billed to that account. A chat subscription doesn't include this API usage.",
  apiKey: 'API key',
  apiKeyHelp: (source: string) => `A connection key from ${source}. Paste it here to let Transmogrifier make and change apps.`,
  replaceKey: 'Replace key',
  keyMissing: 'No key saved',
  keySaved: (hint: string) => `Saved key ending in …${hint}`,
  test: 'Check connection',
  keyTesting: 'Checking connection…',
  keyTestPassed: (count: number) =>
    count === 1 ? 'Connected. Your key works with 1 model.' : `Connected. Your key works with ${count} models.`,
  keySaveFailed: (detail: string) => `Couldn't save the key. ${detail}`,
  defaultProvider: 'AI service for new apps',
  defaultProviderHelp: 'Each app keeps the service it was made with.',
  advancedSettings: 'Advanced settings',
  hideAdvancedSettings: 'Hide advanced settings',
  advancedHelp: 'Choose how your AI service works. The defaults are ready to use.',
  defaultModel: 'Model for new apps',
  effort: 'Thinking effort',
  effortHelp: 'More thinking can take longer and cost more.',
  limits: 'Limits',
  limitsHelp: 'These limits control how much work each request can do. They are not spending limits.',
  maxToolIterations: 'Steps per request',
  maxOutputTokens: 'Reply limit (tokens)',
  range: (low: number, high: number) => `${low.toLocaleString()} to ${high.toLocaleString()}`,
  appInstructions: 'Instructions for all apps',
  appInstructionsHelp:
    'Your AI service follows these in every app, for example "Use large text" or "Make the apps in Spanish." Changes apply from each app\'s next request.',
  characterCount: (count: number, max: number) => `${count.toLocaleString()} / ${max.toLocaleString()}`,
  appNetwork: 'Network access for apps',
  appNetworkLabel: 'Let apps use the network',
  appNetworkHelp:
    'Apps can then load data, images, and media from other secure (https) websites. Scripts, styles, and fonts from other sites stay blocked. Off by default; applies the next time you open an app. Your AI service still avoids network calls unless you ask for them.',
};

export const MAX_APP_INSTRUCTIONS_LENGTH = 4_000;
export const TOOL_ITERATIONS_RANGE = [1, 200] as const;
export const OUTPUT_TOKENS_RANGE = [1_024, 128_000] as const;

/** Built-in model for a provider with no saved choice; mirrors `ProviderId::default_model` in the core. */
export const DEFAULT_MODELS: Record<ProviderId, string> = {
  ANTHROPIC: 'claude-opus-5-5',
  OPENAI: 'gpt-6.1-sol',
};

export function providerLabel(provider: ProviderId): string {
  return provider === 'ANTHROPIC' ? 'Claude' : 'ChatGPT';
}

/** The company that issues the provider's API keys and bills for them. */
export function providerKeySource(provider: ProviderId): string {
  return provider === 'ANTHROPIC' ? 'Anthropic' : 'OpenAI';
}

export function effortLabel(effort: Effort): string {
  const labels: Record<Effort, string> = {
    LOW: 'Low',
    MEDIUM: 'Medium',
    HIGH: 'High',
    XHIGH: 'Extra high',
    MAX: 'Max',
  };
  return labels[effort];
}

/** Activity-log line for a tool call, in progress (`running`) or finished. */
export function toolLabel(name: string, path: string | null, running: boolean): string {
  // A file tool's path streams in after its name.
  if (running && path === null && name !== 'list_files') return text.toolPreparing;
  const target = path ?? '';
  switch (name) {
    case 'list_files':
      return running ? 'Listing files…' : 'Listed files';
    case 'read_file':
      return running ? `Reading ${target}…` : `Read ${target}`;
    case 'write_file':
      return running ? `Writing ${target}…` : `Wrote ${target}`;
    case 'edit_file':
      return running ? `Editing ${target}…` : `Edited ${target}`;
    case 'delete_file':
      return running ? `Deleting ${target}…` : `Deleted ${target}`;
    default:
      return [name, path].filter(Boolean).join(' ');
  }
}

export function retryingLabel(delaySeconds: number, reason: string): string {
  const unit = delaySeconds === 1 ? 'second' : 'seconds';
  return `Trying again in ${delaySeconds} ${unit}: ${reason}`;
}

export function usageLabel(entry: Extract<TranscriptEntry, { type: 'usage' }>): string {
  const count = (value: number) => value.toLocaleString();
  return `Usage: ${count(entry.inputTokens)} input · ${count(entry.outputTokens)} output · ${count(entry.cacheReadTokens)} cached tokens`;
}

export function noticeText(kind: NoticeKind, detail?: string): string {
  switch (kind) {
    case 'STOPPED':
      return "Stopped. You can continue when you're ready.";
    case 'ITERATION_LIMIT':
      return 'This request reached its step limit. You can continue or ask for a smaller change.';
    case 'TRUNCATED':
      return 'The reply was too long to finish. Ask for a smaller change or adjust the limit in Settings.';
    case 'REFUSED':
      return "The AI service couldn't help with this request. Try a different description.";
    case 'AUTH_FAILED':
      return "Your AI service couldn't accept this key. Check it in Settings.";
    case 'FAILED':
      return `This request couldn't finish: ${detail ?? ''}`;
    case 'PROVIDER_UNAVAILABLE':
      return "This app's AI service isn't available in this version, so the app can't be changed.";
    case 'IMPORTED_WITHOUT_CONVERSATION':
      return "Imported without its conversation. Your AI service starts again from the app's files.";
  }
}

/** Notices that mean something went wrong, as opposed to a pause. */
export function noticeIsError(kind: NoticeKind): boolean {
  return kind === 'FAILED' || kind === 'AUTH_FAILED' || kind === 'PROVIDER_UNAVAILABLE';
}

/** Why a connection check failed, in plain language, with the service's own detail when it gave one. */
export function keyErrorText(provider: ProviderId, error: KeyTestFailure): string {
  const service = providerLabel(provider);
  const explanation = (() => {
    switch (error.kind) {
      case 'MISSING_KEY':
        return 'No key is saved. Paste your key above and save it.';
      case 'INVALID_KEY':
        return `${service} didn't recognize this key. Check that you copied all of it, or create a new one.`;
      case 'PERMISSION_DENIED':
        return `This key isn't allowed to use ${service}. Check its permissions in your ${providerKeySource(provider)} account.`;
      case 'RATE_LIMITED':
        return `${service} is getting too many requests from this key. Wait a minute and try again.`;
      case 'OVERLOADED':
        return `${service} is busy right now. Try again in a few minutes.`;
      case 'SERVER':
        return `${service} had a problem on its end. Try again later.`;
      case 'REJECTED':
        return `${service} refused the request.`;
      case 'NETWORK':
        switch (error.network) {
          case 'HOST_NOT_FOUND':
            return `Couldn't find the ${service} server. Check that you're connected to the internet.`;
          case 'TIMED_OUT':
            return `${service} took too long to respond. Check your connection and try again.`;
          case 'CONNECTION_REFUSED':
            return `Couldn't connect to ${service}. Check your connection, or try another network.`;
          case 'INSECURE':
            return `Couldn't make a secure connection to ${service}. Check that this computer's date and time are right, or try another network.`;
          default:
            return `The connection to ${service} failed. Check your connection and try again.`;
        }
    }
  })();
  if (error.status === null) return explanation;
  const code = error.detail ? `Error ${error.status}: ${error.detail}` : `Error ${error.status}`;
  return `${explanation} ${code}`;
}

/** "5 minutes ago", "yesterday", and so on. */
export function relativeTime(millis: number, now: number = Date.now()): string {
  const seconds = Math.round((millis - now) / 1000);
  if (Math.abs(seconds) < 60) return text.justNow;
  const format = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
  const units: Array<[Intl.RelativeTimeFormatUnit, number]> = [
    ['minute', 60],
    ['hour', 60 * 60],
    ['day', 60 * 60 * 24],
    ['month', 60 * 60 * 24 * 30],
    ['year', 60 * 60 * 24 * 365],
  ];
  let [unit, size] = units[0];
  for (const candidate of units) {
    if (Math.abs(seconds) >= candidate[1]) [unit, size] = candidate;
  }
  return format.format(Math.round(seconds / size), unit);
}
