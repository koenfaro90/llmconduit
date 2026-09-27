import { test, expect, VIEWS, installDeterminism, login, openView } from './harness';

test.describe('Argus dashboard', () => {
  test('login shell renders before auth', async ({ page, consoleErrors }) => {
    await installDeterminism(page);
    await page.goto('/dashboard/?mock=1', { waitUntil: 'networkidle' });
    await expect(page.getByText(/dashboard token or management-enabled API key required/i)).toBeVisible();
    await page.evaluate(() => document.fonts.ready.then(() => undefined));
    await expect(page).toHaveScreenshot('login.png');
    expect(consoleErrors, 'console errors on login shell').toEqual([]);
  });

  test('requests show named identities, live usage, filters, and the normal inspector', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'requests')!);
    await expect(page.getByTestId('request-row')).toHaveCount(5);
    const running = page.getByRole('row', { name: 'Open request api_001' });
    await expect(running).toContainText('Operations');
    await expect(running).toContainText('operator laptop');
    await expect(running).toContainText('claude-x → llama-3.1-70b');
    await expect(running).toContainText('812'); // live usage overlays a durable running row with null tokens
    await expect(running.locator('td').nth(8)).toHaveText(/4[45]0ms/);
    await expect(running).toHaveAttribute('data-status', 'completed'); // live WS beats the lagging SQL row
    await expect(running.locator('td').nth(2)).not.toHaveText('—');
    await expect(running.getByTestId('request-new-input')).toHaveText('684 84%');
    await expect(running.getByTestId('request-new-input')).toHaveAttribute('data-quality', 'derived');
    await expect(running.locator('td').nth(12)).toHaveText('512');
    await expect(running.getByTestId('request-decode-speed')).toHaveText('—');
    await expect(running.getByTestId('request-decode-speed')).toHaveAttribute('data-quality', 'unavailable');
    await page.getByLabel('Search requests').fill('codex');
    await expect(page.getByTestId('request-row')).toHaveCount(2);
    await expect(page.getByTestId('request-row').first()).toContainText('Batch inference');
    await expect(page.getByTestId('request-row').first()).toContainText('batch worker');
    await expect(page.getByRole('row', { name: 'Open request api_004' }).getByTestId('request-new-input'))
      .toHaveAttribute('data-cache-bust', 'true');
    await expect(page.getByRole('row', { name: 'Open request api_004' }).getByTestId('request-new-input'))
      .toHaveText('300 25% · bust');
    await page.getByRole('button', { name: 'Clear', exact: true }).click();
    await page.getByRole('button', { name: 'User', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Include User: Batch inference' }).check();
    await expect(page.getByTestId('request-row')).toHaveCount(2);
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Clear', exact: true }).click();
    await page.getByRole('row', { name: 'Open request api_agent_1' }).click();
    await expect(page.getByTestId('flow-detail')).toBeVisible();
    await expect(page.getByTestId('flow-detail')).toContainText('Durable history');
    await expect(page.getByTestId('flow-detail')).toContainText('Subagent task');
    await page.getByRole('button', { name: 'close detail' }).click();
    await page.getByRole('row', { name: 'Open request api_001' }).click();
    await expect(page.getByTestId('flow-detail')).toBeVisible();
    await expect(page.getByTestId('flow-detail')).not.toContainText('Durable history');
    expect(consoleErrors, 'console errors on requests').toEqual([]);
  });

  test('sessions expand child sessions and reuse the request table and inspector', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'sessions')!);
    await expect(page.getByText('Select a session to see its sub-sessions and requests.')).toHaveCount(0);
    await expect(page.getByTestId('sessions-tree').locator(':scope > div').first()).toHaveClass(/w-full/);
    const root = page.getByTestId('all-sessions-section').getByTestId('session-row').filter({ hasText: 'claude-code' }).first();
    await expect(root.getByTestId('session-child-count')).toHaveText('1');
    await root.click();
    await expect(page.getByTestId('sessions-tree').locator(':scope > div').first()).toHaveClass(/w-\[42%\]/);
    await expect(page.getByTestId('all-sessions-section').getByTestId('session-roots-expanded-row')).toHaveCount(1);
    await expect(page.getByTestId('session-stat-children')).toContainText('1');
    const table = page.getByTestId('session-request-table').getByTestId('request-table');
    await expect(table.getByRole('columnheader', { name: 'Status' })).toBeVisible();
    await expect(table.getByRole('columnheader', { name: 'New IN' })).toBeVisible();
    await table.getByTestId('session-request').first().click();
    await expect(page.getByTestId('flow-detail')).toBeVisible();
    expect(consoleErrors, 'console errors on session drill-down').toEqual([]);
  });

  test('request model and provider facets search, include, and exclude multiple values', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'requests')!);
    await page.getByRole('button', { name: 'Model', exact: true }).click();
    const model = page.getByTestId('model-facet');
    await model.getByLabel('Search model').fill('claude');
    await model.getByRole('checkbox', { name: 'Include Model: claude-x' }).check();
    await expect(page.getByTestId('request-row')).toHaveCount(3);
    await model.getByRole('checkbox', { name: 'Exclude Model: claude-x' }).check();
    await expect(page.getByTestId('request-row')).toHaveCount(2);
    await model.getByLabel('Search model').fill('codex');
    await model.getByRole('checkbox', { name: 'Include Model: codex-large' }).check();
    await expect(page.getByTestId('request-row')).toHaveCount(2);
    await page.getByRole('button', { name: 'Provider', exact: true }).click();
    const provider = page.getByTestId('provider-facet');
    await provider.getByLabel('Search provider').fill('vllm');
    await provider.getByRole('checkbox', { name: 'Include Provider: vllm-a' }).check();
    await expect(page.getByTestId('request-row')).toHaveCount(2);
    expect(consoleErrors, 'console errors on request facets').toEqual([]);
  });

  test('request harness and numbered session facets apply immediately', async ({ page }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'requests')!);
    await page.getByRole('button', { name: 'Harness', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Include Harness: codex' }).check();
    await expect(page.getByTestId('request-row')).toHaveCount(2);
    await page.getByRole('button', { name: 'Clear', exact: true }).click();
    await page.getByRole('button', { name: 'Session', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Include Session: S-3' }).check();
    await expect(page.getByTestId('request-row')).toHaveCount(2);
  });

  test('session count ranges apply lower and upper bounds immediately', async ({ page }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'sessions')!);
    const history = page.getByTestId('all-sessions-section');
    await expect(history.getByTestId('session-row')).toHaveCount(3);
    await page.getByLabel('Session filters').getByRole('button', { name: 'Requests', exact: true }).click();
    await page.getByRole('spinbutton', { name: 'Requests at least' }).fill('2');
    await expect(history.getByTestId('session-row')).toHaveCount(2);
    await page.getByRole('spinbutton', { name: 'Requests at most' }).fill('2');
    await expect(history.getByTestId('session-row')).toHaveCount(2);
    await page.getByRole('spinbutton', { name: 'Requests at most' }).fill('1');
    await expect(page.getByText('Lower bound exceeds upper bound.').first()).toBeVisible();
    await page.getByRole('spinbutton', { name: 'Requests at least' }).fill('');
    await expect(history.getByTestId('session-row')).toHaveCount(1);
    await history.getByRole('button', { name: 'session-roots columns' }).click();
    await history.getByTestId('session-roots-column-chooser').getByLabel('requests', { exact: true }).uncheck();
    await expect(page.getByLabel('Session filters').getByRole('button', { name: 'Requests', exact: true })).toHaveCount(0);
    await expect(page.getByTestId('session-hidden-filters')).toContainText('Requests');
    await expect(history.getByTestId('session-row')).toHaveCount(1);
  });

  test('table column choices apply to rows and persist across views', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'requests')!);
    const requests = page.getByTestId('request-table');
    await page.getByRole('button', { name: 'User', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Include User: Batch inference' }).check();
    await expect(requests.getByTestId('request-row')).toHaveCount(2);
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'requests columns' }).click();
    await page.getByTestId('requests-column-chooser').getByLabel('User', { exact: true }).uncheck();
    await expect(page.getByRole('button', { name: 'User', exact: true })).toHaveCount(0);
    await expect(page.getByTestId('request-hidden-filters')).toContainText('user');
    await expect(requests.getByTestId('request-row')).toHaveCount(2);
    await page.getByTestId('requests-column-chooser').getByLabel('User', { exact: true }).check();
    await expect(page.getByRole('button', { name: 'User', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Clear', exact: true }).click();
    const before = await requests.getByTestId('request-row').first().locator('td').count();
    await page.getByTestId('requests-column-chooser').getByLabel('TTFT').uncheck();
    await expect(requests.getByRole('columnheader', { name: 'TTFT' })).toHaveCount(0);
    await expect(requests.getByTestId('request-row').first().locator('td')).toHaveCount(before - 1);

    await openView(page, VIEWS.find((view) => view.name === 'sessions')!);
    await openView(page, VIEWS.find((view) => view.name === 'requests')!);
    await expect(page.getByTestId('request-table').getByRole('columnheader', { name: 'TTFT' })).toHaveCount(0);
    expect(consoleErrors, 'console errors on shared column controls').toEqual([]);
  });

  test('running request phases and reported decode rate update without a page refresh', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'requests')!);
    await page.waitForTimeout(500); // let the finite mock terminal frame settle
    const row = page.getByRole('row', { name: 'Open request api_001' });
    await page.evaluate(async () => {
      const { dashboardStore } = await import('/dashboard/src/store/dashboardStore.ts');
      const flow = dashboardStore.getState().flows.get('api_001')!;
      dashboardStore.getState().upsertFlow({ ...flow, response_id: 'resp_phase', status: 'open',
        first_content_delta_ms: null, first_upstream_byte_ms: null,
        usage: { prompt: 812, completion: 100, total: 912 } });
    });
    await expect(row).toHaveAttribute('data-status', 'queued');
    await page.evaluate(async () => {
      const { dashboardStore } = await import('/dashboard/src/store/dashboardStore.ts');
      const flow = dashboardStore.getState().flows.get('api_001')!;
      dashboardStore.getState().upsertFlow({ ...flow, first_upstream_byte_ms: flow.started_ms + 200 });
    });
    await expect(row).toHaveAttribute('data-status', 'prefilling');
    await page.evaluate(async () => {
      const { dashboardStore } = await import('/dashboard/src/store/dashboardStore.ts');
      const flow = dashboardStore.getState().flows.get('api_001')!;
      dashboardStore.getState().upsertFlow({ ...flow, first_content_delta_ms: flow.started_ms + 400 });
    });
    await expect(row).toHaveAttribute('data-status', 'decoding');
    await expect(row.getByTestId('request-decode-speed')).toHaveText('—');
    await page.waitForTimeout(300);
    await page.evaluate(async () => {
      const { dashboardStore } = await import('/dashboard/src/store/dashboardStore.ts');
      dashboardStore.getState().patchUsage('api_001', { prompt: 812, completion: 160, total: 972 });
    });
    await expect(row.getByTestId('request-decode-speed')).toHaveAttribute('data-quality', 'measured');
    await expect(row.getByTestId('request-decode-speed')).toContainText('tok/s');
    expect(consoleErrors, 'console errors on live request phases').toEqual([]);
  });

  test('request history clear requires an exact typed confirmation', async ({ page, consoleErrors }) => {
    await login(page);
    await page.setViewportSize({ width: 800, height: 600 });
    await openView(page, VIEWS.find((view) => view.name === 'requests')!);
    await page.getByRole('button', { name: 'Clear history…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Clear request and session history' });
    await expect(dialog).toBeVisible();
    const deleteButton = dialog.getByRole('button', { name: 'Delete all history' });
    await expect(deleteButton).toHaveCount(0);
    await dialog.getByLabel('Type confirmation phrase').fill('DELETE ALL REQUESTS');
    await expect(deleteButton).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(page.getByTestId('request-row')).toHaveCount(5);

    await page.getByRole('button', { name: 'Clear history…' }).click();
    const input = page.getByLabel('Type confirmation phrase');
    await input.fill('DELETE ALL REQUESTS AND SESSIONS');
    await expect(deleteButton).toBeVisible();
    const colors = await deleteButton.evaluate((element) => {
      const style = getComputedStyle(element);
      return { background: style.backgroundColor, text: style.color };
    });
    expect(colors).toEqual({ background: 'rgb(255, 110, 110)', text: 'rgb(11, 13, 23)' });
    const buttonBox = await deleteButton.boundingBox();
    expect(buttonBox && buttonBox.y + buttonBox.height).toBeLessThanOrEqual(600);
    await expect(dialog.getByRole('status')).toContainText('Nothing has been deleted');
    await input.press('Enter');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('request-row')).toHaveCount(5);
    await deleteButton.click();
    await expect(dialog).toHaveCount(0);
    await expect(page.getByTestId('request-row')).toHaveCount(0);
    await expect(page.getByRole('status')).toContainText('Deleted 5 requests and 3 sessions');
    expect(consoleErrors, 'console errors on history clear').toEqual([]);
  });

  test('navigation and status controls reflow at half-screen width', async ({ page, consoleErrors }) => {
    await page.setViewportSize({ width: 800, height: 900 });
    await login(page);
    await page.waitForTimeout(800);

    const dimensions = await page.evaluate(() => {
      const nav = document.querySelector('nav');
      const stats = document.querySelector('[data-testid="stats-strip"]');
      return {
        viewport: innerWidth,
        document: document.documentElement.scrollWidth,
        navClient: nav?.clientWidth ?? 0,
        navScroll: nav?.scrollWidth ?? 0,
        statsClient: stats?.clientWidth ?? 0,
        statsScroll: stats?.scrollWidth ?? 0,
      };
    });

    expect(dimensions.document).toBeLessThanOrEqual(dimensions.viewport);
    expect(dimensions.navScroll).toBeLessThanOrEqual(dimensions.navClient);
    expect(dimensions.statsScroll).toBeLessThanOrEqual(dimensions.statsClient);
    await expect(page.getByRole('navigation').getByRole('button', { name: 'Admin', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Logout', exact: true })).toBeVisible();
    await expect(page.getByTestId('window-selector')).toBeVisible();
    expect(consoleErrors, 'console errors at half-screen width').toEqual([]);
  });

  // Gap 01: the stats strip must be HONEST under live (mock-streamed) traffic — real
  // numeric values, not the all-`0.0` the live WS tile used to ship. The mock streams a
  // snapshot + a metric_tick (active_streams/tokens_per_sec/cost_per_min all > 0), so the
  // chips must read real numbers — and the don't-lie-with-zeros markers must NOT appear
  // while real samples are present.
  test('stats strip reads real metrics under live mock traffic (gap 01)', async ({ page, consoleErrors }) => {
    await login(page);
    // Let the mock deliver its snapshot + live frames (incl. the metric_tick).
    await page.waitForTimeout(800);

    // tok/s + $/min + active are the fields the OLD WS tile hard-coded to 0 — they must
    // now carry real values (the mock seeds them > 0), proving live flows reach the strip.
    for (const key of ['active_streams', 'prefill_tokens_per_sec', 'decode_tokens_per_sec', 'cost_per_min', 'reqs_per_sec']) {
      const value = page.getByTestId(`chip-${key}`).getByTestId('chip-value');
      await expect(value).toBeVisible();
      const text = (await value.textContent())?.trim() ?? '';
      expect(text, `${key} must be measured, not unavailable`).not.toBe('—');
      // A real, non-zero reading (the mock's seeded window is all > 0).
      expect(text, `${key} reads a real number`).toMatch(/[1-9]/);
    }

    // Gap 01 finding 4: every chip exposes its data-quality provenance. The mock window
    // is fully measured, so directly-counted metrics read `measured`, sample-derived ones
    // `derived`, and the priced cost `estimated` (labelled as such, per the plan).
    const quality = (key: string) => page.getByTestId(`chip-${key}`).getAttribute('data-quality');
    expect(await quality('reqs_per_sec')).toBe('measured');
    expect(await quality('active_streams')).toBe('measured');
    expect(await quality('p50')).toBe('derived');
    expect(await quality('prefill_tokens_per_sec')).toBe('derived');
    expect(await quality('decode_tokens_per_sec')).toBe('derived');
    expect(await quality('cost_per_min')).toBe('estimated');

    expect(consoleErrors, 'console errors on the stats strip').toEqual([]);
  });

  // Gap 08: the tokens cell reveals a token-economics popover (cached/reasoning split + cache-hit
  // + "$ saved"), and the aggregate cache-economics panel rolls the hit rate up by model. The
  // popover must render the split AND an honest `—` for an UNREPORTED class (never a fabricated 0).
  test('tokens popover shows the cached/reasoning split + — on unreported (gap 08)', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'flows')!); // Flows
    await page.waitForTimeout(400);

    // api_002's seed flow reports prompt/completion but UNREPORTED cached/reasoning → the popover
    // must show `—` for those classes, not `0`. Hover its tokens cell to reveal the breakdown.
    const row = page.getByTestId('flow-row').filter({ hasText: '/v1/chat/completions' }).first();
    await row.getByTestId('tokens-cell').hover();
    const popover = page.getByTestId('tokens-popover');
    await expect(popover).toBeVisible();
    // The split lines are present…
    await expect(popover.getByTestId('econ-line-cached')).toBeVisible();
    await expect(popover.getByTestId('econ-line-reasoning')).toBeVisible();
    // …and the unreported cached class reads the unavailable marker, NEVER `0`.
    const cachedLine = popover.getByTestId('econ-line-cached');
    await expect(cachedLine).toHaveAttribute('data-quality', 'unavailable');
    await expect(cachedLine).toContainText('—');

    // The aggregate cache-economics panel expands to a per-model roll-up.
    await page.getByTestId('cache-economics-toggle').click();
    await expect(page.getByTestId('cache-economics-table')).toBeVisible();
    await expect(page.getByTestId('cache-economics-row').first()).toBeVisible();

    expect(consoleErrors, 'console errors on the tokens popover').toEqual([]);
  });

  // Gap 09: the FlowDetail inspector shows a context-window utilization gauge, and the flows
  // screen shows an aggregate context-pressure stat. The gauge must render a DERIVED % for a flow
  // on a model WITH a known context window, and `—` (unavailable) for one WITHOUT — never a
  // fabricated 0%/100%. Covers the acceptance criterion: gauge WITH and WITHOUT `context_limit`.
  test('context gauge: derived with a known limit, — without (gap 09)', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'flows')!); // Flows
    await page.waitForTimeout(400);

    // The aggregate context-pressure stat is present, with a measured-coverage readout.
    await expect(page.getByTestId('context-pressure-panel')).toBeVisible();
    await expect(page.getByTestId('context-pressure-coverage')).toContainText('measured');

    // api_001 is served by llama-3.1-70b (catalog context_limit 131072) + reports usage → the
    // inspector gauge reads a DERIVED utilization, not `—`.
    const known = page.getByTestId('flow-row').filter({ hasText: '/v1/responses' }).first();
    await known.click();
    await expect(page.getByTestId('flow-detail')).toBeVisible();
    const gauge = page.getByTestId('context-gauge');
    await expect(gauge).toBeVisible();
    await expect(gauge).toHaveAttribute('data-quality', 'derived');
    await expect(page.getByTestId('context-util-pct')).not.toHaveText('—');
    await expect(page.getByTestId('context-gauge-fill')).toBeVisible();

    // api_004 is served by `mystery-model` (catalog context_limit NULL) but DOES report usage →
    // the gauge must read `—` (unknown capacity), NEVER 0% / 100%. Select by the model id (unique
    // to that row) so the known-window llama row on the same endpoint is not picked instead.
    const unknown = page.getByTestId('flow-row').filter({ hasText: 'mystery-model' }).first();
    await unknown.click();
    await expect(page.getByTestId('context-gauge')).toHaveAttribute('data-quality', 'unavailable');
    const pct = page.getByTestId('context-util-pct');
    await expect(pct).toHaveText('—');
    await expect(page.getByTestId('context-gauge-fill')).toHaveCount(0);

    expect(consoleErrors, 'console errors on the context gauge').toEqual([]);
  });

  // Gap 10: the FlowDetail inspector shows a per-flow latency breakdown — a "Timing" line
  // (TTFT/wire TTFB/total/tok-s) + a phase waterfall. The MEASURED/derived TTFT label must switch
  // correctly: a flow with the full gap-02 spine reads a MEASURED TTFT (no est badge) and renders
  // every waterfall segment; a flow that errored before content shows its prefill/generation
  // segments as `—` (unavailable), never a fabricated 0ms.
  test('latency breakdown: measured TTFT + waterfall, — on missing phases (gap 10)', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'flows')!); // Flows
    await page.waitForTimeout(400);

    // api_002 (completed) carries the FULL phase spine (incl. stream_end + finalize) + a served
    // attempt with a wire first byte → a MEASURED TTFT (first_content_delta), a measured wire TTFB,
    // and EVERY waterfall segment present (incl. generation + finalize). Select it by its endpoint,
    // excluding the mystery-model row on the same endpoint.
    const known = page
      .getByTestId('flow-row')
      .filter({ hasText: '/v1/chat/completions' })
      .filter({ hasNotText: 'mystery' })
      .first();
    await known.click();
    await expect(page.getByTestId('flow-detail')).toBeVisible();
    const breakdown = page.getByTestId('latency-breakdown');
    await expect(breakdown).toBeVisible();

    const ttft = page.getByTestId('latency-ttft');
    await expect(ttft).toHaveAttribute('data-quality', 'measured');
    await expect(ttft).not.toContainText('—');
    // A MEASURED TTFT carries NO est badge (the derived fallback would).
    await expect(ttft.getByTestId('latency-quality-badge')).toHaveCount(0);
    // The wire TTFB segment is enriched (measured) and the full waterfall is present.
    await expect(page.getByTestId('latency-ttfb')).toHaveAttribute('data-quality', 'measured');
    await expect(page.getByTestId('latency-seg-upstream')).toBeVisible();
    await expect(page.getByTestId('latency-seg-generation')).toBeVisible();
    await expect(page.getByTestId('latency-seg-finalize')).toBeVisible();

    // api_003 (failed before content): the prefill + generation segments are UNAVAILABLE — `—`, NOT
    // 0ms — and have no bar fill. Select by its id (the row renders the short api_call_id verbatim;
    // `openai` is no longer unique — the gap-11 `api_005` failover flow also serves it).
    const failed = page.locator('[data-api-call-id="api_003"]');
    await failed.click();
    await expect(page.getByTestId('latency-legend-prefill')).toHaveAttribute('data-quality', 'unavailable');
    await expect(page.getByTestId('latency-dur-prefill')).toHaveText('—');
    await expect(page.getByTestId('latency-seg-prefill')).toHaveCount(0); // no width in the bar
    // TTFT for an errored-before-content flow reads `—` (unavailable), never 0.
    await expect(page.getByTestId('latency-ttft')).toHaveAttribute('data-quality', 'unavailable');
    await expect(page.getByTestId('latency-ttft')).toContainText('—');

    // api_004 (mystery-model: full content spine but NO wire first byte / no served attempt): the
    // prefill segment must NOT be presented as a MEASURED prefill from routing→content (gap-10 review
    // round 1). It is a SEPARATELY-LABELLED `derived` "routing → first token" span — `data-quality`
    // is `derived` (not `measured`), it carries a visible `derived` badge, and its label is NOT
    // "prefill". The wire TTFB headline is unavailable since no first byte was measured.
    const mystery = page.getByTestId('flow-row').filter({ hasText: 'mystery' }).first();
    await mystery.click();
    const prefillLegend = page.getByTestId('latency-legend-prefill');
    await expect(prefillLegend).toHaveAttribute('data-quality', 'derived');
    await expect(prefillLegend).not.toHaveAttribute('data-quality', 'measured');
    await expect(page.getByTestId('latency-derived-prefill')).toBeVisible(); // the labelled `derived` marker
    await expect(prefillLegend).toContainText(/routing/i);
    await expect(prefillLegend).not.toContainText(/^prefill/);
    await expect(page.getByTestId('latency-dur-prefill')).not.toHaveText('—'); // a real (derived) duration
    await expect(page.getByTestId('latency-ttfb')).toHaveAttribute('data-quality', 'unavailable');

    expect(consoleErrors, 'console errors on the latency breakdown').toEqual([]);
  });

  // Gap 11: the FlowDetail inspector shows a FAILOVER / attempt-trace stepper from `attempts[]`.
  // A multi-attempt flow renders the chain (failed → served), the served node visually distinct,
  // and an UNMEASURED per-attempt time reads `—` (never 0). A single-attempt flow renders a single
  // node with NO failover claim (no fake chain). Covers the spec's "single-attempt AND failover
  // fixtures render correctly" acceptance.
  test('failover trace: chain on a failover flow, single node + — on no first byte (gap 11)', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'flows')!); // Flows
    await page.waitForTimeout(400);

    // api_005 is a FAILOVER flow (vllm-b failed → openai served). Rows render the (short) api_call_id
    // verbatim (`api_005` is ≤10 chars), so select by it — unambiguous. The trace shows a 2-node chain.
    await page.locator('[data-api-call-id="api_005"]').click();
    await expect(page.getByTestId('flow-detail')).toBeVisible();
    const trace = page.getByTestId('attempt-trace');
    await expect(trace).toBeVisible();
    await expect(trace).toHaveAttribute('data-failover', 'true');
    // The failover summary names the served handoff; the failed node carries its error class.
    await expect(page.getByTestId('attempt-failover-label')).toContainText('served');
    await expect(page.getByTestId('attempt-error-0')).toContainText('http status');
    // The served node (B) is marked distinct.
    await expect(page.getByTestId('attempt-node-1')).toHaveAttribute('data-served', 'true');
    await expect(page.getByTestId('attempt-status-1')).toHaveText('served');

    // Expand the FAILED node: its first byte is `—` (no header arrived) — NEVER 0 (spec 11 core).
    await page.getByTestId('attempt-toggle-0').click();
    const failedByte = page.getByTestId('attempt-firstbyte-0');
    await expect(failedByte).toHaveAttribute('data-quality', 'unavailable');
    await expect(failedByte).toHaveText('—');

    // api_003 is a SINGLE FAILED attempt (no failover): one node, the "no failover" label, no chain.
    await page.locator('[data-api-call-id="api_003"]').click();
    await expect(page.getByTestId('attempt-trace')).toHaveAttribute('data-failover', 'false');
    await expect(page.getByTestId('attempt-single-label')).toBeVisible();
    await expect(page.getByTestId('attempt-node-0')).toBeVisible();
    await expect(page.getByTestId('attempt-node-1')).toHaveCount(0);

    expect(consoleErrors, 'console errors on the failover trace').toEqual([]);
  });

  // Gap 13: the topology tooltip shows PER-PROVIDER p50/p95/p99 + error rate (replacing the old
  // GLOBAL p99), and nodes are sized/colored by per-provider latency/error. The per-provider data
  // comes from the REST `/topology` node (the WS frame carries it ABSENT) — so the tooltip reads the
  // REST path. Asserts the three states: a healthy provider (measured 0% — NOT —), a degrading one
  // (real percentiles + an error distribution + a degrading node), and an unavailable one (`—`,
  // never a fabricated 0ms/0%, neutral node).
  test('topology per-provider tooltip + node states (gap 13)', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'topology')!); // Topology
    // Let d3-force settle so the nodes sit at stable, hoverable positions.
    await page.waitForTimeout(800);

    // vllm-a (healthy, all-served): the tile shows derived percentiles + a MEASURED 0% error rate
    // (distinct from the unavailable `—`). Hover its node; the tooltip renders the per-provider tile.
    await page.locator('[data-node-id="vllm-a"]').hover();
    const tip = page.getByTestId('cooldown-tooltip');
    await expect(tip).toBeVisible();
    const tileA = tip.getByTestId('provider-latency-tile');
    await expect(tileA).toHaveAttribute('data-available', 'true');
    await expect(tip.getByTestId('provider-p50')).toHaveAttribute('data-quality', 'derived');
    await expect(tip.getByTestId('provider-p50')).not.toContainText('—');
    const errA = tip.getByTestId('provider-error-rate');
    await expect(errA).toHaveAttribute('data-quality', 'measured');
    await expect(errA).toContainText('0%'); // a real measured zero — NOT — and NOT absent
    await expect(errA).not.toContainText('—');
    // The healthy node is NEUTRAL (nominal emphasis, no error ring).
    expect(await page.locator('[data-node-id="vllm-a"]').getAttribute('data-emphasis')).toBe('nominal');

    // vllm-b (cooling, degrading): real derived percentiles + a per-class error distribution, and the
    // node is emphasized `degrading`. Move the hover to it.
    await page.locator('[data-node-id="vllm-b"]').hover();
    const tileB = page.getByTestId('cooldown-tooltip').getByTestId('provider-latency-tile');
    await expect(tileB).toHaveAttribute('data-available', 'true');
    const errB = page.getByTestId('cooldown-tooltip').getByTestId('provider-error-rate');
    await expect(errB).toHaveAttribute('data-quality', 'measured');
    await expect(errB).not.toHaveText('—'); // a measured, elevated rate
    // The error distribution lists the classes that occurred (connect + timeout in the mock).
    await expect(page.getByTestId('cooldown-tooltip').getByTestId('provider-error-distribution')).toBeVisible();
    await expect(page.getByTestId('cooldown-tooltip').getByTestId('provider-error-connect')).toBeVisible();
    expect(await page.locator('[data-node-id="vllm-b"]').getAttribute('data-emphasis')).toBe('degrading');
    await expect(page.locator('[data-node-id="vllm-b"] [data-testid="topo-error-ring"]')).toBeVisible();

    // openai (down, ZERO in-window samples): per-provider is ABSENT → the tile reads `—`
    // (unavailable), NEVER a fabricated 0ms/0%; the node stays NEUTRAL (not 0-sized / not healthy).
    await page.locator('[data-node-id="openai"]').hover();
    const tileC = page.getByTestId('cooldown-tooltip').getByTestId('provider-latency-tile');
    await expect(tileC).toHaveAttribute('data-available', 'false');
    const unavail = page.getByTestId('cooldown-tooltip').getByTestId('provider-latency-unavailable');
    await expect(unavail).toHaveAttribute('data-quality', 'unavailable');
    await expect(unavail).toContainText('—');
    expect(await page.locator('[data-node-id="openai"]').getAttribute('data-emphasis')).toBe('unavailable');

    expect(consoleErrors, 'console errors on the per-provider tooltip').toEqual([]);
  });

  // Gap 14: the AGGREGATE failure taxonomy panel groups failures by reason × provider/model with a
  // DERIVED error rate (+ an overall error-rate chip), and the inspector ErrorTab shows the captured
  // upstream error body when capture is ON — and an explicit "capture disabled" state when OFF (NOT a
  // blank implying "no error"). Rows are selected by stable `api_call_id` (gap-11 selector hygiene).
  test('failure taxonomy panel + ErrorTab capture on/off (gap 14)', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'flows')!); // Flows
    await page.waitForTimeout(400);

    // The aggregate panel renders (flows ARE observed). The overall error-rate chip is derived (not —).
    const panel = page.getByTestId('failure-taxonomy');
    await expect(panel).toBeVisible();
    const chip = page.getByTestId('failure-error-rate');
    await expect(chip).toHaveAttribute('data-quality', 'derived');
    await expect(page.getByTestId('failure-error-rate-value')).not.toHaveText('—'); // a measured/derived rate
    await expect(page.getByTestId('failure-grouping-quality')).toHaveAttribute('data-quality', 'measured');

    // The mock has FAILED flows on openai/gpt-4o (api_003, 503) and vllm-b/llama (api_006, timeout) ⇒
    // at least those two failure groups are listed, each with a derived rate + a reason chip.
    const groups = panel.getByTestId('failure-group');
    expect(await groups.count()).toBeGreaterThanOrEqual(2);
    // The vllm-b group (api_006, timeout) is present with a derived rate. Its reason chip uses a
    // BOUNDED gap-03 key (`class:timeout`), NOT the free-form terminal_reason string (review HIGH).
    const vllmGroup = panel.getByTestId('failure-group').filter({ has: page.getByTestId('failure-group-provider').filter({ hasText: 'vllm-b' }) }).first();
    await expect(vllmGroup.getByTestId('failure-group-rate')).toHaveAttribute('data-quality', 'derived');
    const timeoutReason = vllmGroup.getByTestId('failure-reason').filter({ hasText: 'timeout' }).first();
    await expect(timeoutReason).toBeVisible();
    await expect(timeoutReason).toHaveAttribute('data-reason-key', 'class:timeout');
    await expect(timeoutReason).toHaveAttribute('data-source', 'error_class');

    // ErrorTab — capture ON: api_003 (failed, 503) has a CAPTURED upstream error body. Select by id.
    await page.locator('[data-api-call-id="api_003"]').click();
    await expect(page.getByTestId('flow-detail')).toBeVisible();
    await page.getByRole('tab', { name: 'Error' }).click();
    const capture = page.getByTestId('error-capture');
    await expect(capture).toHaveAttribute('data-state', 'captured');
    await expect(capture).toHaveAttribute('data-quality', 'measured');
    await expect(page.getByTestId('error-capture-body')).toContainText('Service Unavailable');

    // ErrorTab — capture OFF: api_006 (failed, timeout) has NO captured body ⇒ explicit "capture
    // disabled" state (unavailable), NOT a blank implying "no error".
    await page.locator('[data-api-call-id="api_006"]').click();
    await page.getByRole('tab', { name: 'Error' }).click();
    const capture2 = page.getByTestId('error-capture');
    await expect(capture2).toHaveAttribute('data-state', 'unavailable');
    await expect(page.getByTestId('error-capture-disabled')).toBeVisible();
    await expect(page.getByTestId('error-capture-disabled')).toContainText(/capture disabled/i);
    // The terminal reason is still shown — this IS an error; the capture-disabled note is additive.
    await expect(page.getByTestId('error-tab')).toContainText('upstream timeout');

    expect(consoleErrors, 'console errors on the failure taxonomy').toEqual([]);
  });

  // Gap 15: the CLIENT column renders the non-secret client attribution (a key-hash `key-<hex>` /
  // configured caller-id / WEAK User-Agent fallback) with a source-strength tag, an UNATTRIBUTED flow
  // reads `—` (don't-lie-with-zeros), and the per-client filter + the "by client" roll-up work. Rows
  // are selected by stable `api_call_id` (gap-11 selector hygiene). This is an auth-gated DIAGNOSTIC
  // surface: showing the key-HASH (not a raw key) to the operator is the INTENDED purpose.
  test('client attribution: source-tagged CLIENT column + per-client filter + roll-up (gap 15)', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'flows')!); // Flows
    await page.waitForTimeout(400);

    const rowClient = (id: string) =>
      page.locator(`[data-api-call-id="${id}"]`).getByTestId('flow-client');

    // api_001: a STRONG key-hash identity → `measured`, the hash prefix shown (NEVER a raw key), `key` badge.
    const kh = rowClient('api_001');
    await expect(kh).toContainText('key-9f3a1c0b2d4e');
    await expect(kh).toHaveAttribute('data-quality', 'measured');
    await expect(kh).toHaveAttribute('data-strength', 'strong');
    await expect(rowClient('api_001').getByTestId('flow-client-source')).toHaveText('key');

    // api_004: a WEAK User-Agent fallback → `derived` (NOT measured), visibly weaker, `ua` badge. The
    // weak-UA distinction is the heart of the spec — a UA must never read as a confirmed identity.
    const ua = rowClient('api_004');
    await expect(ua).toContainText('python-httpx/0.27');
    await expect(ua).toHaveAttribute('data-quality', 'derived');
    await expect(ua).toHaveAttribute('data-strength', 'weak');
    await expect(rowClient('api_004').getByTestId('flow-client-source')).toHaveText('ua');

    // Review MEDIUM (column width): the CLIENT column must actually SHOW the label — not truncate two
    // distinct clients to a non-distinguishing prefix. Assert the visible cell text renders the FULL
    // labels AND the two clients are visually distinct (the widened minmax column fits them).
    const khLabel = (await rowClient('api_001').locator('span').first().textContent())?.trim() ?? '';
    const uaLabel = (await rowClient('api_004').locator('span').first().textContent())?.trim() ?? '';
    expect(khLabel).toBe('key-9f3a1c0b2d4e'); // full label, not a clipped prefix
    expect(uaLabel).toBe('python-httpx/0.27');
    expect(khLabel).not.toBe(uaLabel); // two seeded clients are distinguishable in the column
    // The rendered cell is wide enough that the text is not overflow-clipped (scrollWidth ≤ clientWidth).
    const khBox = rowClient('api_001').locator('span').first();
    const fits = await khBox.evaluate((el) => el.scrollWidth <= el.clientWidth + 1);
    expect(fits, 'the key-hash label fits the CLIENT column without truncation').toBe(true);

    // api_006: NO attribution → `—` (unavailable), NEVER a fabricated id / the HTTP method.
    const none = rowClient('api_006');
    await expect(none).toHaveText('—');
    await expect(none).toHaveAttribute('data-quality', 'unavailable');
    await expect(none).toHaveAttribute('data-attributed', 'false');

    // The AGGREGATE "by client" roll-up: expand it; the heaviest client (key-9f3a1c0b2d4e, api_001+002)
    // shows a derived 0% err, a derived mean latency, and a summed cost whose DQ tag is the WEAKEST of
    // its priced flows — api_001/002 are both `estimated` (unconfigured cache rate) ⇒ the cost is
    // `estimated` (labelled, never silently `measured`).
    await page.getByTestId('client-rollup-toggle').click();
    await expect(page.getByTestId('client-rollup-table')).toBeVisible();
    const khRow = page.getByTestId('client-rollup-row').filter({ hasText: 'key-9f3a1c0b2d4e' }).first();
    await expect(khRow).toHaveAttribute('data-strength', 'strong');
    await expect(khRow.getByTestId('client-rollup-flows')).toHaveText('2');
    await expect(khRow.getByTestId('client-rollup-err')).toHaveAttribute('data-quality', 'derived');
    await expect(khRow.getByTestId('client-rollup-cost')).toHaveAttribute('data-quality', 'estimated');
    // The weak-UA client's roll-up source tag is `derived` (never `measured`).
    const uaRow = page.getByTestId('client-rollup-row').filter({ hasText: 'python-httpx/0.27' }).first();
    await expect(uaRow.getByTestId('client-rollup-source')).toHaveAttribute('data-quality', 'derived');

    // Cross-link: clicking the key-hash client row SETS the per-client filter → the table narrows to
    // that client's 2 flows (api_001 + api_002). The filter chip is then active + toggle-off-able.
    await khRow.getByTestId('client-rollup-pick').click();
    await expect(page.getByTestId('flow-count')).toContainText('2 / 6');
    await expect(page.getByTestId('flow-row')).toHaveCount(2);
    // The active client chip clears via the filter-bar clear control, restoring all rows.
    await page.getByTestId('flow-filter-clear').click();
    await expect(page.getByTestId('flow-count')).toContainText('6 flows');

    expect(consoleErrors, 'console errors on the client attribution surface').toEqual([]);
  });

  test('a ~4 KiB client label stays bounded in the searchable facet', async ({ page, consoleErrors }) => {
    await installDeterminism(page);
    await page.goto('/dashboard/?mock=1&longclient=1', { waitUntil: 'networkidle' });
    await page.locator('input').first().fill('dev-token');
    await page.getByRole('button', { name: /sign in/i }).click();
    await expect(page.getByRole('navigation', { name: 'Dashboard' }).getByRole('button', { name: 'Chat', exact: true })).toBeVisible();
    await openView(page, VIEWS.find((view) => view.name === 'flows')!); // Flows
    await page.waitForTimeout(400);

    const filterBar = page.getByTestId('flow-filter-bar');
    await filterBar.getByRole('button', { name: 'Client' }).click();
    const longChipLabel = filterBar.locator('[title^="python-httpx/x"]').first();
    await expect(longChipLabel).toBeVisible();

    // REAL layout: the label CLIPS (content wider than its box) — the inline-block + max-width + overflow
    // actually take effect (the inline-span bug would NOT clip, leaving scrollWidth == clientWidth).
    const clipped = await longChipLabel.evaluate((el) => el.scrollWidth > el.clientWidth);
    expect(clipped, 'the ~4 KiB label span is clipped (overflow constrained)').toBe(true);
    // The label box is capped by the dropdown's central grid column.
    const labelWidth = await longChipLabel.evaluate((el) => el.clientWidth);
    expect(labelWidth, 'label box is bounded').toBeLessThanOrEqual(220);

    const dropdown = filterBar.getByTestId('client-facet');
    const chipWidth = await dropdown.evaluate((el) => el.getBoundingClientRect().width);
    expect(chipWidth, 'dropdown width is bounded').toBeLessThanOrEqual(320);
    const barWidth = await filterBar.evaluate((el) => el.getBoundingClientRect().width);
    expect(chipWidth, 'dropdown never exceeds the filter bar width').toBeLessThanOrEqual(barWidth);
    await expect(longChipLabel).toHaveAttribute('title', /^python-httpx\/x{4096}$/);

    expect(consoleErrors, 'console errors on the long-client chip').toEqual([]);
  });

  // Gap 16: the CONTROL-ROOM overview (the 5th route) COMPOSES the gap-01–15 surfaces into one honest
  // screen. Asserts: the route loads; the per-provider tiles read the REST/snapshot topology DTO (the
  // gap-12/13 wire source — a degrading provider shows real derived percentiles, NOT a fabricated 0);
  // a mixed-confidence cost leaderboard inherits the WEAKEST tag (estimated, labelled); an unpriced
  // model's cost reads — (never $0.00); the failure + client tiles compose the gap-14/15 models; and
  // an unreported token class reads — (don't-lie-with-zeros). Rows selected by stable identity.
  test('control-room overview composes the surfaces with honest DQ tags (gap 16)', async ({ page, consoleErrors }) => {
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'overview')!); // Overview
    await page.waitForTimeout(600);

    // The view + headline render (the unified gap-01 metrics tile, with measured active streams).
    await expect(page.getByTestId('overview-view')).toBeVisible();
    await expect(page.getByTestId('overview-headline')).toBeVisible();
    await expect(page.getByTestId('overview-hl-active')).toHaveAttribute('data-quality', 'measured');
    // The mock's $/min is an ESTIMATE (unconfigured cache rate) ⇒ the headline labels it estimated.
    await expect(page.getByTestId('overview-hl-cost')).toHaveAttribute('data-quality', 'estimated');

    // PER-PROVIDER tiles (gap 12/13 — from the REST/snapshot topology node, NOT the WS frame). vllm-b
    // is degrading: a real derived p50 (NOT —) + a measured error rate. This proves the wire source —
    // the live WS topology_update frame carries per_provider ABSENT, so a WS-only read would be empty.
    const providers = page.getByTestId('overview-providers');
    await expect(providers).toHaveAttribute('data-available', 'true');
    // `data-provider` is ON the tile element itself, so select by the attribute directly.
    const vllmB = providers.locator('[data-testid="overview-provider"][data-provider="vllm-b"]').first();
    await expect(vllmB.getByTestId('provider-p50')).toHaveAttribute('data-quality', 'derived');
    await expect(vllmB.getByTestId('provider-p50')).not.toContainText('—');
    await expect(vllmB.getByTestId('provider-error-rate')).toHaveAttribute('data-quality', 'measured');
    // vllm-a is healthy/all-served: a MEASURED 0% (distinct from the unavailable —).
    const vllmA = providers.locator('[data-testid="overview-provider"][data-provider="vllm-a"]').first();
    await expect(vllmA.getByTestId('provider-error-rate')).toContainText('0%');

    // TOP MODELS · COST: llama is priced via an UNCONFIGURED cache rate ⇒ estimated (labelled `est`),
    // never silently confident. The cost board is available (some flows are priced).
    const costBoard = page.getByTestId('overview-top-models-cost');
    await expect(costBoard).toHaveAttribute('data-available', 'true');
    const llamaCost = costBoard.getByTestId('overview-leaderboard-row').filter({ hasText: 'llama-3.1-70b' }).first();
    await expect(llamaCost.getByTestId('overview-leaderboard-cost')).toHaveAttribute('data-quality', 'estimated');
    await expect(llamaCost.getByTestId('overview-leaderboard-est')).toBeVisible();

    // TOP MODELS · VOLUME: mystery-model has an UNPRICED flow ⇒ its cost reads — (never $0.00).
    const volBoard = page.getByTestId('overview-top-models-volume');
    await expect(volBoard).toHaveAttribute('data-available', 'true');
    const mysteryRow = volBoard.getByTestId('overview-leaderboard-row').filter({ hasText: 'mystery-model' }).first();
    const mysteryCost = mysteryRow.getByTestId('overview-leaderboard-cost');
    await expect(mysteryCost).toHaveAttribute('data-quality', 'unavailable');
    await expect(mysteryCost).toHaveText('—');

    // TOP PROVIDERS · COST (review HIGH 1 — the spec requires providers by volume AND cost): vllm-a's
    // priced flows (api_001/002) are estimated ⇒ its provider-cost is estimated (labelled). The board
    // is available (some flows are priced).
    const provCostBoard = page.getByTestId('overview-top-providers-cost');
    await expect(provCostBoard).toHaveAttribute('data-available', 'true');
    const vllmACost = provCostBoard.getByTestId('overview-leaderboard-row').filter({ hasText: 'vllm-a' }).first();
    await expect(vllmACost.getByTestId('overview-leaderboard-cost')).toHaveAttribute('data-quality', 'estimated');

    // FAILURES tile (gap 14 model): flows fail on openai (api_003) + vllm-b (api_006) ⇒ a derived
    // overall rate (NOT —) + at least one failing group.
    const failures = page.getByTestId('overview-failures');
    await expect(failures).toHaveAttribute('data-available', 'true');
    await expect(failures.getByTestId('overview-failures-rate')).toHaveAttribute('data-quality', 'derived');
    expect(await failures.getByTestId('overview-failure-group').count()).toBeGreaterThanOrEqual(1);

    // CLIENTS tile (gap 15 model): the heaviest client is the key-hash (api_001+002), shown as the
    // one-way HASH (the auth-gated diagnostic purpose — never a raw key); api_006 is unattributed.
    const clients = page.getByTestId('overview-clients');
    await expect(clients).toHaveAttribute('data-available', 'true');
    await expect(clients.getByTestId('overview-client-row').filter({ hasText: 'key-9f3a1c0b2d4e' }).first()).toBeVisible();
    await expect(clients).toContainText(/unattributed/);
    // The weak UA client is rendered with a `ua` badge (a fallback, not a confirmed identity).
    await expect(clients.getByTestId('overview-client-row').filter({ hasText: 'python-httpx/0.27' }).first().getByTestId('overview-client-ua')).toBeVisible();

    // TOKEN MIX: prompt is measured; the cached class is reported by some flows (api_001) so it is
    // measured too — but reasoning, reported by none of the usage flows as > nothing, stays honest.
    const mix = page.getByTestId('overview-token-mix');
    await expect(mix).toHaveAttribute('data-available', 'true');
    await expect(mix.getByTestId('overview-token-prompt')).toHaveAttribute('data-quality', 'measured');

    // CONTEXT PRESSURE (gap 09 model): a derived peak (the known-window flows are measurable) with a
    // measured/total coverage readout.
    await expect(page.getByTestId('overview-context-coverage')).toContainText('measured');

    expect(consoleErrors, 'console errors on the control-room overview').toEqual([]);
  });

  test('chat streams a complete response without narrow-screen overflow', async ({ page, consoleErrors }) => {
    await page.setViewportSize({ width: 800, height: 900 });
    await login(page);
    await openView(page, VIEWS.find((view) => view.name === 'chat')!);
    await expect(page.getByLabel('Thinking level')).toHaveValue('medium');
    await expect(page.getByLabel('Temperature')).toHaveValue('1');
    await expect(page.getByLabel('Top P')).toHaveValue('0.95');
    await expect(page.getByLabel('Max context length')).toHaveValue('4096');
    await page.getByLabel('Message').fill('ping');
    await page.getByRole('button', { name: 'Send', exact: true }).click();

    await expect(page.getByTestId('chat-message-assistant')).toContainText('Mock response from gpt-4o: ping');
    await expect(page.getByTestId('chat-thinking')).toContainText('Checking the request.');
    await expect(page.getByTestId('chat-thinking').locator('strong')).toContainText('Checking');
    await expect(page.getByTestId('chat-run-status')).toContainText('finish: stop');
    const widths = await page.evaluate(() => ({ viewport: innerWidth, document: document.documentElement.scrollWidth }));
    expect(widths.document).toBeLessThanOrEqual(widths.viewport);
    expect(consoleErrors, 'console errors on dashboard chat').toEqual([]);
  });

  for (const view of VIEWS) {
    test(`${view.name}: renders + no console errors + matches baseline`, async ({ page, consoleErrors }) => {
      await login(page);
      await openView(page, view);
      if (view.name === 'sessions') {
        await page.getByTestId('all-sessions-section').getByTestId('session-row').filter({ hasText: 'claude-code' }).first().click();
        await expect(page.getByTestId('all-sessions-section').getByTestId('session-roots-expanded-row')).toHaveCount(1);
      }
      // Let d3-force / uPlot / sankey reach their settled frame before the pixel baseline.
      // (mock streams a finite snapshot + 5 frames, then is quiescent.)
      await page.waitForTimeout(800);
      await expect(page).toHaveScreenshot(`${view.name}.png`);
      expect(consoleErrors, `console errors on ${view.name}`).toEqual([]);
    });
  }
});
