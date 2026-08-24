/*
 * Copyright 2026 Coralogix Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

import { defineConfig } from 'vitepress'

// Tagged so OSS-driven traffic to coralogix.com is attributable per project.
const CORALOGIX_URL =
  'https://coralogix.com/?utm_source=galeforcecss-docs&utm_medium=oss&utm_campaign=galeforcecss'

// GitHub Pages serves the site under the repo name, so the production build
// needs a `/galeforce-css/` prefix. Cloudflare Pages PR previews serve it from
// the root of a `*.pages.dev` host, where that prefix would 404 every asset —
// the preview workflow sets DOCS_BASE=/ for those builds. Anything that hand-
// writes an absolute asset URL (favicon, footer mark) must go through `base`.
const base = process.env.DOCS_BASE ?? '/galeforce-css/'

export default defineConfig({
  title: 'GaleforceCSS',
  description:
    'Rust-powered Tailwind CSS v3-compatible compiler — 23x faster builds, sub-millisecond HMR.',
  base,

  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: `${base}coralogix-mark.svg` }],
    // Nunito Sans + Inconsolata are the Coralogix design system's families
    // (tailwind.theme.ts `fontFamily`). Served from Google Fonts rather than
    // vendored: the design system ships TTFs, which are several hundred kB
    // heavier than the woff2 the CDN negotiates.
    ['link', { rel: 'preconnect', href: 'https://fonts.googleapis.com' }],
    ['link', { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' }],
    [
      'link',
      {
        rel: 'stylesheet',
        href:
          'https://fonts.googleapis.com/css2?family=Nunito+Sans:ital,opsz,wght@0,6..12,400..800;1,6..12,400..800' +
          '&family=Inconsolata:wght@400..700&display=swap',
      },
    ],
  ],

  themeConfig: {
    siteTitle: 'GaleforceCSS',

    nav: [
      { text: 'Guide', link: '/guide/getting-started' },
      { text: 'Reference', link: '/reference/architecture' },
      { text: 'Conformance', link: '/reference/conformance' },
      {
        text: 'v0.1.0',
        items: [
          { text: 'Changelog', link: 'https://github.com/coralogix/galeforce-css/releases' },
          { text: 'Contributing', link: '/contributing' },
        ],
      },
    ],

    sidebar: [
      {
        text: 'Guide',
        items: [
          { text: 'Getting Started', link: '/guide/getting-started' },
          { text: 'Vite Plugin', link: '/guide/vite' },
          { text: 'CLI', link: '/guide/cli' },
          { text: 'Node API', link: '/guide/node-api' },
          { text: 'Configuration', link: '/guide/configuration' },
          { text: 'Migrating from Tailwind v3', link: '/guide/migration' },
          { text: 'Debugging', link: '/guide/debugging' },
          { text: 'Performance', link: '/guide/performance' },
          { text: 'Unsupported Features', link: '/guide/unsupported' },
        ],
      },
      {
        text: 'Reference',
        items: [
          { text: 'Architecture', link: '/reference/architecture' },
          { text: 'Conformance', link: '/reference/conformance' },
        ],
      },
      { text: 'Contributing', link: '/contributing' },
    ],

    socialLinks: [{ icon: 'github', link: 'https://github.com/coralogix/galeforce-css' }],

    footer: {
      message:
        'Released under the Apache License 2.0. Not affiliated with Tailwind Labs. ' +
        'GaleforceCSS is an independent port of Tailwind CSS v3.',
      copyright: [
        'Built with 💚 by',
        `<a href="${CORALOGIX_URL}">`,
        `<img src="${base}coralogix-mark.svg" alt="" width="14" height="14"` +
        ' style="display:inline-block;vertical-align:-2px">',
        'Coralogix</a>',
      ].join(' '),
    },

    editLink: {
      pattern: 'https://github.com/coralogix/galeforce-css/edit/master/docs/:path',
      text: 'Edit this page on GitHub',
    },

    search: {
      provider: 'local',
    },
  },
})
