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

import { Button } from '../components/Button'

const tiers = [
  {
    name: 'Hobby',
    price: '0',
    description: 'For tinkering with side projects.',
    features: ['Up to 3 projects', 'Community support', '100 builds / month'],
    featured: false,
  },
  {
    name: 'Pro',
    price: '12',
    description: 'For solo developers shipping production apps.',
    features: ['Unlimited projects', 'Email support', 'Unlimited builds', 'Custom domains', 'Priority queues'],
    featured: true,
  },
  {
    name: 'Team',
    price: '49',
    description: 'For teams that want to ship together.',
    features: ['Everything in Pro', 'Shared workspaces', 'SSO + SCIM', 'Audit logs', 'Dedicated support'],
    featured: false,
  },
]

export default function Pricing() {
  return (
    <div className="bg-white py-24 dark:bg-gray-900 sm:py-32">
      <div className="mx-auto max-w-7xl px-6 lg:px-8">
        <div className="mx-auto max-w-2xl text-center">
          <h2 className="text-base font-semibold leading-7 text-indigo-600 dark:text-indigo-400">Pricing</h2>
          <p className="mt-2 text-balance text-4xl font-bold tracking-tight text-gray-900 dark:text-gray-100 sm:text-5xl">
            Plans for builders of every size
          </p>
        </div>
        <p className="mx-auto mt-6 max-w-2xl text-pretty text-center text-lg leading-8 text-gray-600 dark:text-gray-300">
          Choose an affordable plan packed with features for engaging your audience, creating customer loyalty, and driving sales.
        </p>
        <div className="isolate mx-auto mt-10 grid max-w-md grid-cols-1 gap-8 lg:mx-0 lg:max-w-none lg:grid-cols-3">
          {tiers.map((tier, idx) => (
            <div
              key={tier.name}
              className={[
                'relative rounded-3xl p-8 ring-1 xl:p-10',
                tier.featured
                  ? 'bg-gray-900 ring-gray-900 dark:bg-gray-800 dark:ring-gray-700'
                  : 'bg-white ring-gray-200 dark:bg-gray-900 dark:ring-gray-800',
              ].join(' ')}
            >
              {tier.featured && (
                <p className="absolute -top-3 left-1/2 -translate-x-1/2 rounded-full bg-indigo-600 px-3 py-1 text-xs font-semibold uppercase tracking-wide text-white">
                  Most popular
                </p>
              )}
              <h3 className={tier.featured ? 'text-lg font-semibold leading-8 text-white' : 'text-lg font-semibold leading-8 text-gray-900 dark:text-gray-100'}>
                {tier.name}
              </h3>
              <p className={tier.featured ? 'mt-4 text-sm leading-6 text-gray-300' : 'mt-4 text-sm leading-6 text-gray-600 dark:text-gray-300'}>
                {tier.description}
              </p>
              <p className="mt-6 flex items-baseline gap-x-1">
                <span className={tier.featured ? 'text-4xl font-bold tracking-tight text-white' : 'text-4xl font-bold tracking-tight text-gray-900 dark:text-gray-100'}>
                  ${tier.price}
                </span>
                <span className={tier.featured ? 'text-sm font-semibold leading-6 text-gray-300' : 'text-sm font-semibold leading-6 text-gray-500 dark:text-gray-400'}>
                  /month
                </span>
              </p>
              <Button variant={tier.featured ? 'primary' : 'secondary'} size="md" className="mt-6 w-full">
                Get started
              </Button>
              <ul className="mt-8 space-y-3 text-sm leading-6">
                {tier.features.map((feature) => (
                  <li key={feature} className={tier.featured ? 'flex items-center gap-x-3 text-white' : 'flex items-center gap-x-3 text-gray-600 dark:text-gray-300'}>
                    <svg className="size-5 flex-none text-indigo-400" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
                      <path fillRule="evenodd" d="M16.704 4.153a.75.75 0 01.143 1.052l-8 10.5a.75.75 0 01-1.127.075l-4.5-4.5a.75.75 0 011.06-1.06l3.894 3.893 7.48-9.817a.75.75 0 011.05-.143z" clipRule="evenodd" />
                    </svg>
                    {feature}
                  </li>
                ))}
              </ul>
              <p className={`mt-6 border-t pt-4 text-xs ${tier.featured ? 'border-gray-700 text-gray-400' : 'border-gray-200 text-gray-500 dark:border-gray-800 dark:text-gray-400'}`}>
                Tier index: {idx}
              </p>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
