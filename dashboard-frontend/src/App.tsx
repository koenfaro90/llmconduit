/**
 * App shell (D9 §"Hash router + layout shell"): stats-strip slot (top), scrubber slot
 * (under it), view router with the four routes. Gated by auth — the login shell renders
 * when unauthenticated; a 401 anywhere routes through `teardownSession()` (cache cleared,
 * stores reset, WS closed) which flips auth off and returns to the login shell.
 */
import { useEffect, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { getConnection, teardownSession } from './api/connection';
import { useAuth } from './store/hooks';
import { LoginShell } from './components/LoginShell';
import { NavTabs } from './components/NavTabs';
import { StatsStrip } from './components/StatsStrip/StatsStrip';
import { Scrubber } from './components/Scrubber/Scrubber';
import { useHashRoute } from './router/useHashRoute';
import { VIEW_BY_ROUTE } from './views/registry';

export function App() {
  const authed = useAuth((s) => s.authenticated);
  const { client } = getConnection();

  if (!authed) {
    return <LoginShell client={client} />;
  }
  return <Dashboard />;
}

function Dashboard() {
  const { client, socket } = getConnection();
  const route = useHashRoute();
  const ActiveView = VIEW_BY_ROUTE[route];
  const layout = useQuery({ queryKey: ['traffic-layout'], queryFn: () => client.trafficLayout(), retry: false, refetchInterval: 30000 });
  const [compactPreference, setCompactPreference] = useState<boolean | null>(() => { try { const value = localStorage.getItem('llmconduit.mobile.compact'); return value == null ? null : value === 'true'; } catch { return null; } });
  const compact = compactPreference ?? layout.data?.mobile_compact ?? false;
  function toggleCompact() { const next = !compact; setCompactPreference(next); try { localStorage.setItem('llmconduit.mobile.compact', String(next)); } catch { /* optional preference */ } }

  // Open the WS once on mount; close on unmount. StrictMode double-mounts in dev — the
  // socket.connect()/disconnect() pair is idempotent so no duplicate pipe leaks.
  useEffect(() => {
    socket.connect();
    return () => socket.disconnect();
  }, [socket]);

  async function onLogout() {
    try {
      await client.logout();
    } finally {
      // Centralized teardown: clears the REST cache + resets both stores + closes the WS,
      // so no session-scoped data survives logout (finding 1).
      teardownSession();
    }
  }

  return (
    <div className={`flex h-full min-w-0 flex-col bg-bg text-text ${compact ? 'dashboard-compact' : ''}`}>
      <NavTabs active={route} onLogout={onLogout} compact={compact} onToggleCompact={toggleCompact} />
      {/* stats-strip slot */}
      <div className="dashboard-global-stats"><StatsStrip /></div>
      {/* Access is current authorization state, not a historical telemetry surface. */}
      {route !== 'access' && route !== 'chat' && route !== 'requests' && route !== 'traffic' && <Scrubber socket={socket} />}
      {/* view router */}
      <main className="flex flex-1 overflow-hidden">
        <ActiveView />
      </main>
    </div>
  );
}
