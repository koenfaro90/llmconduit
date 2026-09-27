/** Filter model for the FlowTable, kept out of the component file so react-refresh stays happy. */
import type { FlowStatus } from '../../api/types';
import { emptyFacet, type FacetSelection } from '../ui/facetModel';

export type FlowFacetName = 'status' | 'model' | 'upstream' | 'client' | 'harness' | 'session' | 'cacheBust' | 'user' | 'key';

export interface FlowFilters {
  facets: Record<FlowFacetName, FacetSelection>;
  status: FlowStatus | null;
  model: string | null;
  upstream: string | null;
  /** Gap 15 — the per-client facet: the `client_label` to scope the table (+ roll-up) to one client. */
  client: string | null;
  /** Sessions — the detected harness profile name. */
  harness: string | null;
  /** Sessions — a gateway session node id (set by the Sessions view cross-link). */
  session: string | null;
  /** Sessions — `true` scopes the table to cache-busting flows only. */
  cacheBust: boolean | null;
}

export const EMPTY_FILTERS: FlowFilters = {
  facets: { status: emptyFacet(), model: emptyFacet(), upstream: emptyFacet(), client: emptyFacet(),
    harness: emptyFacet(), session: emptyFacet(), cacheBust: emptyFacet(), user: emptyFacet(), key: emptyFacet() },
  status: null, model: null, upstream: null, client: null, harness: null, session: null, cacheBust: null,
};
