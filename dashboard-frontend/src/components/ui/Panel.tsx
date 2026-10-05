import type { ComponentPropsWithoutRef, ReactNode } from 'react';
import {HelpIcon} from './HelpIcon';
import { cn } from '../../lib/cn';

/**
 * A dark-ops surface panel (shadcn-style primitive). Forwards arbitrary `<div>` props (incl.
 * `data-testid`/`data-*`/`aria-*`) to the root so callers can tag the surface for tests/e2e
 * without wrapping it (a bare `data-testid` on `<Panel>` was previously dropped silently).
 */
export function Panel({
  children,
  className,
  raised,
  ...rest
}: {
  children: ReactNode;
  className?: string;
  raised?: boolean;
} & Omit<ComponentPropsWithoutRef<'div'>, 'className' | 'children'>) {
  return (
    <div
      className={cn(
        'rounded-md border border-line',
        raised ? 'bg-panel-raised' : 'bg-panel',
        className,
      )}
      {...rest}
    >
      {children}
    </div>
  );
}

/** Keep actions attached to the surface they operate on, above its contents. */
export function PanelHeader({children,actions,help,label}: {children:ReactNode;actions?:ReactNode;help?:string;label?:string}) {
  return <div className="flex min-w-0 flex-wrap items-start justify-between gap-3" data-testid="panel-header"><div className="min-w-0 flex items-start gap-2"><div className="min-w-0">{children}</div>{help&&<HelpIcon label={label??'Configuration section'}>{help}</HelpIcon>}</div>{actions&&<div className="ml-auto flex max-w-full flex-wrap items-center justify-end gap-2">{actions}</div>}</div>;
}
