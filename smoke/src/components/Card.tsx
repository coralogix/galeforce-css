import type { ReactNode } from 'react'

export function Card({
  title,
  description,
  badge,
  href,
  children,
}: {
  title: string
  description?: string
  badge?: string
  href?: string
  children?: ReactNode
}) {
  const Tag = href ? 'a' : 'div'
  return (
    <Tag
      href={href}
      className="group relative flex flex-col overflow-hidden rounded-xl border border-gray-200 bg-white p-6 shadow-sm transition hover:-translate-y-0.5 hover:border-indigo-300 hover:shadow-md focus-within:ring-2 focus-within:ring-indigo-500 dark:border-gray-800 dark:bg-gray-900 dark:hover:border-indigo-600"
    >
      {badge && (
        <span className="absolute right-4 top-4 inline-flex items-center rounded-full bg-emerald-100 px-2 py-1 text-xs font-medium text-emerald-700 ring-1 ring-inset ring-emerald-600/20 dark:bg-emerald-900 dark:text-emerald-300">
          {badge}
        </span>
      )}
      <h3 className="mb-2 line-clamp-2 text-lg font-semibold text-gray-900 group-hover:text-indigo-600 dark:text-gray-100 dark:group-hover:text-indigo-400">
        {title}
      </h3>
      {description && (
        <p className="mb-4 line-clamp-3 grow text-sm leading-relaxed text-gray-600 dark:text-gray-300">
          {description}
        </p>
      )}
      {children && <div className="mt-auto">{children}</div>}
      <span className="pointer-events-none absolute inset-0 rounded-xl ring-2 ring-transparent transition group-hover:ring-indigo-500/10" />
    </Tag>
  )
}

export function CardGrid({ children }: { children: ReactNode }) {
  return (
    <div className="grid grid-cols-1 gap-6 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
      {children}
    </div>
  )
}
