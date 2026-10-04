# Engine styles

The engine renders a handful of pages itself — sign-in, your account, OAuth
consent, delegation, elevation, the permission page and the install page. They
all use one stylesheet, `assets/engine.css`, and a script can link the same
sheet so its pages match them:

```html
<link rel="stylesheet" href="/engine/engine.css" />
```

`/engine/engine.css` is served on every host, including hosts that are not in
`server.management_hosts`, because it is for every host's pages. No script can
register a route that shadows it, since `/engine` is reserved. Linking it from
the same host needs nothing extra in a script's own Content-Security-Policy
beyond `style-src 'self'`.

## What you can rely on

Everything in the sheet is prefixed with `aw-`, so linking it cannot collide with
your own class names or custom properties. The parts are not equally stable:

1. **The `--aw-*` custom properties** are the contract. Colours, fonts, radii,
   spacing and card widths are all named here. Write your own CSS against them
   and it follows the engine's look, including dark mode:

   ```css
   .my-panel {
     background: var(--aw-color-surface);
     border: 1px solid var(--aw-color-border);
     border-radius: var(--aw-radius-lg);
     padding: var(--aw-space-5);
   }
   ```

2. **Element defaults** style `body`, headings, paragraphs, links, `code`,
   `label`, `input`, `select`, `textarea` and `button` as soon as the sheet is
   linked. The form-control rules are wrapped in `:where()`, so they have element
   specificity and any rule of your own overrides them.

3. **The `aw-` classes** are what the engine's own pages are built from. Use them,
   but expect them to change when those pages change; the properties are what
   stays put.

## Tokens

| Group   | Properties                                                                |
| ------- | ------------------------------------------------------------------------- |
| Surface | `--aw-color-bg`, `--aw-color-surface`, `--aw-color-surface-sunken`        |
| Text    | `--aw-color-text`, `--aw-color-muted`, `--aw-color-link`                  |
| Lines   | `--aw-color-border`, `--aw-color-border-strong`, `--aw-color-focus`       |
| Primary | `--aw-color-primary`, `--aw-color-primary-hover`, `--aw-color-on-primary` |
| Status  | `--aw-color-{danger,success,warning}`, each with `-bg` and `-border`      |
| Type    | `--aw-font-sans`, `--aw-font-mono`, `--aw-font-size`, `--aw-line-height`  |
| Shape   | `--aw-radius-sm`, `--aw-radius`, `--aw-radius-lg`, `--aw-shadow`          |
| Space   | `--aw-space-1` … `--aw-space-6` (0.25rem to 2rem)                         |
| Layout  | `--aw-card-width`, `--aw-card-width-wide`                                 |

## Dark mode

The sheet follows the visitor's system setting. A page that offers its own
toggle sets `data-theme` on the root element, and that overrides the system
setting in either direction:

```html
<html data-theme="dark">
  <!-- or "light" -->
</html>
```

## Classes

| Class                                           | For                                                                   |
| ----------------------------------------------- | --------------------------------------------------------------------- |
| `aw-page` (on `body`)                           | Centres a single card on the page; full-bleed on a phone              |
| `aw-card`, `aw-card--wide`                      | The card itself                                                       |
| `aw-identity`                                   | The centred line under the heading saying whose page this is          |
| `aw-explain`, `aw-muted`, `aw-small`, `aw-hint` | Quieter text: explanatory prose, an inline aside, a link line, a hint |
| `aw-notice`, `aw-notice--ok`, `aw-notice--warn` | A message about what just happened; plain `aw-notice` is an error     |
| `aw-form`                                       | A stacked form whose buttons take the full width                      |
| `aw-button`                                     | Makes a link look like a button (a `<button>` already does)           |
| `aw-button--secondary`, `--danger`, `--small`   | Button variants                                                       |
| `aw-actions`                                    | Buttons side by side, sharing the width                               |
| `aw-choice`                                     | A `<label>` holding a checkbox or radio and its description           |
| `aw-field`                                      | A `<label>` holding a select or input on a line of its own            |
| `aw-detail`, `aw-detail-label`                  | A labelled fact, as on a consent page                                 |
| `aw-rows`, `aw-row-title`, `aw-row-meta`        | A list whose items have a title, a line of detail and an action       |
| `aw-list`                                       | A plain bulleted list                                                 |
| `aw-codes`                                      | Monospaced values to copy                                             |
| `aw-divider`                                    | A rule with a word in it ("or")                                       |

A page built from them:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Subscribe</title>
    <link rel="stylesheet" href="/engine/engine.css" />
  </head>
  <body class="aw-page">
    <main class="aw-card">
      <h1>Subscribe</h1>
      <p class="aw-identity aw-muted">Get the weekly digest.</p>
      <form class="aw-form" method="post" action="/subscribe">
        <label for="email">Email</label>
        <input id="email" name="email" type="email" required />
        <label class="aw-choice">
          <input type="checkbox" name="daily" />
          Daily instead
        </label>
        <button type="submit">Subscribe</button>
      </form>
      <p class="aw-small"><a href="/">Not now</a></p>
    </main>
  </body>
</html>
```

## Account menu

A page only a signed-in person sees can carry the same person button the
engine's own signed-in pages have, in the top right with who is signed in,
a link to `/auth/account` and Sign out:

```html
<link rel="stylesheet" href="/engine/engine.css" />
<script src="/engine/engine.js"></script>

<aw-account-menu>
  <a href="/my-script/settings">Settings</a>
  <button type="button" id="export">Export</button>
</aw-account-menu>
```

Links and buttons nested in the element are your page's own items, listed above
the engine's. The element asks `/auth/status` who is signed in (`label` is their
email, else name, else id); signed out, it shows a Sign in link instead. It is
styled by the sheet above, so a page must link both. `<script src>` needs
`script-src 'self'` in the page's policy. Sign out returns to the current path.

## Caching

`/engine/engine.js` is cached the same way.

The path carries no version: a script linking it wants the look of whichever
engine is serving it, and a versioned path would stop resolving after every
upgrade. The response carries an `ETag` and `Cache-Control: public, no-cache`,
so a browser revalidates on each use and an unchanged sheet costs a `304`.

## How the engine's own pages use it

They do not link it. `engine_page::document` inlines the same bytes into a
`<style>` block carrying the response's nonce, because several of these pages
are shown when something has already gone wrong, and a page that needs a second
request to succeed before it is legible is worse at the one thing it is for.
Every engine page goes through `engine_page::document` and
`engine_page::response`, which is what keeps them from drifting apart again.
