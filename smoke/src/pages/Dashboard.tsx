// Dashboard page exercising tables, divides, list/grid, arbitrary values,
// and a smattering of less-common utilities (line-clamp, truncate, decoration,
// underline-offset, scroll-mt, etc.) that real apps use.

const rows = [
  { id: 1, name: 'cluster-a', status: 'healthy', requests: '12.4M', errors: '0.02%' },
  { id: 2, name: 'cluster-b', status: 'degraded', requests: '8.1M', errors: '1.4%' },
  { id: 3, name: 'cluster-c', status: 'healthy', requests: '4.7M', errors: '0.01%' },
]

const statusClasses: Record<string, string> = {
  healthy: 'bg-emerald-50 text-emerald-700 ring-emerald-600/20',
  degraded: 'bg-amber-50 text-amber-700 ring-amber-600/20',
  down: 'bg-red-50 text-red-700 ring-red-600/20',
}

export default function Dashboard() {
  return (
    <div className="px-4 py-10 sm:px-6 lg:px-8">
      <div className="mx-auto max-w-7xl">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <h1 className="text-2xl/8 font-bold text-gray-900 dark:text-gray-100">Clusters</h1>
            <p className="mt-1 text-sm text-gray-600 dark:text-gray-400">
              A list of all the clusters in your workspace including their status and traffic.
            </p>
          </div>
          <div className="flex items-center gap-2">
            <input
              type="search"
              placeholder="Search..."
              className="block w-full rounded-md border-0 bg-white px-3 py-1.5 text-sm text-gray-900 shadow-sm ring-1 ring-inset ring-gray-300 placeholder:text-gray-400 focus:ring-2 focus:ring-inset focus:ring-indigo-600 dark:bg-gray-800 dark:text-gray-100 dark:ring-gray-700 sm:max-w-xs"
            />
            <button className="rounded-md bg-indigo-600 px-3 py-1.5 text-sm font-semibold text-white shadow-sm hover:bg-indigo-500 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-600">
              New cluster
            </button>
          </div>
        </div>
        <div className="-mx-4 mt-8 sm:-mx-0">
          <table className="min-w-full divide-y divide-gray-300 dark:divide-gray-700">
            <thead>
              <tr>
                <th scope="col" className="py-3.5 pl-4 pr-3 text-left text-sm font-semibold text-gray-900 dark:text-gray-100 sm:pl-0">
                  Cluster
                </th>
                <th scope="col" className="hidden px-3 py-3.5 text-left text-sm font-semibold text-gray-900 dark:text-gray-100 sm:table-cell">
                  Status
                </th>
                <th scope="col" className="hidden px-3 py-3.5 text-left text-sm font-semibold text-gray-900 dark:text-gray-100 lg:table-cell">
                  Requests
                </th>
                <th scope="col" className="px-3 py-3.5 text-left text-sm font-semibold text-gray-900 dark:text-gray-100">
                  Error rate
                </th>
                <th scope="col" className="relative py-3.5 pl-3 pr-4 sm:pr-0">
                  <span className="sr-only">Edit</span>
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-200 dark:divide-gray-800">
              {rows.map((row) => (
                <tr key={row.id} className="even:bg-gray-50/50 dark:even:bg-gray-800/30">
                  <td className="w-full max-w-0 py-4 pl-4 pr-3 text-sm font-medium text-gray-900 dark:text-gray-100 sm:w-auto sm:max-w-none sm:pl-0">
                    <span className="truncate">{row.name}</span>
                    <dl className="font-normal lg:hidden">
                      <dt className="sr-only sm:hidden">Status</dt>
                      <dd className="mt-1 truncate text-gray-700 sm:hidden dark:text-gray-300">{row.status}</dd>
                      <dt className="sr-only">Requests</dt>
                      <dd className="mt-1 truncate text-gray-500 dark:text-gray-400">{row.requests}</dd>
                    </dl>
                  </td>
                  <td className="hidden whitespace-nowrap px-3 py-4 text-sm sm:table-cell">
                    <span className={`inline-flex items-center rounded-md px-2 py-1 text-xs font-medium ring-1 ring-inset ${statusClasses[row.status] || ''}`}>
                      {row.status}
                    </span>
                  </td>
                  <td className="hidden whitespace-nowrap px-3 py-4 text-sm text-gray-500 dark:text-gray-400 lg:table-cell tabular-nums">
                    {row.requests}
                  </td>
                  <td className="whitespace-nowrap px-3 py-4 text-sm text-gray-500 dark:text-gray-400 tabular-nums">
                    {row.errors}
                  </td>
                  <td className="whitespace-nowrap py-4 pl-3 pr-4 text-right text-sm font-medium sm:pr-0">
                    <a href="#" className="text-indigo-600 hover:text-indigo-900 underline-offset-2 hover:underline dark:text-indigo-400 dark:hover:text-indigo-200">
                      Edit<span className="sr-only">, {row.name}</span>
                    </a>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <nav className="flex items-center justify-between border-t border-gray-200 px-4 py-3 sm:px-6 dark:border-gray-800">
          <p className="text-sm text-gray-700 dark:text-gray-300">
            Showing <span className="font-medium">1</span> to <span className="font-medium">3</span> of <span className="font-medium">3</span> results
          </p>
          <div className="flex flex-1 justify-end gap-2">
            <button disabled className="relative inline-flex items-center rounded-md bg-white px-3 py-2 text-sm font-semibold text-gray-900 ring-1 ring-inset ring-gray-300 hover:bg-gray-50 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-gray-800 dark:text-gray-100 dark:ring-gray-700">
              Previous
            </button>
            <button disabled className="relative inline-flex items-center rounded-md bg-white px-3 py-2 text-sm font-semibold text-gray-900 ring-1 ring-inset ring-gray-300 hover:bg-gray-50 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-gray-800 dark:text-gray-100 dark:ring-gray-700">
              Next
            </button>
          </div>
        </nav>
      </div>
    </div>
  )
}
