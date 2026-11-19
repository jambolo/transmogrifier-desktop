# Transmogrifier branding

**Little apps. For right now.**

A bright, witty identity for adults making small apps for everyday moments. Start with the [visual brand book](brand-preview.html), which opens locally and includes a light/dark UI concept toggle.

## Documents

| Document | Contents |
| --- | --- |
| [Brand strategy](01-brand-strategy.md) | Positioning, audience, promise, use cases, and product boundaries |
| [Voice and messaging](03-voice-and-messaging.md) | Message hierarchy, tone, vocabulary, examples, and claim rules |
| [Visual identity](04-visual-identity.md) | Logo rules, palette, measured contrast, typography, icons, illustration, and motion |
| [Campaign copy](06-campaign-copy.md) | Landing page, release-page copy, social captions, demo script, and FAQ |
| [Asset inventory](assets/README.md) | Export formats, licenses, and illustration provenance |

## Assets

Editable SVG logos and icons; outlined vector wordmarks; PNG exports; an original campaign illustration; social and feature graphics; bundled fonts and licenses; CSS/JSON design tokens; and listing copy in plain text.

The UI mockup and campaign scorekeeper are labeled concepts. Public copy discloses the Anthropic or OpenAI API key and usage costs. “For right now” means short-lived usefulness; apps remain until manually deleted.

Trademark, domain, and handle availability have not been established.

## Using the identity in the app

The theme is in [styles.css](../../src/styles.css), as CSS custom properties taken from [tokens.css](design-tokens/tokens.css), and interface wording is in [strings.ts](../../src/strings.ts). The app follows the system light/dark preference and uses the fixed brand palette instead of system accent colors. Keep error and success semantics separate from decorative coral and citrus.

Do not add fake progress, a nonfunctional undo, or a “Make app” action that also submits follow-up edits without changing its label.

The app icon set in `src-tauri/icons/` is generated from [app-icon.svg](assets/icons/app-icon.svg):

```sh
pnpm tauri icon docs/branding/assets/icons/app-icon.svg
```

## Rebuilding

Source values are in [tools/build-assets.cjs](tools/build-assets.cjs). Edit it and regenerate rather than changing exported files.

The scripts need Node.js with `@napi-rs/canvas` and `sharp`. Point `NODE_PATH` at an installation that contains them; they aren't app dependencies.

```powershell
node docs/branding/tools/build-assets.cjs
node docs/branding/tools/check-package.cjs
```

`build-assets.cjs` regenerates the logos, campaign files, PNG exports, and design tokens. It doesn't regenerate the illustration. `check-package.cjs` checks assets and local references.

Fonts are already bundled. Rebuilding them is optional and needs Python with `fonttools`:

```powershell
python docs/branding/tools/instance-fonts.py
```
