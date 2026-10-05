import { test as base, expect, type Page } from '@playwright/test';

/**
 * Fixed wall-clock. The mock stamps flow timestamps + time-window math off `Date.now()`,
 * so freezing it makes those render byte-identically run-to-run (stable pixel baselines).
 */
export const FIXED_NOW = Date.UTC(2026, 5, 21, 14, 20, 0); // 2026-06-21T14:20:00Z

export type ViewName =
  | 'flows'
  | 'requests'
  | 'sessions'
  | 'topology'
  | 'sankey'
  | 'theater'
  | 'overview'
  | 'chat'
  | 'providers'
  | 'gateway-settings' | 'profiles' | 'provider-settings' | 'integrations' | 'authentication' | 'access-settings' | 'observability' | 'service' | 'host-settings'
  | 'aliases' | 'access' | 'hosts' | 'recipes' | 'models' | 'deployment' | 'benchmarks';

/** Each view: the nav-tab label to click + a route-specific "ready" marker (text/regex). */
export const VIEWS: { name: ViewName; section: string; tab: string; ready: string | RegExp }[] = [
  { name: 'flows', section: 'Observe', tab: 'Flows', ready: '/v1/responses' },
  { name: 'requests', section: 'Observe', tab: 'Requests', ready: /all retained requests/i },
  { name: 'topology', section: 'Infrastructure', tab: 'Topology', ready: /click a node to filter flows/i },
  { name: 'sankey', section: 'Infrastructure', tab: 'Sankey', ready: /Token Sankey/i },
  { name: 'theater', section: 'Infrastructure', tab: 'Theater', ready: /No active streams/i },
  // Gap 16 — the control-room overview (the 5th route). Its masthead text is the ready marker.
  { name: 'overview', section: 'Observe', tab: 'Overview', ready: /control room/i },
  { name: 'sessions', section: 'Observe', tab: 'Sessions', ready: /all retained sessions/i },
  { name: 'chat', section: 'Chat', tab: 'Chat', ready: /^chat$/i },
  {name:'hosts', section:'Infrastructure', tab:'Hosts', ready:/Stored Fleet history/i},
  {name:'recipes', section:'Infrastructure', tab:'Recipes', ready:/Review immutable revisions/i},
  {name:'aliases', section:'Infrastructure', tab:'Aliases', ready:/Stable client names/i},
  {name:'models', section:'Infrastructure', tab:'Models', ready:/One row per served model/i},
  {name:'deployment', section:'Infrastructure', tab:'Deployment', ready:/Deploy recipes to ordered hosts/i},
  {name:'benchmarks', section:'Infrastructure', tab:'Benchmarks', ready:/Measure throughput and latency/i},
  { name: 'providers', section: 'Infrastructure', tab: 'Providers', ready: /provider inventory/i },
  {name:'gateway-settings',section:'Admin',tab:'Gateway',ready:'Request limits, timeouts, replay, image caches and generation defaults.'},
  {name:'profiles',section:'Infrastructure',tab:'Profiles & routing',ready:'Model profiles & routing'},
  {name:'provider-settings',section:'Infrastructure',tab:'Provider settings',ready:'Edit upstream URLs, enable or disable backends, rotate credentials and configure fallback chains.'},
  {name:'integrations',section:'Admin',tab:'Integrations',ready:'Search and vision services. Configure GitHub sign-in on Authentication.'},
  {name:'authentication',section:'Admin',tab:'Authentication',ready:/Dashboard sign-in, SSO, sessions and legacy inference authentication/},
  {name:'access-settings',section:'Admin',tab:'Access activation',ready:/Enable policy-backed API authorization/},
  {name:'observability',section:'Admin',tab:'Observability',ready:/Upstream metrics, vision probes/},
  {name:'service',section:'Admin',tab:'Service & storage',ready:/Gateway listeners, mesh controller identity/},
  {name:'host-settings',section:'Infrastructure',tab:'Host settings',ready:/Fleet owns deployment admission/},
  { name: 'access', section: 'Admin', tab: 'Access', ready: /Access control/i },
];

/**
 * Determinism shim, injected before any app code runs:
 *  - seed Math.random (mulberry32) so d3-force's jiggle lays nodes out identically;
 *  - freeze Date / Date.now so mock-stamped timestamps + the LIVE window are fixed.
 * performance.now() + timers are left alone, so d3-force / uPlot still animate to a
 * settled state — we just remove the two non-deterministic inputs (RNG + wall-clock).
 */
export async function installDeterminism(page: Page): Promise<void> {
  await page.addInitScript((fixedNow) => {
    let s = 0x02f6e2b1 >>> 0;
    Math.random = () => {
      s = (s + 0x6d2b79f5) >>> 0;
      let t = Math.imul(s ^ (s >>> 15), 1 | s);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
    const RealDate = Date;
    class FrozenDate extends RealDate {
      constructor(...args: ConstructorParameters<typeof Date>) {
        super(...(args.length ? args : [fixedNow]));
      }
      static now() {
        return fixedNow;
      }
    }
    // @ts-ignore - replace the global clock for the page
    globalThis.Date = FrozenDate;
  }, FIXED_NOW);
}

/** Log into the mock dashboard. The mock accepts any token (`/dashboard/login` -> `{ ok: true }`). */
export async function login(page: Page): Promise<void> {
  await installDeterminism(page);
  await page.goto('/dashboard/?mock=1', { waitUntil: 'networkidle' });
  await page.locator('input').first().fill('dev-token');
  await page.getByRole('button', { name: /sign in/i }).click();
  // Auth flips -> the default Chat section renders.
  await expect(page.getByRole('navigation', { name: 'Dashboard' }).getByRole('button', { name: 'Chat', exact: true })).toBeVisible();
}

/** Open a dashboard section and view, then wait for its route-specific ready marker. */
export async function openView(page: Page, view: { section: string; tab: string; ready: string | RegExp }): Promise<void> {
  const navigation = page.getByRole('navigation', { name: 'Dashboard' });
  await navigation.getByRole('button', { name: view.section, exact: true }).click();
  if (view.tab !== view.section) {
    await navigation.getByRole('button', { name: view.tab, exact: true }).click();
  }
  await expect(page.getByText(view.ready).first()).toBeVisible();
  await page.waitForLoadState('networkidle');
  // Self-hosted webfonts must paint before the pixel baseline, else metrics differ run-to-run.
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
}

/**
 * `test` with an auto console-error gate. Any `console.error` / uncaught `pageerror`
 * is collected; assert `consoleErrors` is empty in the test. Fully deterministic
 * (0 errors is 0 errors) and independent of layout/pixel jitter.
 */
export const test = base.extend<{ consoleErrors: string[] }>({
  consoleErrors: async ({ page }, use) => {
    const errors: string[] = [];
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text());
    });
    page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
    await use(errors);
  },
});

export { expect };
