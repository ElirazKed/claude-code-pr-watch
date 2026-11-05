import type { Row } from '../types'

/** The repo's name on a tint of its own colour, as the TUI's chip: the same repo, the same
    colour, in every list. Its owner too where `isFull` (or a namesake makes the row say it). */
export function RepoChip({
  row,
  isLarge = false,
  isFull = false,
}: {
  row: Row
  isLarge?: boolean
  isFull?: boolean
}) {
  return (
    <span className={`repo h${row.hue}${isLarge ? ' lg' : ''}`} title={row.repo}>
      {isFull ? row.repo : row.repoName}
    </span>
  )
}
