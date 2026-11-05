// Each PR keeps one view-transition name wherever it is drawn, so a move between lists slides
// it to where it lands.

const names = new Map<string, string>()

export function transitionName(url: string): string {
  let name = names.get(url)
  if (!name) {
    name = `pr-${names.size}`
    names.set(url, name)
  }
  return name
}
