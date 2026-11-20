// Choosing the AI service and model for one app: when it is made, and when it is moved to another.

import { connectionFor, connectionReady } from '../api';
import type { Api, Connection, Project, ServiceChoice, Settings } from '../api';
import { errorMessage, h, showDialog, toast } from '../dom';
import { connectionLabel, connectionModel, text } from '../strings';

/** The connections new apps can use. */
export function readyConnections(settings: Settings): Connection[] {
  return settings.connections.filter(connectionReady);
}

/**
 * Fields for choosing one of the ready connections and a model, starting with `initial` or the connection the
 * last new app used. There must be a ready connection. `value` is undefined until a model is entered.
 */
export function servicePicker(api: Api, settings: Settings, initial?: ServiceChoice) {
  const ready = readyConnections(settings);
  const start =
    ready.find((connection) => connection.id === initial?.connection) ??
    ready.find((connection) => connection.id === settings.lastConnection) ??
    ready[0];
  let selected = start.id;

  const select = h(
    'select',
    {},
    ...ready.map((connection) =>
      h('option', { value: connection.id, selected: connection === start }, connectionLabel(connection)),
    ),
  );
  const suggestions = h('datalist', { id: 'service-models' });
  const model = h('input', { type: 'text', autocomplete: 'off', spellcheck: false, attrs: { list: suggestions.id } });

  /** Shows the connection's model and, once the service answers, the models it offers. */
  function show(connection: Connection): void {
    selected = connection.id;
    model.value = connection === start && initial?.connection === start.id ? initial.model : connectionModel(connection);
    suggestions.replaceChildren();
    void api
      .testApiKey(connection.id)
      .then((test) => {
        if (selected !== connection.id || test.result !== 'passed') return;
        suggestions.replaceChildren(...test.models.map((info) => h('option', { value: info.id }, info.displayName)));
      })
      // The model can still be typed.
      .catch(() => undefined);
  }

  select.addEventListener('change', () => {
    const connection = ready.find((candidate) => candidate.id === select.value);
    if (connection) show(connection);
  });
  show(start);

  const field = (label: string, control: HTMLElement) =>
    h('label', { className: 'field' }, h('span', { className: 'field-label' }, label), control);
  return {
    // With one service there is nothing to choose but the model.
    fields: [ready.length > 1 ? field(text.service, select) : null, field(text.model, model), suggestions],
    value: (): ServiceChoice | undefined => {
      const chosen = model.value.trim();
      return chosen === '' ? undefined : { connection: selected, model: chosen };
    },
  };
}

/** Asks which service and model an app should use from now on, and saves the answer. Resolves to whether it changed. */
export async function changeService(api: Api, project: Project): Promise<boolean> {
  try {
    const settings = await api.getSettings();
    if (readyConnections(settings).length === 0) {
      toast(text.noServiceReady, true);
      return false;
    }
    const current = connectionFor(settings, project);
    const picker = servicePicker(api, settings, current && { connection: current.id, model: project.model });
    const choice = await showDialog(
      text.changeServiceTitle(project.name),
      [h('p', {}, text.changeServiceBody), ...picker.fields],
      [{ label: text.changeService, kind: 'primary', value: picker.value }],
    );
    if (choice === null) return false;
    await api.setProjectService(project.id, choice.connection, choice.model);
    toast(text.serviceChanged);
    return true;
  } catch (error) {
    toast(errorMessage(error), true);
    return false;
  }
}
