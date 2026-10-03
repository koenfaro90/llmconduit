/** Route → view-component map. Kept out of the .tsx so react-refresh stays happy. */
import type { ComponentType } from 'react';
import type { RouteName } from '../router/useHashRoute';
import { FlowsView } from './FlowsView';
import { RequestsView } from './requests/RequestsView';
import { TopologyView } from './TopologyView';
import { SankeyView } from './SankeyView';
import { TheaterView } from './TheaterView';
import { OverviewView } from './overview/OverviewView';
import { SessionsView } from './sessions/SessionsView';
import { ThroughputView } from './throughput/ThroughputView';
import { ActivityView } from './activity/ActivityView';
import { AccountView } from './account/AccountView';
import { ProvidersView } from './providers/ProvidersView';
import { AccessView } from './access/AccessView';
import { ChatView } from './chat/ChatView';

import { BenchmarksView } from './infrastructure/BenchmarksView';
import { HostsView } from './infrastructure/HostsView';
import { RecipesView } from './infrastructure/RecipesView';
import { ModelsView } from './infrastructure/ModelsView';
import { DeploymentView } from './infrastructure/DeploymentView';

export const VIEW_BY_ROUTE: Record<RouteName, ComponentType> = {
  flows: FlowsView,
  requests: RequestsView,
  sessions: SessionsView,
  throughput: ThroughputView,
  activity: ActivityView,
  chat: ChatView,
  account: AccountView,
  topology: TopologyView,
  sankey: SankeyView,
  theater: TheaterView,
  overview: OverviewView,
  providers: ProvidersView,
  hosts: HostsView,
  benchmarks: BenchmarksView,
  recipes: RecipesView,
  models: ModelsView,
  deployment: DeploymentView,
  access: AccessView,
};
