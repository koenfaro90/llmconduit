export interface FacetSelection {
  include: string[];
  exclude: string[];
}

export function emptyFacet(): FacetSelection { return { include: [], exclude: [] }; }

export function toggleFacet(value: FacetSelection, option: string, side: 'include' | 'exclude'): FacetSelection {
  const other = side === 'include' ? 'exclude' : 'include';
  return {
    ...value,
    [side]: value[side].includes(option) ? value[side].filter((item) => item !== option) : [...value[side], option],
    [other]: value[other].filter((item) => item !== option),
  };
}

export function matchesFacet(value: FacetSelection, candidates: readonly (string | null | undefined)[]): boolean {
  const included = value.include.length === 0 || value.include.some((item) => candidates.includes(item));
  const excluded = value.exclude.some((item) => candidates.includes(item));
  return included && !excluded;
}

export function hasFacet(value: FacetSelection): boolean { return value.include.length > 0 || value.exclude.length > 0; }

export function serializeFacets(values: Record<string, FacetSelection>): string | undefined {
  const active = Object.fromEntries(Object.entries(values).filter(([, value]) => hasFacet(value)));
  return Object.keys(active).length ? JSON.stringify(active) : undefined;
}

export function mergeFacetOptions(known: Array<{ value: string; label: string }>, values: readonly string[]) {
  const options = new Map(known.map((item) => [item.value, item]));
  for (const value of values) if (!options.has(value)) options.set(value, { value, label: value });
  return [...options.values()].sort((a, b) => a.label.localeCompare(b.label));
}
