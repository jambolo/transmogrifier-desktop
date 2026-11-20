import { describe, expect, it } from 'vitest';
import { PROVIDERS, usesEndpoint } from './api';
import {
  keyErrorText,
  noticeText,
  providerLabel,
  providerServices,
  relativeTime,
  retryingLabel,
  toolLabel,
  usageLabel,
} from './strings';

describe('strings', () => {
  it('labels tool calls in progress and finished', () => {
    expect(toolLabel('write_file', null, true)).toBe('Preparing a change…');
    expect(toolLabel('write_file', 'index.html', true)).toBe('Writing index.html…');
    expect(toolLabel('edit_file', 'js/app.js', false)).toBe('Edited js/app.js');
    expect(toolLabel('list_files', null, true)).toBe('Listing files…');
    expect(toolLabel('web_search', null, false)).toBe('web_search');
  });

  it('explains connection failures with the service detail', () => {
    expect(keyErrorText('ANTHROPIC', { kind: 'INVALID_KEY', network: null, status: 401, detail: 'invalid x-api-key' })).toBe(
      "Claude didn't recognize this key. Check that you copied all of it, or create a new one. Error 401: invalid x-api-key",
    );
    expect(keyErrorText('OPENAI', { kind: 'NETWORK', network: 'HOST_NOT_FOUND', status: null, detail: null })).toBe(
      "Couldn't find the ChatGPT server. Check that you're connected to the internet.",
    );
    expect(keyErrorText('OPENAI', { kind: 'SERVER', network: null, status: 500, detail: null })).toBe(
      'ChatGPT had a problem on its end. Try again later. Error 500',
    );
  });

  it('explains a failed check of a server in terms of its address', () => {
    expect(keyErrorText('LOCAL', { kind: 'NETWORK', network: 'CONNECTION_REFUSED', status: null, detail: null })).toBe(
      "Couldn't connect to the server. Check that it is running and that the address is right.",
    );
    expect(keyErrorText('OPENAI_COMPATIBLE', { kind: 'REJECTED', network: null, status: 404, detail: 'Not Found' })).toBe(
      'The server refused the request. Check the address; it usually ends in /v1. Error 404: Not Found',
    );
  });

  it('names every provider and the services it covers', () => {
    expect(PROVIDERS.map(providerLabel)).toEqual(['Claude', 'ChatGPT', 'Gemini', 'OpenAI-compatible', 'Local model']);
    for (const provider of PROVIDERS) {
      const services = providerServices(provider);
      expect(services.length).toBeGreaterThan(0);
      // A server's services each come with an address to fill in.
      expect(services.every((service) => (service.endpoint !== undefined) === usesEndpoint(provider))).toBe(true);
    }
  });

  it('formats notices, retries, and usage', () => {
    expect(noticeText('FAILED', 'boom')).toBe("This request couldn't finish: boom");
    expect(retryingLabel(1, 'busy')).toBe('Trying again in 1 second: busy');
    expect(retryingLabel(5, 'busy')).toBe('Trying again in 5 seconds: busy');
    expect(usageLabel({ type: 'usage', inputTokens: 1, outputTokens: 2, cacheReadTokens: 3 })).toBe(
      'Usage: 1 input · 2 output · 3 cached tokens',
    );
  });

  it('describes when an app was edited', () => {
    const now = 1_000_000_000_000;
    expect(relativeTime(now - 5_000, now)).toBe('just now');
    expect(relativeTime(now - 5 * 60_000, now)).toBe('5 minutes ago');
    expect(relativeTime(now - 26 * 3_600_000, now)).toBe('yesterday');
  });
});
