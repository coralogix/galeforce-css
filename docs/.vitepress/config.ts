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

export default defineConfig({
  title: 'GaleforceCSS',
  description:
    'Rust-powered Tailwind CSS v3-compatible compiler — 23x faster builds, sub-millisecond HMR.',
  base: '/galeforcecss/',

  head: [['link', { rel: 'icon', type: 'image/svg+xml', href: '/galeforcecss/logo.svg' }]],

  themeConfig: {
    logo: '/logo.svg',
    siteTitle: 'GaleforceCSS',

    nav: [
      { text: 'Guide', link: '/guide/getting-started' },
      { text: 'Reference', link: '/reference/architecture' },
      { text: 'Conformance', link: '/reference/conformance' },
      {
        text: 'v0.1.0-alpha',
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
      message: 'Released under the MIT License.',
      copyright:
        'Not affiliated with Tailwind Labs. GaleforceCSS is an independent port of Tailwind CSS v3.',
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
